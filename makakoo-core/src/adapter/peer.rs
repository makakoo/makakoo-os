//! Peer-to-peer Ed25519 identity + signed-request primitives.
//!
//! Distinct from `sign.rs` (which verifies publisher-signed manifest
//! detached sigs). `peer.rs` is about two Makakoo installs trusting
//! each other at RPC time: each peer has a signing key; each peer
//! carries a trust file naming the peers it accepts requests from.
//!
//! The wire format is the minimum to prevent replay + tamper:
//!
//!   Client sends:
//!     POST /rpc
//!     X-Makakoo-Peer: <peer-name>         — selector for trust file
//!     X-Makakoo-Ts:   <unix-millis>       — request timestamp
//!     X-Makakoo-Sig:  ed25519=<base64>    — Ed25519 over sha256(body || ts)
//!
//!   Server verifies:
//!     1. Lookup peer pubkey in trust file → 401 if unknown.
//!     2. |now - ts| < DRIFT_WINDOW (60s) → 401 if drift.
//!     3. Verify signature over sha256(body || ts_bytes) → 401 if bad.
//!
//! If all three pass the request body is handed to the MCP server as if
//! it arrived over stdio.
//!
//! **No TLS here.** The user brings the transport (Tailscale SSH-style,
//! Cloudflare Tunnel, reverse-proxy TLS). See v0.6 SPRINT.md §9 Q4.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use thiserror::Error;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Signed requests older than this window are rejected. Matches clock-
/// drift tolerance the reference MCP SDKs use.
pub const DRIFT_WINDOW_MS: i64 = 60_000;

/// Canonical header names. Extracted for test reuse.
pub const PEER_HEADER: &str = "X-Makakoo-Peer";
pub const TS_HEADER: &str = "X-Makakoo-Ts";
pub const SIG_HEADER: &str = "X-Makakoo-Sig";
/// Optional header carrying the originating subagent slot id.
/// Set by the per-slot Python gateway on every outbound MCP call;
/// the MCP server forwards it to tool handlers via the
/// `harvey_agent_id` ContextVar so tools (grants, brain writes,
/// audit logs) can attribute the call to the right agent.
/// Phase 3 locked.
pub const AGENT_ID_HEADER: &str = "X-Makakoo-Agent-Id";

/// Prefix marking the signature scheme. Future schemes (ed448, etc.)
/// would extend the enum.
pub const SIG_PREFIX: &str = "ed25519=";

#[derive(Debug, Error)]
pub enum PeerError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("trust file line {line} malformed in {path}")]
    TrustLine { path: PathBuf, line: usize },
    #[error("unknown peer `{0}` (add via `makakoo adapter trust add`)")]
    UnknownPeer(String),
    #[error("invalid base64 in {0}: {1}")]
    Base64(&'static str, base64::DecodeError),
    #[error("key must decode to 32 bytes (got {0})")]
    BadKeyLen(usize),
    #[error("signature must decode to 64 bytes (got {0})")]
    BadSigLen(usize),
    #[error("missing signature scheme prefix `ed25519=`")]
    MissingPrefix,
    #[error("timestamp parse failed: {0}")]
    BadTimestamp(String),
    #[error("clock drift {drift}ms exceeds window ±{window}ms")]
    ClockDrift { drift: i64, window: i64 },
    #[error("signature verification failed")]
    VerifyFailed,
}

/// Default on-disk locations rooted at `$MAKAKOO_HOME` — callers may
/// override for tests.
pub fn default_peers_dir(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("config").join("peers")
}

pub fn default_trust_file(makakoo_home: &Path) -> PathBuf {
    default_peers_dir(makakoo_home).join("trusted.keys")
}

pub fn default_signing_key_path(makakoo_home: &Path) -> PathBuf {
    default_peers_dir(makakoo_home).join("signing.key")
}

pub fn default_signing_pub_path(makakoo_home: &Path) -> PathBuf {
    default_peers_dir(makakoo_home).join("signing.pub")
}

