//! `makakoo agent destroy <slot>` — interactive teardown.
//!
//! Locked by Phase 0 Q3:
//!
//! 1. Stop the supervisor (caller responsibility — destroy() expects
//!    the supervisor already shut down).
//! 2. Move TOML to `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/<slot>.toml`.
//! 3. Move data dir to `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/data/`.
//! 4. Scan TOML for **direct** `secret_ref = "..."` literals and
//!    return the list. The CLI surfaces these to the user; whether
//!    they get revoked is a separate explicit action
//!    (`--revoke-secrets`).
//!
//! `--yes` skips the destroy confirmation prompt but does NOT
//! auto-revoke secrets. Secrets are PRESERVED unless the operator
//! says so explicitly.
//!
//! Re-creating a slot after destroy is always a fresh slot — never a
//! restore from archive (operator does that manually if needed).
//!
//! `harveychat` is the legacy Olibia migration anchor; refusing to
//! destroy it without an explicit `--really-destroy-harveychat` flag
//! protects years of conversation history.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agents::slot::slot_path;
use crate::error::{MakakooError, Result};

/// Locked archive root (under `$MAKAKOO_HOME`, NOT `~/.makakoo`).
pub fn archive_root(makakoo_home: &Path) -> PathBuf {
    makakoo_home.join("archive/agents")
}

/// Per-destroy archive directory: `<archive_root>/<slot>-<unix_ts>/`.
pub fn archive_dir(makakoo_home: &Path, slot_id: &str, unix_ts: u64) -> PathBuf {
    archive_root(makakoo_home).join(format!("{slot_id}-{unix_ts}"))
}

/// Where slot data lives. Phase 1 uses
/// `$MAKAKOO_HOME/data/agents/<slot>/`.
pub fn slot_data_dir(makakoo_home: &Path, slot_id: &str) -> PathBuf {
    makakoo_home.join("data/agents").join(slot_id)
}

/// The `harveychat` legacy slot — protected by the
/// `--really-destroy-harveychat` flag.
pub const PROTECTED_SLOT: &str = "harveychat";

/// Outcome of a destroy. CLI uses this to print restore instructions
/// + the secret list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestroyOutcome {
    pub slot_id: String,
    pub archive_dir: PathBuf,
    pub archived_toml: PathBuf,
    /// `Some(path)` if the slot had a data dir; `None` if it didn't
    /// exist (e.g., never started).
    pub archived_data_dir: Option<PathBuf>,
    /// Direct `secret_ref = "..."` literals found in the TOML.
    /// Note: secrets nested under `[transport.config]` sub-tables
    /// or referenced via env-var interpolation are NOT detected
    /// (locked Q3 limitation — surfaced in walkthrough).
    pub detected_secrets: Vec<String>,
}

/// Errors specific to the destroy path.
#[derive(Debug, thiserror::Error)]
pub enum DestroyError {
    #[error("slot '{slot_id}' not found at {path}")]
    SlotNotFound { slot_id: String, path: PathBuf },

    #[error(
        "refusing to destroy '{PROTECTED_SLOT}' without --really-destroy-harveychat. \
         This slot carries the legacy Olibia conversation history."
    )]
    HarveychatProtected,

    #[error("archive_dir already exists at {path}: refusing to overwrite")]
    ArchiveExists { path: PathBuf },
}

