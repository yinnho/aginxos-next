//! aginx-sign lib — ed25519 验签面（M26 从 agupd 内联抽出，一把钥匙一条链；
//! N4② 搬入新仓改姓）。
//!
//! 契约（M21 起不变）：detached 签名 = base64(64 字节 ed25519 签名) 落
//! `<file>.sig`，签的是文件 RAW 字节——没有 JSON 规范化。验签永远发生在
//! 任何解析/下载/写入之前。公钥编译进二进制；私钥是本地机密（.local/
//! keys/aginx.key，gitignored），轮换走"旧钥匙签的更新里嵌新钥匙"（v2 问题）。
//!
//! 消费方：aginx-update（更新 manifest）、aginx-pkg（包 manifest）、
//! aginx-sign CLI（keygen/sign/verify，主机侧）。
//!
//! **公钥挪到这里后，update/pkg 都不再内嵌自己的副本**——升钥匙改一处。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// 更新签名公钥（ed25519，base64）。与 .local/keys/aginx.key 配对；
/// 轮换 = 发一个用它签的更新、更新里带新公钥（一期单钥匙）。
pub const AGINX_PUBKEY_B64: &str = "N/qhN+0s1P0GOJjvpcyQjxAYmwHT00z09p2+6JA5nns=";

/// 用指定公钥验 `body` 对 `sig_b64`。Ok(()) = 有效。
pub fn verify_with_key(pub_b64: &str, body: &[u8], sig_b64: &str) -> Result<(), String> {
    let key_bytes: [u8; 32] = B64
        .decode(pub_b64.trim())
        .map_err(|e| format!("pubkey base64: {e}"))?
        .try_into()
        .map_err(|v: Vec<u8>| format!("pubkey: want 32 bytes, got {}", v.len()))?;
    let vk = VerifyingKey::from_bytes(&key_bytes).map_err(|e| format!("pubkey: {e}"))?;
    let sig_bytes: [u8; 64] = B64
        .decode(sig_b64.trim())
        .map_err(|e| format!("sig base64: {e}"))?
        .try_into()
        .map_err(|v: Vec<u8>| format!("sig: want 64 bytes, got {}", v.len()))?;
    vk.verify(body, &Signature::from_bytes(&sig_bytes))
        .map_err(|e| format!("signature: {e}"))
}

/// 用内置公钥验。Ok(()) = 有效。
pub fn verify(body: &[u8], sig_b64: &str) -> Result<(), String> {
    verify_with_key(AGINX_PUBKEY_B64, body, sig_b64)
}

/// 读 `<path>` 与 `<path>.sig`，用内置公钥验 detached 签名。
/// 文件缺失/坏 base64/验签失败都归一为 Err（调用方只需 fail-closed）。
pub fn verify_detached(path: &std::path::Path) -> Result<(), String> {
    let body = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let sig_path = {
        let mut s = path.as_os_str().to_os_string();
        s.push(".sig");
        std::path::PathBuf::from(s)
    };
    let sig_b64 = std::fs::read_to_string(&sig_path)
        .map_err(|e| format!("read {}: {e} (未签名？)", sig_path.display()))?;
    verify(&body, &sig_b64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand_core::{OsRng, RngCore};

    #[test]
    fn verify_roundtrip_and_reject() {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let sk = SigningKey::from_bytes(&seed);
        let pub_b64 = B64.encode(sk.verifying_key().as_ref());
        let body = b"name url sha256 core\n";
        let sig_b64 = B64.encode(sk.sign(body).to_bytes());
        assert!(verify_with_key(&pub_b64, body, &sig_b64).is_ok());
        // 篡改一处 → 拒
        assert!(verify_with_key(&pub_b64, b"name url deadbeef core\n", &sig_b64).is_err());
        // 换钥匙 → 拒
        let mut seed2 = [0u8; 32];
        OsRng.fill_bytes(&mut seed2);
        let pub2 = B64.encode(SigningKey::from_bytes(&seed2).verifying_key().as_ref());
        assert!(verify_with_key(&pub2, body, &sig_b64).is_err());
    }

    #[test]
    fn builtin_key_is_the_update_key() {
        // 解码必须成立（agupd 同款 32 字节）；真实签名链在 agupd 的设备
        // 验收里覆盖，这里只锁"内置钥匙可解析"。
        assert_eq!(B64.decode(AGINX_PUBKEY_B64).unwrap().len(), 32);
    }
}