/// Parse a trust file. Skips blank lines + `#` comments. Lines are
/// `<name> <base64-pubkey>`. Missing file → empty map (caller decides
/// whether to treat as "deny all" or auto-create).
pub fn load_trust_file(path: &Path) -> Result<HashMap<String, VerifyingKey>, PeerError> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let body = fs::read_to_string(path).map_err(|source| PeerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut out = HashMap::new();
    for (i, raw) in body.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.splitn(2, char::is_whitespace);
        let (name, key_b64) = match (it.next(), it.next()) {
            (Some(n), Some(k)) if !n.is_empty() && !k.is_empty() => (n, k.trim()),
            _ => {
                return Err(PeerError::TrustLine {
                    path: path.to_path_buf(),
                    line: i + 1,
                });
            }
        };
        let raw = B64.decode(key_b64).map_err(|e| PeerError::Base64("pubkey", e))?;
        let key = pubkey_from_bytes(&raw)?;
        out.insert(name.to_string(), key);
    }
    Ok(out)
}

/// Ensure a signing keypair exists on disk. Returns `(signing_key,
/// verifying_key, was_generated)` — callers surface the latter so
/// first-run can print a "share this pubkey" notice.
pub fn load_or_create_signing_key(
    key_path: &Path,
    pub_path: &Path,
) -> Result<(SigningKey, VerifyingKey, bool), PeerError> {
    if key_path.exists() {
        let body = fs::read_to_string(key_path).map_err(|source| PeerError::Io {
            path: key_path.to_path_buf(),
            source,
        })?;
        let raw = B64
            .decode(body.trim())
            .map_err(|e| PeerError::Base64("signing key", e))?;
        if raw.len() != 32 {
            return Err(PeerError::BadKeyLen(raw.len()));
        }
        let arr: [u8; 32] = raw.as_slice().try_into().unwrap();
        let signing = SigningKey::from_bytes(&arr);
        let verifying = signing.verifying_key();
        return Ok((signing, verifying, false));
    }

    // Generate + persist.
    use rand::RngCore;
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let signing = SigningKey::from_bytes(&secret);
    let verifying = signing.verifying_key();

    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent).map_err(|source| PeerError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(key_path, B64.encode(signing.to_bytes())).map_err(|source| PeerError::Io {
        path: key_path.to_path_buf(),
        source,
    })?;
    fs::write(pub_path, B64.encode(verifying.to_bytes())).map_err(|source| PeerError::Io {
        path: pub_path.to_path_buf(),
        source,
    })?;
    // On Unix, chmod 0600 the private key — best-effort.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(key_path, fs::Permissions::from_mode(0o600));
    }
    Ok((signing, verifying, true))
}

/// Current unix time in milliseconds. Split out so tests can pin it.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Compute the canonical bytes a signature covers: `sha256(body || ts_decimal_ascii)`.
fn canonical_digest(body: &[u8], ts: i64) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(body);
    h.update(ts.to_string().as_bytes());
    h.finalize().into()
}

/// Sign a request body + timestamp. Returns the base64 signature (without
/// the `ed25519=` prefix — caller composes the final header value).
pub fn sign_request(signing: &SigningKey, body: &[u8], ts: i64) -> String {
    let digest = canonical_digest(body, ts);
    let sig: Signature = signing.sign(&digest);
    B64.encode(sig.to_bytes())
}

/// Verify a signed request. Returns Ok on success.
pub fn verify_request(
    trust: &HashMap<String, VerifyingKey>,
    peer: &str,
    body: &[u8],
    ts: i64,
    sig_header: &str,
    now: i64,
) -> Result<(), PeerError> {
    let drift = (now - ts).abs();
    if drift > DRIFT_WINDOW_MS {
        return Err(PeerError::ClockDrift {
            drift,
            window: DRIFT_WINDOW_MS,
        });
    }
    let Some(key) = trust.get(peer) else {
        return Err(PeerError::UnknownPeer(peer.to_string()));
    };
    let stripped = sig_header
        .strip_prefix(SIG_PREFIX)
        .ok_or(PeerError::MissingPrefix)?;
    let raw = B64
        .decode(stripped)
        .map_err(|e| PeerError::Base64("signature", e))?;
    if raw.len() != 64 {
        return Err(PeerError::BadSigLen(raw.len()));
    }
    let arr: [u8; 64] = raw.as_slice().try_into().unwrap();
    let sig = Signature::from_bytes(&arr);
    let digest = canonical_digest(body, ts);
    key.verify(&digest, &sig).map_err(|_| PeerError::VerifyFailed)
}

