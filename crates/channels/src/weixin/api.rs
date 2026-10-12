//! iLink Bot API HTTP client.
//!
//! Stateless async functions wrapping all iLink endpoints at `ilinkai.weixin.qq.com`.

use base64::Engine;
use rand::Rng;
use reqwest::{header::HeaderMap, Client};
use std::time::Duration;
use crate::vocab::{CarrierError, CarrierResult};

use super::models::*;

/// Build the required iLink request headers (with optional Bearer token).
fn ilink_headers(bot_token: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("Content-Type", "application/json".parse().unwrap());
    h.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
    h.insert("iLink-App-Id", "bot".parse().unwrap());

    // Client version: (major << 16) | (minor << 8) | patch
    let client_ver = ((1u32 << 16) | 2).to_string();
    h.insert("iLink-App-ClientVersion", client_ver.parse().unwrap());

    // X-WECHAT-UIN: random uint32 -> decimal string -> base64
    let uin = rand::thread_rng().gen::<u32>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(uin.to_string());
    h.insert("X-WECHAT-UIN", encoded.parse().unwrap());

    if let Some(token) = bot_token {
        if !token.is_empty() {
            h.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        }
    }

    h
}

/// GET `/ilink/bot/get_bot_qrcode?bot_type=3`
///
/// No auth required. Returns QR code for WeChat scanning.
pub async fn get_bot_qrcode(http: &Client) -> CarrierResult<QrCodeResponse> {
    get_bot_qrcode_with_base(http, ILINK_API_BASE).await
}

/// GET `<base>/ilink/bot/get_bot_qrcode?bot_type=3` with custom base URL.
pub async fn get_bot_qrcode_with_base(
    http: &Client,
    base_url: &str,
) -> CarrierResult<QrCodeResponse> {
    let url = format!("{base_url}/ilink/bot/get_bot_qrcode?bot_type={BOT_TYPE}");
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("get_bot_qrcode request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "get_bot_qrcode HTTP {status}: {body}"
        )));
    }

    resp.json::<QrCodeResponse>()
        .await
        .map_err(|e| CarrierError::Serialization(format!("get_bot_qrcode parse error: {e}")))
}

/// GET `<base>/ilink/bot/get_qrcode_status?qrcode=xxx`
///
/// No auth required. Long-polls for scan status (server holds up to 35s).
pub async fn get_qrcode_status(
    http: &Client,
    base_url: &str,
    qrcode: &str,
) -> CarrierResult<QrCodeStatusResponse> {
    let url = format!(
        "{base_url}/ilink/bot/get_qrcode_status?qrcode={}",
        urlencoding::encode(qrcode)
    );
    let resp = http
        .get(&url)
        .timeout(Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("get_qrcode_status request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "get_qrcode_status HTTP {status}: {body}"
        )));
    }

    // iLink may return application/octet-stream content type
    let text = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("get_qrcode_status read body error: {e}")))?;

    serde_json::from_str::<QrCodeStatusResponse>(&text).map_err(|e| {
        CarrierError::Serialization(format!("get_qrcode_status parse error: {e}: {text}"))
    })
}

/// POST `/ilink/bot/getupdates`
///
/// Long-poll receive messages. Server holds up to 35s.
pub async fn get_updates(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    cursor: &str,
) -> CarrierResult<GetUpdatesResponse> {
    let url = format!("{baseurl}/ilink/bot/getupdates");
    let body = GetUpdatesRequest {
        get_updates_buf: cursor.to_string(),
        base_info: BaseInfo::default(),
    };

    let resp = http
        .post(&url)
        .headers(ilink_headers(Some(bot_token)))
        .json(&body)
        .timeout(Duration::from_millis(LONG_POLL_TIMEOUT_MS + 5_000))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("getupdates request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "getupdates HTTP {status}: {body}"
        )));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("getupdates read body error: {e}")))?;

    serde_json::from_str::<GetUpdatesResponse>(&text)
        .map_err(|e| CarrierError::Serialization(format!("getupdates parse error: {e}")))
}