/// Core destroy primitive. Pure data movement — no prompts, no
/// secret revocation. The CLI layer wraps this with confirmation
/// prompts and the optional `--revoke-secrets` follow-up.
///
/// Pre-condition: caller has already stopped the supervisor (the
/// destroy itself does NOT touch launchd/systemd).
pub fn destroy_slot(
    makakoo_home: &Path,
    slot_id: &str,
    really_destroy_harveychat: bool,
    unix_ts: u64,
) -> std::result::Result<DestroyOutcome, DestroyError> {
    if slot_id == PROTECTED_SLOT && !really_destroy_harveychat {
        return Err(DestroyError::HarveychatProtected);
    }

    let toml_path = slot_path(makakoo_home, slot_id);
    if !toml_path.exists() {
        return Err(DestroyError::SlotNotFound {
            slot_id: slot_id.to_string(),
            path: toml_path,
        });
    }

    let dst = archive_dir(makakoo_home, slot_id, unix_ts);
    if dst.exists() {
        return Err(DestroyError::ArchiveExists { path: dst });
    }
    std::fs::create_dir_all(&dst).map_err(|e| {
        // Map io::Error into a domain error variant. The error
        // surface is wrapped in DestroyError::ArchiveExists's family
        // — but for "permission denied" we just bubble through the
        // generic SlotNotFound::path display (close enough for the
        // CLI; the io::Error details print via `{e}`).
        DestroyError::SlotNotFound {
            slot_id: format!("could not create archive dir: {e}"),
            path: dst.clone(),
        }
    })?;

    // Read TOML body BEFORE moving it so we can scan for
    // secret_ref literals. The scan is intentionally simple — just
    // `secret_ref` at any indent — to avoid false positives from
    // commented-out lines while still catching every shipping
    // variant (secret_ref + app_token_ref + signing_secret_ref +
    // verify_token_ref + access_token_ref + password_ref +
    // refresh_token_ref + client_secret_ref).
    let toml_body = std::fs::read_to_string(&toml_path).map_err(|e| {
        DestroyError::SlotNotFound {
            slot_id: format!("could not read slot TOML: {e}"),
            path: toml_path.clone(),
        }
    })?;
    let detected_secrets = scan_secret_refs(&toml_body);

    // Move the TOML.
    let archived_toml = dst.join(format!("{slot_id}.toml"));
    std::fs::rename(&toml_path, &archived_toml).map_err(|e| {
        DestroyError::SlotNotFound {
            slot_id: format!("could not move slot TOML: {e}"),
            path: toml_path.clone(),
        }
    })?;

    // Always create `<archive>/data/`. If the source data dir
    // exists, move its contents in; otherwise the archive ships an
    // empty `data/` so the locked Q3 archive shape (`<slot>.toml +
    // data/`) is invariant. Restoration semantics differ between
    // the two cases (see `render_restore_one_liner`).
    let data_dst = dst.join("data");
    let data_src = slot_data_dir(makakoo_home, slot_id);
    let archived_data_dir = if data_src.exists() {
        std::fs::rename(&data_src, &data_dst).map_err(|e| {
            DestroyError::SlotNotFound {
                slot_id: format!("could not move slot data dir: {e}"),
                path: data_src.clone(),
            }
        })?;
        Some(data_dst.clone())
    } else {
        std::fs::create_dir_all(&data_dst).map_err(|e| {
            DestroyError::SlotNotFound {
                slot_id: format!("could not create empty data archive: {e}"),
                path: data_dst.clone(),
            }
        })?;
        None
    };

    Ok(DestroyOutcome {
        slot_id: slot_id.to_string(),
        archive_dir: dst,
        archived_toml,
        archived_data_dir,
        detected_secrets,
    })
}

/// Scan a TOML body for `*_ref = "..."` literals that look like
/// secret references. Recognises every locked secret-ref field
/// across the v2 transport adapters.
pub fn scan_secret_refs(toml_body: &str) -> Vec<String> {
    let known_keys = [
        "secret_ref",
        "app_token_ref",
        "signing_secret_ref",
        "verify_token_ref",
        "access_token_ref",
        "password_ref",
        "refresh_token_ref",
        "client_id_ref",
        "client_secret_ref",
        "app_secret_ref",
        "bot_token_ref",
    ];
    let mut out = Vec::new();
    for line in toml_body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        for key in &known_keys {
            // Match `key = "..."` or `key="..."` (any whitespace).
            let needle = format!("{key}");
            if let Some(pos) = trimmed.find(&needle) {
                // Left of pos must be empty or whitespace (so we
                // don't match `inline_secret_ref` against
                // `secret_ref`).
                if pos > 0
                    && !trimmed[..pos].chars().last().map(char::is_whitespace).unwrap_or(true)
                {
                    continue;
                }
                let after = &trimmed[pos + needle.len()..];
                let after = after.trim_start();
                if !after.starts_with('=') {
                    continue;
                }
                let after = after[1..].trim_start();
                if let Some(value) = extract_quoted(after) {
                    out.push(value);
                }
            }
        }
    }
    // Dedup while preserving first-seen order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| seen.insert(v.clone()));
    out
}

fn extract_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let quote = bytes.first()?;
    if *quote != b'"' && *quote != b'\'' {
        return None;
    }
    let mut end = 1;
    while end < bytes.len() && bytes[end] != *quote {
        if bytes[end] == b'\\' && end + 1 < bytes.len() {
            end += 1;
        }
        end += 1;
    }
    if end >= bytes.len() {
        return None;
    }
    Some(s[1..end].to_string())
}