fn pubkey_from_bytes(raw: &[u8]) -> Result<VerifyingKey, PeerError> {
    if raw.len() != 32 {
        return Err(PeerError::BadKeyLen(raw.len()));
    }
    let arr: [u8; 32] = raw.try_into().unwrap();
    VerifyingKey::from_bytes(&arr).map_err(|_| PeerError::BadKeyLen(32))
}

/// Write a peer record to a trust file, atomically. Replaces an existing
/// entry with the same name. Comments and other entries are preserved.
pub fn trust_add(path: &Path, name: &str, pubkey_b64: &str) -> Result<(), PeerError> {
    // Validate before writing.
    let raw = B64.decode(pubkey_b64).map_err(|e| PeerError::Base64("pubkey", e))?;
    pubkey_from_bytes(&raw)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| PeerError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let existing = if path.exists() {
        fs::read_to_string(path).map_err(|source| PeerError::Io {
            path: path.to_path_buf(),
            source,
        })?
    } else {
        String::new()
    };
    let mut out = String::new();
    let mut replaced = false;
    for line in existing.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let first = t.split_whitespace().next().unwrap_or("");
        if first == name {
            out.push_str(&format!("{name} {pubkey_b64}\n"));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("{name} {pubkey_b64}\n"));
    }
    write_atomic(path, &out)?;
    Ok(())
}

