//! Ed25519-based manifest signing for supply chain integrity.
//!
//! Agent manifests are TOML files that define an agent's capabilities,
//! tools, and configuration. A compromised or tampered manifest can grant
//! an agent elevated privileges. This module allows manifests to be
//! cryptographically signed so that the kernel can verify their integrity
//! and provenance before loading.
//!
//! The signing scheme:
//! 1. Compute SHA-256 of the manifest content.
//! 2. Sign the hash with Ed25519 (via `ed25519-dalek`).
//! 3. Bundle the signature, public key, and content hash into a
//!    `SignedManifest` envelope.
//!
//! Verification recomputes the hash and checks the Ed25519 signature
//! against the embedded public key.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CarrierError, CarrierResult};

/// A signed manifest envelope containing the original manifest text,
/// its content hash, the Ed25519 signature, and the signer's public key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    /// The raw manifest content (typically TOML).
    pub manifest: String,
    /// Hex-encoded SHA-256 hash of `manifest`.
    pub content_hash: String,
    /// Ed25519 signature bytes over `content_hash`.
    pub signature: Vec<u8>,
    /// The signer's Ed25519 public key bytes (32 bytes).
    pub signer_public_key: Vec<u8>,
    /// Human-readable identifier for the signer (e.g. email or key ID).
    pub signer_id: String,
}

/// Computes the hex-encoded SHA-256 hash of a manifest string.
pub fn hash_manifest(manifest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(manifest.as_bytes());
    hex::encode(hasher.finalize())
}

impl SignedManifest {
    /// Signs a manifest with the given Ed25519 signing key.
    ///
    /// Returns a `SignedManifest` envelope ready for serialisation and
    /// distribution alongside (or instead of) the raw manifest file.
    pub fn sign(
        manifest: impl Into<String>,
        signing_key: &SigningKey,
        signer_id: impl Into<String>,
    ) -> Self {
        let manifest = manifest.into();
        let content_hash = hash_manifest(&manifest);
        let signature = signing_key.sign(content_hash.as_bytes());
        let verifying_key = signing_key.verifying_key();

        Self {
            manifest,
            content_hash,
            signature: signature.to_bytes().to_vec(),
            signer_public_key: verifying_key.to_bytes().to_vec(),
            signer_id: signer_id.into(),
        }
    }

    /// Verifies the manifest signature against a specific public key.
    ///
    /// This is the same verification logic as `verify`, but accepts an
    /// explicit `VerifyingKey` instead of using the embedded one.
    pub fn verify_with_key(&self, key: &VerifyingKey) -> CarrierResult<()> {
        // Re-compute the hash and compare.
        let recomputed = hash_manifest(&self.manifest);
        if recomputed != self.content_hash {
            return Err(CarrierError::ManifestParse(format!(
                "content hash mismatch: expected {} but manifest hashes to {}",
                self.content_hash, recomputed
            )));
        }

        // Reconstruct the signature.
        let sig_bytes: [u8; 64] = self.signature.as_slice().try_into().map_err(|_| {
            CarrierError::ManifestParse("invalid signature length (expected 64 bytes)".to_string())
        })?;
        let signature = Signature::from_bytes(&sig_bytes);

        // Verify against the provided key.
        key.verify(self.content_hash.as_bytes(), &signature)
            .map_err(|e| {
                CarrierError::ManifestParse(format!("signature verification failed: {}", e))
            })
    }

    /// Verify the manifest signature against a set of trusted public keys.
    /// Returns Ok(()) if any trusted key validates the signature.
    pub fn verify_with_trust_store(
        &self,
        trusted_keys: &[ed25519_dalek::VerifyingKey],
    ) -> CarrierResult<()> {
        let mut last_err: Option<CarrierError> = None;
        for key in trusted_keys {
            match self.verify_with_key(key) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| CarrierError::Config("No trusted keys provided".to_string())))
    }

    /// Verifies the integrity and authenticity of this signed manifest.
    ///
    /// **Warning**: This method trusts the embedded `signer_public_key` without
    /// checking it against a known key store. A malicious actor can generate
    /// a valid signature with their own key pair. Use `verify_with_trust_store`
    /// with a set of known trusted keys for production verification.
    ///
    /// Checks:
    /// 1. The `content_hash` matches a fresh SHA-256 of `manifest`.
    /// 2. The `signature` is valid for `content_hash` under `signer_public_key`.
    ///
    /// Returns `Ok(())` on success, or `Err(CarrierError)` on failure.
    pub fn verify(&self) -> CarrierResult<()> {
        // Re-compute the hash and compare.
        let recomputed = hash_manifest(&self.manifest);
        if recomputed != self.content_hash {
            return Err(CarrierError::ManifestParse(format!(
                "content hash mismatch: expected {} but manifest hashes to {}",
                self.content_hash, recomputed
            )));
        }

        // Reconstruct the public key.
        let pk_bytes: [u8; 32] = self.signer_public_key.as_slice().try_into().map_err(|_| {
            CarrierError::ManifestParse("invalid public key length (expected 32 bytes)".to_string())
        })?;
        let verifying_key = VerifyingKey::from_bytes(&pk_bytes)
            .map_err(|e| CarrierError::ManifestParse(format!("invalid public key: {}", e)))?;

        // Reconstruct the signature.
        let sig_bytes: [u8; 64] = self.signature.as_slice().try_into().map_err(|_| {
            CarrierError::ManifestParse("invalid signature length (expected 64 bytes)".to_string())
        })?;
        let signature = Signature::from_bytes(&sig_bytes);

        // Verify.
        verifying_key
            .verify(self.content_hash.as_bytes(), &signature)
            .map_err(|e| {
                CarrierError::ManifestParse(format!("signature verification failed: {}", e))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_sign_and_verify() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let manifest = r#"
[agent]
name = "hello-world"
description = "A simple test agent"

[capabilities]
shell = false
network = false
"#;

        let signed = SignedManifest::sign(manifest, &signing_key, "test@carrier.dev");
        assert_eq!(signed.content_hash, hash_manifest(manifest));
        assert_eq!(signed.signer_id, "test@carrier.dev");
        assert!(signed.verify().is_ok());
    }

    #[test]
    fn test_tampered_fails() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let manifest = "[agent]\nname = \"secure-agent\"\n";

        let mut signed = SignedManifest::sign(manifest, &signing_key, "signer-1");

        // Tamper with the manifest content after signing.
        signed.manifest = "[agent]\nname = \"evil-agent\"\nshell = true\n".to_string();

        let result = signed.verify();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("content hash mismatch"));
    }

    #[test]
    fn test_wrong_key_fails() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng);

        let manifest = "[agent]\nname = \"test\"\n";
        let mut signed = SignedManifest::sign(manifest, &signing_key, "signer-a");

        // Replace the public key with a different key's public key.
        signed.signer_public_key = wrong_key.verifying_key().to_bytes().to_vec();

        let result = signed.verify();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signature verification failed"));
    }
}