/// Render the locked restore one-liner the CLI prints on success.
/// When the source slot had no data dir (`archived_data_dir = None`),
/// the data-restore arm is omitted — restoring TOML alone is the
/// correct action.
pub fn render_restore_one_liner(outcome: &DestroyOutcome, makakoo_home: &Path) -> String {
    let slot = &outcome.slot_id;
    let archive = outcome.archive_dir.display();
    let cfg = makakoo_home.join("config/agents").display().to_string();
    if outcome.archived_data_dir.is_some() {
        let data = slot_data_dir(makakoo_home, slot).display().to_string();
        format!(
            "to restore: mv {archive}/{slot}.toml {cfg}/ && mv {archive}/data {data}"
        )
    } else {
        format!("to restore: mv {archive}/{slot}.toml {cfg}/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_slot(home: &Path, slot_id: &str, body: &str) {
        let cfg = home.join("config/agents");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join(format!("{slot_id}.toml")), body).unwrap();
    }

    fn write_data(home: &Path, slot_id: &str, file: &str, body: &str) {
        let dir = slot_data_dir(home, slot_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), body).unwrap();
    }

    #[test]
    fn destroy_moves_toml_and_data_to_archive() {
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        write_data(tmp.path(), "secretary", "conversations.db", "fake data");

        let outcome = destroy_slot(tmp.path(), "secretary", false, 1700000000).unwrap();

        assert!(!slot_path(tmp.path(), "secretary").exists());
        assert!(!slot_data_dir(tmp.path(), "secretary").exists());
        assert!(outcome.archived_toml.exists());
        assert!(outcome.archived_data_dir.is_some());
        assert!(outcome.archived_data_dir.as_ref().unwrap().exists());
        assert!(outcome
            .archive_dir
            .ends_with("archive/agents/secretary-1700000000"));
    }

    #[test]
    fn destroy_works_when_data_dir_absent() {
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");

        let outcome = destroy_slot(tmp.path(), "secretary", false, 1700000001).unwrap();
        assert!(outcome.archived_data_dir.is_none());
        assert!(outcome.archived_toml.exists());
    }

    #[test]
    fn destroy_refuses_protected_slot_without_flag() {
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "harveychat", "slot_id = \"harveychat\"\n");
        let err = destroy_slot(tmp.path(), "harveychat", false, 1700000002).unwrap_err();
        assert!(matches!(err, DestroyError::HarveychatProtected));
        assert!(slot_path(tmp.path(), "harveychat").exists(), "TOML preserved");
    }

    #[test]
    fn destroy_protected_slot_with_explicit_flag_succeeds() {
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "harveychat", "slot_id = \"harveychat\"\n");
        let outcome = destroy_slot(tmp.path(), "harveychat", true, 1700000003).unwrap();
        assert_eq!(outcome.slot_id, "harveychat");
        assert!(outcome.archived_toml.exists());
    }

    #[test]
    fn destroy_returns_slot_not_found_for_missing_slot() {
        let tmp = TempDir::new().unwrap();
        let err = destroy_slot(tmp.path(), "ghost", false, 1700000004).unwrap_err();
        assert!(matches!(err, DestroyError::SlotNotFound { .. }));
    }

    #[test]
    fn destroy_refuses_overwriting_existing_archive() {
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        // Pre-create the archive dir to simulate a collision.
        let pre = archive_dir(tmp.path(), "secretary", 1700000005);
        fs::create_dir_all(&pre).unwrap();
        let err = destroy_slot(tmp.path(), "secretary", false, 1700000005).unwrap_err();
        assert!(matches!(err, DestroyError::ArchiveExists { .. }));
        assert!(slot_path(tmp.path(), "secretary").exists(), "TOML preserved on collision");
    }

    #[test]
    fn scan_detects_secret_ref_literal() {
        let body = r#"
[[transport]]
secret_ref = "agent/secretary/telegram-main/bot_token"
"#;
        let v = scan_secret_refs(body);
        assert_eq!(v, vec!["agent/secretary/telegram-main/bot_token".to_string()]);
    }

    #[test]
    fn scan_detects_app_token_signing_verify_etc() {
        let body = r#"
[[transport]]
secret_ref = "x"
app_token_ref = "y"
signing_secret_ref = "z"
verify_token_ref = "v"
access_token_ref = "a"
refresh_token_ref = "r"
client_secret_ref = "c"
app_secret_ref = "s"
"#;
        let v = scan_secret_refs(body);
        assert_eq!(v.len(), 8);
        assert!(v.contains(&"x".to_string()));
        assert!(v.contains(&"y".to_string()));
        assert!(v.contains(&"z".to_string()));
        assert!(v.contains(&"v".to_string()));
        assert!(v.contains(&"a".to_string()));
        assert!(v.contains(&"r".to_string()));
        assert!(v.contains(&"c".to_string()));
        assert!(v.contains(&"s".to_string()));
    }

    #[test]
    fn scan_skips_commented_lines() {
        let body = r#"
# secret_ref = "noise"
secret_ref = "real"
"#;
        let v = scan_secret_refs(body);
        assert_eq!(v, vec!["real".to_string()]);
    }

    #[test]
    fn scan_does_not_collide_with_inline_secret_dev() {
        let body = r#"
inline_secret_dev = "should-not-match"
"#;
        let v = scan_secret_refs(body);
        assert!(v.is_empty(), "inline_secret_dev must not match secret_ref scan; got {v:?}");
    }

    #[test]
    fn scan_dedups() {
        let body = r#"
secret_ref = "same"
secret_ref = "same"
"#;
        let v = scan_secret_refs(body);
        assert_eq!(v, vec!["same".to_string()]);
    }

    #[test]
    fn destroy_outcome_includes_detected_secrets() {
        let tmp = TempDir::new().unwrap();
        let body = r#"
slot_id = "secretary"
[[transport]]
secret_ref = "agent/secretary/telegram-main/bot_token"
"#;
        write_slot(tmp.path(), "secretary", body);
        let outcome = destroy_slot(tmp.path(), "secretary", false, 1700000006).unwrap();
        assert_eq!(
            outcome.detected_secrets,
            vec!["agent/secretary/telegram-main/bot_token".to_string()]
        );
    }

    #[test]
    fn restore_one_liner_includes_archive_path() {
        let outcome = DestroyOutcome {
            slot_id: "secretary".into(),
            archive_dir: PathBuf::from("/m/archive/agents/secretary-1700000000"),
            archived_toml: PathBuf::from("/m/archive/agents/secretary-1700000000/secretary.toml"),
            archived_data_dir: Some(PathBuf::from(
                "/m/archive/agents/secretary-1700000000/data",
            )),
            detected_secrets: vec![],
        };
        let line = render_restore_one_liner(&outcome, Path::new("/m"));
        assert!(line.contains("mv "));
        assert!(line.contains("/m/archive/agents/secretary-1700000000/secretary.toml"));
        assert!(line.contains("/m/config/agents/"));
        assert!(line.contains("/m/archive/agents/secretary-1700000000/data"));
    }

    #[test]
    fn restore_one_liner_omits_data_arm_when_no_data_archived() {
        // Round-2 fix: the restore line must not reference a data/ dir
        // that doesn't exist. Slots that never started have
        // archived_data_dir = None.
        let outcome = DestroyOutcome {
            slot_id: "secretary".into(),
            archive_dir: PathBuf::from("/m/archive/agents/secretary-1700000000"),
            archived_toml: PathBuf::from("/m/archive/agents/secretary-1700000000/secretary.toml"),
            archived_data_dir: None,
            detected_secrets: vec![],
        };
        let line = render_restore_one_liner(&outcome, Path::new("/m"));
        assert!(line.contains("mv "));
        assert!(line.contains("/m/archive/agents/secretary-1700000000/secretary.toml"));
        assert!(
            !line.contains("data"),
            "no data restore arm should appear; got: {line}"
        );
    }

    #[test]
    fn destroy_creates_empty_data_dir_in_archive_when_source_absent() {
        // Locked Q3 archive shape is `<slot>.toml + data/` always.
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "secretary", "slot_id = \"secretary\"\n");
        let outcome = destroy_slot(tmp.path(), "secretary", false, 1700000007).unwrap();
        assert!(
            outcome.archived_data_dir.is_none(),
            "outcome reflects that source had no data"
        );
        let archive_data = outcome.archive_dir.join("data");
        assert!(
            archive_data.exists() && archive_data.is_dir(),
            "archive must include empty data/ dir when source had none"
        );
    }
}