/// Remove a peer record by name. Returns `true` if a line was removed.
pub fn trust_remove(path: &Path, name: &str) -> Result<bool, PeerError> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = fs::read_to_string(path).map_err(|source| PeerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut out = String::new();
    let mut removed = false;
    for line in existing.lines() {
        let t = line.trim();
        if !t.is_empty() && !t.starts_with('#') {
            let first = t.split_whitespace().next().unwrap_or("");
            if first == name {
                removed = true;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if removed {
        write_atomic(path, &out)?;
    }
    Ok(removed)
}

fn write_atomic(path: &Path, body: &str) -> Result<(), PeerError> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body).map_err(|source| PeerError::Io {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, path).map_err(|source| PeerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Pubkey fingerprint suitable for UI display — first 16 hex chars of
/// sha256(pubkey). Matches the shape `sign.rs::keys_dir` consumers use
/// for manifest signatures.
pub fn fingerprint(verifying: &VerifyingKey) -> String {
    let mut h = Sha256::new();
    h.update(verifying.to_bytes());
    let out = h.finalize();
    hex_short(&out, 16)
}

fn hex_short(bytes: &[u8], chars: usize) -> String {
    let mut s = String::with_capacity(chars);
    for b in bytes {
        if s.len() >= chars {
            break;
        }
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(chars);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn key_generation_is_deterministic_on_reload() {
        let tmp = TempDir::new().unwrap();
        let key = tmp.path().join("signing.key");
        let pub_ = tmp.path().join("signing.pub");
        let (_s1, v1, gen1) = load_or_create_signing_key(&key, &pub_).unwrap();
        assert!(gen1);
        let (_s2, v2, gen2) = load_or_create_signing_key(&key, &pub_).unwrap();
        assert!(!gen2);
        assert_eq!(v1.to_bytes(), v2.to_bytes());
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let tmp = TempDir::new().unwrap();
        let key = tmp.path().join("signing.key");
        let pub_ = tmp.path().join("signing.pub");
        let (signing, verifying, _) = load_or_create_signing_key(&key, &pub_).unwrap();

        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let ts = 1_700_000_000_000;
        let sig = sign_request(&signing, body, ts);

        let mut trust = HashMap::new();
        trust.insert("peer-a".to_string(), verifying);
        verify_request(
            &trust,
            "peer-a",
            body,
            ts,
            &format!("{SIG_PREFIX}{sig}"),
            ts,
        )
        .unwrap();
    }

    #[test]
    fn tampered_body_fails_verify() {
        let tmp = TempDir::new().unwrap();
        let (signing, verifying, _) = load_or_create_signing_key(
            &tmp.path().join("k"),
            &tmp.path().join("p"),
        )
        .unwrap();
        let ts = 1_700_000_000_000;
        let sig = sign_request(&signing, b"original body", ts);
        let mut trust = HashMap::new();
        trust.insert("x".to_string(), verifying);
        let err = verify_request(
            &trust,
            "x",
            b"tampered body",
            ts,
            &format!("{SIG_PREFIX}{sig}"),
            ts,
        )
        .unwrap_err();
        assert!(matches!(err, PeerError::VerifyFailed));
    }

    #[test]
    fn drift_beyond_window_rejects() {
        let tmp = TempDir::new().unwrap();
        let (signing, verifying, _) =
            load_or_create_signing_key(&tmp.path().join("k"), &tmp.path().join("p")).unwrap();
        let ts = 1_700_000_000_000;
        let sig = sign_request(&signing, b"body", ts);
        let mut trust = HashMap::new();
        trust.insert("x".to_string(), verifying);
        let err = verify_request(
            &trust,
            "x",
            b"body",
            ts,
            &format!("{SIG_PREFIX}{sig}"),
            ts + DRIFT_WINDOW_MS + 1,
        )
        .unwrap_err();
        assert!(matches!(err, PeerError::ClockDrift { .. }));
    }

    #[test]
    fn unknown_peer_rejects() {
        let err = verify_request(
            &HashMap::new(),
            "nobody",
            b"body",
            0,
            &format!("{SIG_PREFIX}AAAA"),
            0,
        )
        .unwrap_err();
        assert!(matches!(err, PeerError::UnknownPeer(_)));
    }

    #[test]
    fn missing_prefix_rejects() {
        let tmp = TempDir::new().unwrap();
        let (_, verifying, _) =
            load_or_create_signing_key(&tmp.path().join("k"), &tmp.path().join("p")).unwrap();
        let mut trust = HashMap::new();
        trust.insert("x".to_string(), verifying);
        let err = verify_request(&trust, "x", b"body", 0, "AAAA", 0).unwrap_err();
        assert!(matches!(err, PeerError::MissingPrefix));
    }

    #[test]
    fn trust_file_add_list_remove_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted.keys");
        // Generate 2 pubkeys.
        use rand::RngCore;
        let mk = || {
            let mut s = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut s);
            B64.encode(SigningKey::from_bytes(&s).verifying_key().to_bytes())
        };
        let k1 = mk();
        let k2 = mk();

        trust_add(&path, "alice", &k1).unwrap();
        trust_add(&path, "bob", &k2).unwrap();
        let loaded = load_trust_file(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains_key("alice"));
        assert!(loaded.contains_key("bob"));

        // Replace alice's key — should preserve bob, overwrite alice.
        let k1b = mk();
        trust_add(&path, "alice", &k1b).unwrap();
        let loaded = load_trust_file(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            B64.encode(loaded["alice"].to_bytes()),
            k1b,
            "alice's key should have been replaced"
        );

        // Remove.
        assert!(trust_remove(&path, "bob").unwrap());
        let loaded = load_trust_file(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(!loaded.contains_key("bob"));

        // Removing a non-existent entry returns false, doesn't error.
        assert!(!trust_remove(&path, "nonexistent").unwrap());
    }

    #[test]
    fn trust_file_skips_blank_lines_and_comments() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted.keys");
        let (_, v, _) =
            load_or_create_signing_key(&tmp.path().join("k"), &tmp.path().join("p")).unwrap();
        let body = format!(
            "# header comment\n\n# another\nalice {}\n\n",
            B64.encode(v.to_bytes())
        );
        fs::write(&path, body).unwrap();
        let loaded = load_trust_file(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("alice"));
    }

    #[test]
    fn trust_file_malformed_line_errors_with_line_number() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("trusted.keys");
        fs::write(&path, "ok-so-far aGVsbG8=aGVsbG8=aGVsbG8=aGVsbG8=aGVsbG8=AAA=\nbad-line-no-key\n")
            .unwrap();
        // First line is malformed base64; load fails with Base64 error
        // before it reaches line 2 — that's fine, both are errors.
        let err = load_trust_file(&path).unwrap_err();
        assert!(
            matches!(err, PeerError::Base64(_, _) | PeerError::BadKeyLen(_)),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn fingerprint_is_stable_and_short() {
        let tmp = TempDir::new().unwrap();
        let (_, v, _) =
            load_or_create_signing_key(&tmp.path().join("k"), &tmp.path().join("p")).unwrap();
        let f1 = fingerprint(&v);
        let f2 = fingerprint(&v);
        assert_eq!(f1, f2);
        assert_eq!(f1.len(), 16);
    }
}