/// POST `/ilink/bot/sendmessage`
///
/// Send a message to a WeChat user via iLink.
///
/// `context_token` is OPTIONAL — verified against production 2026-08-19:
/// bare sends (field omitted) deliver fine. What actually decides delivery
/// is account-to-account reachability: the recipient account must be alive
/// (dormant accounts fail at `prepare`), and the sender account must have a
/// relationship with the recipient (it IS the recipient, or they chatted
/// before — stranger sends return a message_id but are silently dropped).
/// A STALE cached token makes sends fail upstream, so callers should prefer
/// [`send_message_auto`], which retries bare on failure.
pub async fn send_message(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    text: &str,
) -> CarrierResult<()> {
    let url = format!("{baseurl}/ilink/bot/sendmessage");

    let req = SendMessageRequest {
        msg: SendMessageMsg {
            from_user_id: String::new(),
            to_user_id: to_user_id.to_string(),
            client_id: client_id.to_string(),
            message_type: MSG_TYPE_BOT,
            message_state: MSG_STATE_FINISH,
            context_token: context_token.map(|t| t.to_string()),
            item_list: Some(vec![SendItem {
                type_: ITEM_TYPE_TEXT,
                text_item: Some(SendTextItem {
                    text: text.to_string(),
                }),
                image_item: None,
                video_item: None,
                file_item: None,
            }]),
        },
        base_info: BaseInfo::default(),
    };

    let resp = http
        .post(&url)
        .headers(ilink_headers(Some(bot_token)))
        .json(&req)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("sendmessage request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "sendmessage HTTP {status}: {body}"
        )));
    }

    // iLink returns empty JSON or { } on success
    let _ = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("sendmessage read body error: {e}")))?;

    Ok(())
}

/// Token policy wrapper around [`send_message`]: try with the cached
/// context_token when present; if that fails (e.g. the token went stale
/// upstream), retry once bare. Bare sends are the protocol's baseline —
/// see [`send_message`] for the verified delivery model.
pub async fn send_message_auto(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    text: &str,
) -> CarrierResult<()> {
    if let Some(tok) = context_token {
        match send_message(
            http,
            bot_token,
            baseurl,
            to_user_id,
            Some(tok),
            client_id,
            text,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(to = %to_user_id, error = %e, "send with context_token failed, retrying bare")
            }
        }
    }
    send_message(http, bot_token, baseurl, to_user_id, None, client_id, text).await
}

/// POST `/ilink/bot/getuploadurl` — pre-sign a CDN upload slot.
pub async fn get_upload_url(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    req: &GetUploadUrlRequest,
) -> CarrierResult<GetUploadUrlResponse> {
    let url = format!("{baseurl}/ilink/bot/getuploadurl");
    let resp = http
        .post(&url)
        .headers(ilink_headers(Some(bot_token)))
        .json(req)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("getuploadurl request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "getuploadurl HTTP {status}: {body}"
        )));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("getuploadurl read body error: {e}")))?;

    serde_json::from_str::<GetUploadUrlResponse>(&text).map_err(|e| {
        CarrierError::Serialization(format!("getuploadurl parse error: {e}: {text}"))
    })
}

/// Encrypt plaintext and POST to WeChat CDN; return `x-encrypted-param`.
///
/// Prefer `upload_full_url` from getuploadurl; otherwise build from
/// `upload_param` + filekey against [`CDN_BASE_URL`].
pub async fn upload_buffer_to_cdn(
    http: &Client,
    plaintext: &[u8],
    aes_key: &[u8; 16],
    filekey: &str,
    upload_full_url: Option<&str>,
    upload_param: Option<&str>,
) -> CarrierResult<String> {
    use super::crypto::{aes_128_ecb_encrypt, cdn_upload_url};

    let ciphertext = aes_128_ecb_encrypt(plaintext, aes_key);
    let cdn_url = if let Some(full) = upload_full_url.map(str::trim).filter(|s| !s.is_empty()) {
        full.to_string()
    } else if let Some(param) = upload_param.filter(|s| !s.is_empty()) {
        cdn_upload_url(param, filekey)
    } else {
        return Err(CarrierError::Network(
            "CDN upload URL missing (need upload_full_url or upload_param)".into(),
        ));
    };

    let mut last_err = String::new();
    for attempt in 1..=3u32 {
        let resp = match http
            .post(&cdn_url)
            .header("Content-Type", "application/octet-stream")
            .body(ciphertext.clone())
            .timeout(Duration::from_secs(60))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("CDN upload network error attempt={attempt}: {e}");
                continue;
            }
        };
        let status = resp.status();
        if status.as_u16() >= 400 && status.as_u16() < 500 {
            let err_msg = resp
                .headers()
                .get("x-error-message")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let body = resp.text().await.unwrap_or_default();
            return Err(CarrierError::Network(format!(
                "CDN upload client error {status}: {err_msg} {body}"
            )));
        }
        if !status.is_success() {
            let err_msg = resp
                .headers()
                .get("x-error-message")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("status {status}"));
            last_err = format!("CDN upload server error attempt={attempt}: {err_msg}");
            continue;
        }
        let download_param = resp
            .headers()
            .get("x-encrypted-param")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        if let Some(p) = download_param.filter(|s| !s.is_empty()) {
            return Ok(p);
        }
        last_err = format!("CDN upload missing x-encrypted-param attempt={attempt}");
    }
    Err(CarrierError::Network(last_err))
}

