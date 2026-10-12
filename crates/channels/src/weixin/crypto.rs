//! AES-128-ECB encryption/decryption for WeChat iLink CDN media.
//!
//! All CDN media uses AES-128-ECB with PKCS7 padding.
//! Ciphertext size = ceil((plaintext_size + 1) / 16) * 16.
//!
//! Key encoding (openclaw-weixin / qwen-code dual form):
//! - `getuploadurl.aeskey`: hex of raw 16 bytes
//! - `CDNMedia.aes_key`: base64(ASCII hex string) — not base64(raw bytes)
//! Inbound `parse_aes_key` accepts both base64(raw16) and base64(hex32).

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use base64::Engine;
use rand::RngCore;
use reqwest::Client;

use crate::vocab::{CarrierError, CarrierResult};

use super::models::CDN_BASE_URL;

type Aes128 = aes::Aes128;

/// Predict AES-128-ECB ciphertext size with PKCS7 padding.
pub fn aes_ecb_padded_size(plaintext_size: usize) -> usize {
    ((plaintext_size + 16) / 16) * 16
}

fn pkcs7_pad(data: &[u8]) -> Vec<u8> {
    let pad = 16 - (data.len() % 16);
    let mut out = Vec::with_capacity(data.len() + pad);
    out.extend_from_slice(data);
    out.extend(std::iter::repeat(pad as u8).take(pad));
    out
}

fn pkcs7_unpad(data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let pad = *data.last()? as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return None;
    }
    if !data[data.len() - pad..].iter().all(|&b| b as usize == pad) {
        return None;
    }
    Some(data[..data.len() - pad].to_vec())
}

/// AES-128-ECB encrypt with PKCS7 padding (outbound CDN upload).
pub fn aes_128_ecb_encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut buf = pkcs7_pad(plaintext);
    for chunk in buf.chunks_mut(16) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }
    buf
}

/// AES-128-ECB decrypt. Prefers PKCS7 unpad; falls back to trailing-zero trim
/// for legacy inbound blobs that were not PKCS7-padded.
pub fn aes_128_ecb_decrypt(ciphertext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut buf = ciphertext.to_vec();
    if buf.len() % 16 != 0 {
        // Not a full block stream — return as-is after best-effort decrypt of
        // leading full blocks (should not happen on CDN).
        let full = (buf.len() / 16) * 16;
        for chunk in buf[..full].chunks_mut(16) {
            let block = GenericArray::from_mut_slice(chunk);
            cipher.decrypt_block(block);
        }
        return buf;
    }
    for chunk in buf.chunks_mut(16) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.decrypt_block(block);
    }
    if let Some(plain) = pkcs7_unpad(&buf) {
        return plain;
    }
    // Legacy zero-trim fallback (pre-PKCS7 inbound).
    let end = buf
        .iter()
        .rposition(|&b| b != 0)
        .map(|i| i + 1)
        .unwrap_or(0);
    buf.truncate(end);
    buf
}

/// Generate a random 16-byte AES-128 key.
pub fn generate_aes_key() -> [u8; 16] {
    let mut key = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Hex encode a raw AES key (for `getuploadurl.aeskey` and top-level `aeskey`).
pub fn aes_key_hex(key: &[u8; 16]) -> String {
    hex::encode(key)
}

/// Dual-encode AES key for `CDNMedia.aes_key`: base64(ASCII hex string).
///
/// Matches openclaw-weixin `Buffer.from(hex).toString("base64")` and
/// qwen-code outbound image send (not base64 of raw 16 bytes).
pub fn aes_key_to_media_b64(key: &[u8; 16]) -> String {
    let hex_s = hex::encode(key);
    base64::engine::general_purpose::STANDARD.encode(hex_s.as_bytes())
}

/// Parse AES key from CDNMedia.aes_key field.
///
/// The key can be:
/// - Raw 16 bytes: base64_decode(aes_key) = 16 raw bytes
/// - Hex-encoded: base64_decode(aes_key) = 32 ASCII hex chars → 16 bytes
pub fn parse_aes_key(aes_key_b64: &str) -> Option<[u8; 16]> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(aes_key_b64)
        .ok()?;

    match decoded.len() {
        16 => {
            let mut key = [0u8; 16];
            key.copy_from_slice(&decoded);
            Some(key)
        }
        32 => {
            let s = std::str::from_utf8(&decoded).ok()?;
            if s.chars().all(|c| c.is_ascii_hexdigit()) {
                let mut key = [0u8; 16];
                hex::decode_to_slice(s, &mut key).ok()?;
                Some(key)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Download and decrypt a file from CDN.
pub async fn cdn_download(http: &Client, url: &str, key: &[u8; 16]) -> CarrierResult<Vec<u8>> {
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("CDN download failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CarrierError::Network(format!(
            "CDN download HTTP {status}: {body}"
        )));
    }

    let ciphertext = resp
        .bytes()
        .await
        .map_err(|e| CarrierError::Network(format!("CDN download read error: {e}")))?;

    Ok(aes_128_ecb_decrypt(&ciphertext, key))
}

/// Build CDN download URL from encrypt_query_param.
pub fn cdn_download_url(encrypt_query_param: &str) -> String {
    format!(
        "{}/download?encrypted_query_param={}",
        CDN_BASE_URL,
        urlencoding::encode(encrypt_query_param)
    )
}

/// Build CDN upload URL from upload_param + filekey (fallback when
/// getuploadurl does not return `upload_full_url`).
pub fn cdn_upload_url(upload_param: &str, filekey: &str) -> String {
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        CDN_BASE_URL,
        urlencoding::encode(upload_param),
        urlencoding::encode(filekey)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_size_matches_pkcs7() {
        assert_eq!(aes_ecb_padded_size(0), 16);
        assert_eq!(aes_ecb_padded_size(1), 16);
        assert_eq!(aes_ecb_padded_size(15), 16);
        assert_eq!(aes_ecb_padded_size(16), 32);
        assert_eq!(aes_ecb_padded_size(17), 32);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = generate_aes_key();
        for len in [0usize, 1, 15, 16, 17, 64, 1000] {
            let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let ct = aes_128_ecb_encrypt(&plain, &key);
            assert_eq!(ct.len(), aes_ecb_padded_size(len));
            let back = aes_128_ecb_decrypt(&ct, &key);
            assert_eq!(back, plain, "len={len}");
        }
    }

    #[test]
    fn media_b64_is_base64_of_hex_ascii() {
        let key = [0u8; 16];
        // hex = 32 zeros; base64 of that ASCII
        let b64 = aes_key_to_media_b64(&key);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        assert_eq!(decoded, b"00000000000000000000000000000000");
        assert_eq!(parse_aes_key(&b64), Some(key));
    }

    #[test]
    fn parse_raw16_b64() {
        let key = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let b64 = base64::engine::general_purpose::STANDARD.encode(key);
        assert_eq!(parse_aes_key(&b64), Some(key));
    }
}
