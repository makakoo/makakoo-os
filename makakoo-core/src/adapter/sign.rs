//! Ed25519 signing primitives.
//!
//! Manifest signatures live in `adapter.toml.sig` next to the manifest.
//! Publisher keys live at `~/.makakoo/trust/keys/<publisher>.pub`.
//!
//! Public keys are 32-byte raw Ed25519 bytes encoded as base64 in the
//! `.pub` file — same shape Sigstore / SSH-Ed25519 consumers already use.
//! Sigstore-style PEM is deferred to v0.4.
//!
//! Signatures are 64-byte Ed25519 blobs encoded as base64 in
//! `adapter.toml.sig`.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use thiserror::Error;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Error)]
pub enum SignError {
    #[error("publisher key not found for `{publisher}` at {path}")]
    KeyNotFound { publisher: String, path: PathBuf },
    #[error("failed to read key {path}: {source}")]
    KeyRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("key file {path} is not valid base64: {source}")]
    KeyDecode {
        path: PathBuf,
        #[source]
        source: base64::DecodeError,
    },
    #[error("key at {path} is not a 32-byte Ed25519 public key (got {got} bytes)")]
    KeyLen { path: PathBuf, got: usize },
    #[error("sig file {path} is not valid base64: {source}")]
    SigDecode {
        path: PathBuf,
        #[source]
        source: base64::DecodeError,
    },
    #[error("signature at {path} is not 64 bytes (got {got})")]
    SigLen { path: PathBuf, got: usize },
    #[error("failed to read sig file {path}: {source}")]
    SigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("signature does not verify against publisher `{publisher}`")]
    VerifyFailed { publisher: String },
}