/// Full outbound media pipeline: read → MD5 → AES key → getuploadurl → CDN POST.
pub async fn upload_media_bytes(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    plaintext: &[u8],
    media_type: u32,
) -> CarrierResult<UploadedMedia> {
    use super::crypto::{
        aes_ecb_padded_size, aes_key_hex, generate_aes_key,
    };
    use md5::{Digest, Md5};
    use rand::RngCore;

    let rawsize = plaintext.len() as u64;
    let mut hasher = Md5::new();
    hasher.update(plaintext);
    let rawfilemd5 = hex::encode(hasher.finalize());
    let filesize = aes_ecb_padded_size(plaintext.len()) as u64;
    let mut filekey_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut filekey_bytes);
    let filekey = hex::encode(filekey_bytes);
    let aes_key = generate_aes_key();

    let req = GetUploadUrlRequest {
        filekey: filekey.clone(),
        media_type,
        to_user_id: to_user_id.to_string(),
        rawsize,
        rawfilemd5: rawfilemd5.clone(),
        filesize,
        no_need_thumb: Some(true),
        aeskey: aes_key_hex(&aes_key),
        base_info: BaseInfo::default(),
    };

    let url_resp = get_upload_url(http, bot_token, baseurl, &req).await?;
    let download = upload_buffer_to_cdn(
        http,
        plaintext,
        &aes_key,
        &filekey,
        url_resp.upload_full_url.as_deref(),
        url_resp.upload_param.as_deref(),
    )
    .await?;

    Ok(UploadedMedia {
        filekey,
        download_encrypt_query_param: download,
        aes_key,
        plaintext_size: rawsize,
        ciphertext_size: filesize,
        plaintext_md5_hex: rawfilemd5,
    })
}

/// Upload a local file path through the CDN pipeline.
pub async fn upload_media_file(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    path: &std::path::Path,
    media_type: u32,
) -> CarrierResult<UploadedMedia> {
    let plaintext = std::fs::read(path).map_err(|e| {
        CarrierError::Io(std::io::Error::new(
            e.kind(),
            format!("read media {}: {e}", path.display()),
        ))
    })?;
    if plaintext.is_empty() {
        return Err(CarrierError::InvalidInput(format!(
            "media file empty: {}",
            path.display()
        )));
    }
    upload_media_bytes(http, bot_token, baseurl, to_user_id, &plaintext, media_type).await
}

fn uploaded_to_cdn_media(uploaded: &UploadedMedia) -> SendCdnMedia {
    use super::crypto::aes_key_to_media_b64;
    SendCdnMedia {
        encrypt_query_param: uploaded.download_encrypt_query_param.clone(),
        aes_key: aes_key_to_media_b64(&uploaded.aes_key),
        encrypt_type: Some(1),
    }
}

/// Send an image via CDN refs (after [`upload_media_file`] / [`upload_media_bytes`]).
pub async fn send_image(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    uploaded: &UploadedMedia,
) -> CarrierResult<()> {
    use super::crypto::aes_key_hex;
    let url = format!("{baseurl}/ilink/bot/sendmessage");
    let req = SendMessageRequest {
        msg: SendMessageMsg {
            from_user_id: String::new(),
            to_user_id: to_user_id.to_string(),
            client_id: client_id.to_string(),
            message_type: MSG_TYPE_BOT,
            message_state: MSG_STATE_FINISH,
            context_token: context_token.map(|t| t.to_string()),
            item_list: Some(vec![SendItem {
                type_: ITEM_TYPE_IMAGE,
                text_item: None,
                image_item: Some(SendImageItem {
                    media: uploaded_to_cdn_media(uploaded),
                    aeskey: Some(aes_key_hex(&uploaded.aes_key)),
                    mid_size: Some(uploaded.ciphertext_size),
                    hd_size: Some(uploaded.ciphertext_size),
                }),
                video_item: None,
                file_item: None,
            }]),
        },
        base_info: BaseInfo::default(),
    };

    let resp = http
        .post(&url)
        .headers(ilink_headers(Some(bot_token)))
        .json(&req)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("send_image request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "send_image HTTP {status}: {body}"
        )));
    }
    let _ = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("send_image read body error: {e}")))?;
    Ok(())
}

