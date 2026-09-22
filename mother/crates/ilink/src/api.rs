//! iLink Bot API HTTP client.
//!
//! Stateless async functions wrapping all iLink endpoints at `ilinkai.weixin.qq.com`.

use base64::Engine;
use rand::Rng;
use reqwest::{header::HeaderMap, Client};
use std::time::Duration;
use carrier_types::error::{CarrierError, CarrierResult};

use crate::models::*;

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

/// Send an image message to a WeChat user via iLink.
///
/// `context_token` is optional — see [`send_message`] for the verified
/// delivery model; prefer [`send_image_auto`].
pub async fn send_image(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    image_url: &str,
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
                type_: ITEM_TYPE_IMAGE,
                text_item: None,
                image_item: Some(SendImageItem {
                    image_url: image_url.to_string(),
                }),
                video_item: None,
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

/// Token policy wrapper around [`send_image`] — see [`send_message_auto`].
pub async fn send_image_auto(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    image_url: &str,
) -> CarrierResult<()> {
    if let Some(tok) = context_token {
        match send_image(
            http,
            bot_token,
            baseurl,
            to_user_id,
            Some(tok),
            client_id,
            image_url,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(to = %to_user_id, error = %e, "send_image with context_token failed, retrying bare")
            }
        }
    }
    send_image(
        http, bot_token, baseurl, to_user_id, None, client_id, image_url,
    )
    .await
}

/// POST `/ilink/bot/sendmessage` with ITEM_TYPE_VIDEO
///
/// Send a video message to a WeChat user via iLink.
///
/// `context_token` is optional — see [`send_message`] for the verified
/// delivery model; prefer [`send_video_auto`].
pub async fn send_video(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    video_url: &str,
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
                    video_url: video_url.to_string(),
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

/// Token policy wrapper around [`send_video`] — see [`send_message_auto`].
pub async fn send_video_auto(
    http: &Client,
    bot_token: &str,
    baseurl: &str,
    to_user_id: &str,
    context_token: Option<&str>,
    client_id: &str,
    video_url: &str,
) -> CarrierResult<()> {
    if let Some(tok) = context_token {
        match send_video(
            http,
            bot_token,
            baseurl,
            to_user_id,
            Some(tok),
            client_id,
            video_url,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(to = %to_user_id, error = %e, "send_video with context_token failed, retrying bare")
            }
        }
    }
    send_video(
        http, bot_token, baseurl, to_user_id, None, client_id, video_url,
    )
    .await
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
}