/// Root of the trust directory. `~/.makakoo/trust/` by default.
pub fn default_trust_root() -> PathBuf {
    if let Ok(p) = std::env::var("MAKAKOO_TRUST_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".makakoo")
        .join("trust")
}

pub fn keys_dir() -> PathBuf {
    default_trust_root().join("keys")
}

pub fn key_path(publisher: &str) -> PathBuf {
    keys_dir().join(format!("{publisher}.pub"))
}

/// Load an Ed25519 verifying key by publisher id from `keys_dir()`.
pub fn load_publisher_key(publisher: &str) -> Result<VerifyingKey, SignError> {
    load_publisher_key_at(&key_path(publisher), publisher)
}

/// Explicit-path variant, used by tests that need an ephemeral key dir.
pub fn load_publisher_key_at(path: &Path, publisher: &str) -> Result<VerifyingKey, SignError> {
    if !path.is_file() {
        return Err(SignError::KeyNotFound {
            publisher: publisher.to_string(),
            path: path.to_path_buf(),
        });
    }
    let body = fs::read_to_string(path).map_err(|e| SignError::KeyRead {
        path: path.to_path_buf(),
        source: e,
    })?;
    let decoded = B64.decode(body.trim()).map_err(|e| SignError::KeyDecode {
        path: path.to_path_buf(),
        source: e,
    })?;
    if decoded.len() != 32 {
        return Err(SignError::KeyLen {
            path: path.to_path_buf(),
            got: decoded.len(),
        });
    }
    let arr: [u8; 32] = decoded.as_slice().try_into().unwrap();
    VerifyingKey::from_bytes(&arr).map_err(|_| SignError::KeyLen {
        path: path.to_path_buf(),
        got: decoded.len(),
    })
}

/// Read a 64-byte Ed25519 signature from a base64-encoded file.
pub fn load_signature(path: &Path) -> Result<Signature, SignError> {
    let body = fs::read_to_string(path).map_err(|e| SignError::SigRead {
        path: path.to_path_buf(),
        source: e,
    })?;
    let decoded = B64.decode(body.trim()).map_err(|e| SignError::SigDecode {
        path: path.to_path_buf(),
        source: e,
    })?;
    if decoded.len() != 64 {
        return Err(SignError::SigLen {
            path: path.to_path_buf(),
            got: decoded.len(),
        });
    }
    let bytes: [u8; 64] = decoded.as_slice().try_into().unwrap();
    Ok(Signature::from_bytes(&bytes))
}

/// Verify a manifest body against a publisher's key + their detached sig
/// file. Returns Ok(()) on success.
pub fn verify_manifest_bytes(
    publisher: &str,
    manifest_bytes: &[u8],
    sig_path: &Path,
    keys_dir_override: Option<&Path>,
) -> Result<(), SignError> {
    let key = match keys_dir_override {
        Some(d) => load_publisher_key_at(&d.join(format!("{publisher}.pub")), publisher)?,
        None => load_publisher_key(publisher)?,
    };
    let sig = load_signature(sig_path)?;
    key.verify(manifest_bytes, &sig).map_err(|_| SignError::VerifyFailed {
        publisher: publisher.to_string(),
    })
}

/// Test-only helper: sign a blob with an ephemeral Ed25519 key and write
/// out both the public-key file and the detached sig file. Returns
/// (publisher, dir) so callers can plug them into `verify_manifest_bytes`.
#[cfg(test)]
pub(crate) fn testing_sign_manifest(
    dir: &Path,
    publisher: &str,
    manifest_bytes: &[u8],
) -> std::path::PathBuf {
    use ed25519_dalek::{Signer, SigningKey};
    use rand::RngCore;

    // Build a random 32-byte secret without needing ed25519-dalek's
    // rand_core feature — side-steps the seedable-rng mismatch.
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let signing = SigningKey::from_bytes(&secret);
    let verifying = signing.verifying_key();

    // public key → keys_dir/<publisher>.pub
    let keys_dir = dir.join("keys");
    std::fs::create_dir_all(&keys_dir).unwrap();
    std::fs::write(
        keys_dir.join(format!("{publisher}.pub")),
        B64.encode(verifying.to_bytes()),
    )
    .unwrap();

    // signature → <dir>/adapter.toml.sig
    let sig = signing.sign(manifest_bytes);
    let sig_path = dir.join("adapter.toml.sig");
    std::fs::write(&sig_path, B64.encode(sig.to_bytes())).unwrap();
    sig_path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_sign_and_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = b"[adapter]\nname = \"openclaw\"\n";
        let sig_path = testing_sign_manifest(tmp.path(), "traylinx", manifest);
        verify_manifest_bytes("traylinx", manifest, &sig_path, Some(&tmp.path().join("keys"))).unwrap();
    }

    #[test]
    fn tampered_manifest_fails_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = b"[adapter]\nname = \"openclaw\"\n";
        let sig_path = testing_sign_manifest(tmp.path(), "traylinx", manifest);
        let tampered = b"[adapter]\nname = \"openclawjr\"\n";
        let err = verify_manifest_bytes(
            "traylinx",
            tampered,
            &sig_path,
            Some(&tmp.path().join("keys")),
        )
        .unwrap_err();
        assert!(matches!(err, SignError::VerifyFailed { .. }));
    }

    #[test]
    fn missing_publisher_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let sig = tmp.path().join("adapter.toml.sig");
        std::fs::write(&sig, "AAAA").unwrap(); // anything
        let err = verify_manifest_bytes(
            "unknown-publisher",
            b"manifest",
            &sig,
            Some(&tmp.path().join("keys")),
        )
        .unwrap_err();
        assert!(matches!(err, SignError::KeyNotFound { .. }));
    }

    #[test]
    fn malformed_key_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let keys = tmp.path().join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        std::fs::write(keys.join("bad.pub"), "not-valid-base64-at-all!!!").unwrap();
        let sig = tmp.path().join("adapter.toml.sig");
        std::fs::write(&sig, "AAAA").unwrap();
        let err = verify_manifest_bytes("bad", b"m", &sig, Some(&keys)).unwrap_err();
        assert!(matches!(err, SignError::KeyDecode { .. }));
    }
}