/// Token-policy wrapper around [`send_image`].
pub async fn send_image_auto(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    uploaded: &UploadedMedia,
) -> CarrierResult<()> {
    if let Some(tok) = context_token {
        match send_image(
            http, bot_token, baseurl, to_user_id, Some(tok), client_id, uploaded,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(to = %to_user_id, error = %e, "send_image with context_token failed, retrying bare")
            }
        }
    }
    send_image(http, bot_token, baseurl, to_user_id, None, client_id, uploaded).await
}

/// Upload local image + sendmessage with image_item CDN refs.
pub async fn send_image_file(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    path: &std::path::Path,
) -> CarrierResult<UploadedMedia> {
    let uploaded = upload_media_file(
        http, bot_token, baseurl, to_user_id, path, UPLOAD_MEDIA_IMAGE,
    )
    .await?;
    send_image_auto(
        http, bot_token, baseurl, to_user_id, context_token, client_id, &uploaded,
    )
    .await?;
    Ok(uploaded)
}

/// Send a video via CDN refs.
pub async fn send_video(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    uploaded: &UploadedMedia,
) -> CarrierResult<()> {
    let url = format!("{baseurl}/ilink/bot/sendmessage");
    let req = SendMessageRequest {
        msg: SendMessageMsg {
            from_user_id: String::new(),
            to_user_id: to_user_id.to_string(),
            client_id: client_id.to_string(),
            message_type: MSG_TYPE_BOT,
            message_state: MSG_STATE_FINISH,
            context_token: context_token.map(|t| t.to_string()),
            item_list: Some(vec![SendItem {
                type_: ITEM_TYPE_VIDEO,
                text_item: None,
                image_item: None,
                video_item: Some(SendVideoItem {
                    media: uploaded_to_cdn_media(uploaded),
                    video_size: Some(uploaded.ciphertext_size),
                    play_length: None,
                }),
                file_item: None,
            }]),
        },
        base_info: BaseInfo::default(),
    };

    let resp = http
        .post(&url)
        .headers(ilink_headers(Some(bot_token)))
        .json(&req)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("send_video request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "send_video HTTP {status}: {body}"
        )));
    }
    let _ = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("send_video read body error: {e}")))?;
    Ok(())
}

/// Token-policy wrapper around [`send_video`].
pub async fn send_video_auto(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    uploaded: &UploadedMedia,
) -> CarrierResult<()> {
    if let Some(tok) = context_token {
        match send_video(
            http, bot_token, baseurl, to_user_id, Some(tok), client_id, uploaded,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(to = %to_user_id, error = %e, "send_video with context_token failed, retrying bare")
            }
        }
    }
    send_video(http, bot_token, baseurl, to_user_id, None, client_id, uploaded).await
}

/// Upload local video + sendmessage with video_item CDN refs.
pub async fn send_video_file(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    path: &std::path::Path,
) -> CarrierResult<UploadedMedia> {
    let uploaded = upload_media_file(
        http, bot_token, baseurl, to_user_id, path, UPLOAD_MEDIA_VIDEO,
    )
    .await?;
    send_video_auto(
        http, bot_token, baseurl, to_user_id, context_token, client_id, &uploaded,
    )
    .await?;
    Ok(uploaded)
}

/// Send a file attachment via CDN refs.
pub async fn send_file(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    uploaded: &UploadedMedia,
    file_name: &str,
) -> CarrierResult<()> {
    let url = format!("{baseurl}/ilink/bot/sendmessage");
    let req = SendMessageRequest {
        msg: SendMessageMsg {
            from_user_id: String::new(),
            to_user_id: to_user_id.to_string(),
            client_id: client_id.to_string(),
            message_type: MSG_TYPE_BOT,
            message_state: MSG_STATE_FINISH,
            context_token: context_token.map(|t| t.to_string()),
            item_list: Some(vec![SendItem {
                type_: ITEM_TYPE_FILE,
                text_item: None,
                image_item: None,
                video_item: None,
                file_item: Some(SendFileItem {
                    media: uploaded_to_cdn_media(uploaded),
                    file_name: file_name.to_string(),
                    len: Some(uploaded.plaintext_size.to_string()),
                    md5: Some(uploaded.plaintext_md5_hex.clone()),
                }),
            }]),
        },
        base_info: BaseInfo::default(),
    };

    let resp = http
        .post(&url)
        .headers(ilink_headers(Some(bot_token)))
        .json(&req)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("send_file request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "send_file HTTP {status}: {body}"
        )));
    }
    let _ = resp
        .text()
        .await
        .map_err(|e| CarrierError::Network(format!("send_file read body error: {e}")))?;
    Ok(())
}

/// Token-policy wrapper around [`send_file`].
pub async fn send_file_auto(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    uploaded: &UploadedMedia,
    file_name: &str,
) -> CarrierResult<()> {
    if let Some(tok) = context_token {
        match send_file(
            http, bot_token, baseurl, to_user_id, Some(tok), client_id, uploaded, file_name,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(to = %to_user_id, error = %e, "send_file with context_token failed, retrying bare")
            }
        }
    }
    send_file(
        http, bot_token, baseurl, to_user_id, None, client_id, uploaded, file_name,
    )
    .await
}

/// Upload local file attachment + sendmessage with file_item CDN refs.
pub async fn send_file_path(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    path: &std::path::Path,
) -> CarrierResult<UploadedMedia> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let uploaded = upload_media_file(
        http, bot_token, baseurl, to_user_id, path, UPLOAD_MEDIA_FILE,
    )
    .await?;
    send_file_auto(
        http, bot_token, baseurl, to_user_id, context_token, client_id, &uploaded, &file_name,
    )
    .await?;
    Ok(uploaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_request_omits_context_token_when_none() {
        // The protocol tests (2026-08-19) sent exactly this shape: no
        // context_token key at all. `skip_serializing_if` must hold.
        let req = SendMessageRequest {
            msg: SendMessageMsg {
                from_user_id: String::new(),
                to_user_id: "peer@im.wechat".to_string(),
                client_id: "c".to_string(),
                message_type: MSG_TYPE_BOT,
                message_state: MSG_STATE_FINISH,
                context_token: None,
                item_list: Some(vec![SendItem {
                    type_: ITEM_TYPE_TEXT,
                    text_item: Some(SendTextItem {
                        text: "hi".to_string(),
                    }),
                    image_item: None,
                    video_item: None,
                    file_item: None,
                }]),
            },
            base_info: BaseInfo::default(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            !json.contains("context_token"),
            "bare send must omit the field: {json}"
        );
        assert!(json.contains("peer@im.wechat"));
    }

    #[test]
    fn send_request_includes_context_token_when_some() {
        let req = SendMessageRequest {
            msg: SendMessageMsg {
                from_user_id: String::new(),
                to_user_id: "peer@im.wechat".to_string(),
                client_id: "c".to_string(),
                message_type: MSG_TYPE_BOT,
                message_state: MSG_STATE_FINISH,
                context_token: Some("tok".to_string()),
                item_list: None,
            },
            base_info: BaseInfo::default(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("context_token"));
        assert!(json.contains("\"tok\""));
    }

    #[test]
    fn getuploadurl_request_shape() {
        let req = GetUploadUrlRequest {
            filekey: "abcd".into(),
            media_type: UPLOAD_MEDIA_IMAGE,
            to_user_id: "u".into(),
            rawsize: 10,
            rawfilemd5: "d41d8cd98f00b204e9800998ecf8427e".into(),
            filesize: 16,
            no_need_thumb: Some(true),
            aeskey: "00".repeat(16),
            base_info: BaseInfo::default(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"filekey\":\"abcd\""));
        assert!(json.contains("\"media_type\":1"));
        assert!(json.contains("\"no_need_thumb\":true"));
        assert!(json.contains("\"aeskey\""));
        assert!(json.contains("channel_version"));
    }

    #[test]
    fn image_item_cdn_shape() {
        use super::super::crypto::{aes_key_to_media_b64, generate_aes_key};
        let key = generate_aes_key();
        let item = SendImageItem {
            media: SendCdnMedia {
                encrypt_query_param: "param".into(),
                aes_key: aes_key_to_media_b64(&key),
                encrypt_type: Some(1),
            },
            aeskey: Some(hex::encode(key)),
            mid_size: Some(32),
            hd_size: Some(32),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("encrypt_query_param"));
        assert!(json.contains("aes_key"));
        assert!(!json.contains("image_url"));
        assert!(json.contains("mid_size"));
    }
}
