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
    /// Original generated runtime path, when declared by the slot.
    #[serde(default)]
    pub runtime_project_dir: Option<PathBuf>,
    /// Managed runtime project moved to `<archive>/runtime/`.
    #[serde(default)]
    pub archived_runtime_dir: Option<PathBuf>,
    /// Recovery warning when malformed legacy TOML had no runtime table.
    #[serde(default)]
    pub runtime_archive_warning: Option<String>,
    /// Direct `*_ref = "..."` literals found in the TOML.
    /// Dotted-key assignments and env-var interpolation are not detected.
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

    #[error("invalid slot runtime metadata at {path}: {message}")]
    InvalidRuntimeMetadata { path: PathBuf, message: String },

    #[error("could not archive generated runtime: {message}")]
    RuntimeArchive { message: String },

    #[error("destroy archive transaction failed: {message}")]
    ArchiveTransaction { message: String },

    #[error("invalid slot id '{slot_id}': {message}")]
    InvalidSlotId { slot_id: String, message: String },
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
    super::validate_slot_id(slot_id).map_err(|error| DestroyError::InvalidSlotId {
        slot_id: slot_id.to_string(),
        message: error.to_string(),
    })?;
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

    let toml_body =
        std::fs::read_to_string(&toml_path).map_err(|e| DestroyError::SlotNotFound {
            slot_id: format!("could not read slot TOML: {e}"),
            path: toml_path.clone(),
        })?;
    let runtime_plan =
        super::runtime_archive::plan(makakoo_home, slot_id, &toml_body).map_err(|message| {
            DestroyError::InvalidRuntimeMetadata {
                path: toml_path.clone(),
                message,
            }
        })?;

    let dst = archive_dir(makakoo_home, slot_id, unix_ts);
    if dst.exists() {
        return Err(DestroyError::ArchiveExists { path: dst });
    }
    std::fs::create_dir_all(&dst).map_err(|error| DestroyError::ArchiveTransaction {
        message: format!("create archive directory {}: {error}", dst.display()),
    })?;

    // Read TOML body BEFORE moving it so we can scan for
    // secret_ref literals. The scan is intentionally simple — just
    // `secret_ref` at any indent — to avoid false positives from
    // commented-out lines while still catching every shipping
    // variant (secret_ref + app_token_ref + signing_secret_ref +
    // verify_token_ref + access_token_ref + password_ref +
    // refresh_token_ref + client_secret_ref).
    let detected_secrets = scan_secret_refs(&toml_body);

    // Stage runtime + data first. The TOML is the registry commit marker and
    // moves only after every other archive operation succeeds.
    let data_src = slot_data_dir(makakoo_home, slot_id);
    let staged = super::destroy_transaction::stage(&runtime_plan, &dst, &data_src)
        .map_err(|message| DestroyError::ArchiveTransaction { message })?;

    let archived_toml = dst.join(format!("{slot_id}.toml"));
    super::destroy_transaction::commit_registry(
        &runtime_plan,
        &dst,
        &data_src,
        &staged,
        &toml_path,
        &archived_toml,
    )
    .map_err(|message| DestroyError::ArchiveTransaction { message })?;

    Ok(DestroyOutcome {
        slot_id: slot_id.to_string(),
        archive_dir: dst,
        archived_toml,
        archived_data_dir: staged.archived_data_dir,
        runtime_project_dir: runtime_plan.project_dir,
        archived_runtime_dir: staged.archived_runtime_dir,
        runtime_archive_warning: runtime_plan.warning,
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
            let needle = key.to_string();
            if let Some(pos) = trimmed.find(&needle) {
                // Left of pos must be empty or whitespace (so we
                // don't match `inline_secret_ref` against
                // `secret_ref`).
                if pos > 0
                    && !trimmed[..pos]
                        .chars()
                        .last()
                        .map(char::is_whitespace)
                        .unwrap_or(true)
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
    let archive = &outcome.archive_dir;
    let cfg = makakoo_home.join("config/agents");
    let mut commands = vec![render_move(&archive.join(format!("{slot}.toml")), &cfg)];
    if outcome.archived_data_dir.is_some() {
        let data = slot_data_dir(makakoo_home, slot);
        commands.push(render_move(&archive.join("data"), &data));
    }
    if outcome.archived_runtime_dir.is_some() {
        if let Some(runtime) = outcome.runtime_project_dir.as_ref() {
            commands.push(render_move(&archive.join("runtime"), runtime));
        }
    }
    format!("to restore: {}", commands.join(" && "))
}

/// Unix restore step: POSIX `mv` with single-quote escaping.
#[cfg(not(windows))]
fn render_move(source: &Path, destination: &Path) -> String {
    format!("mv {} {}", shell_quote(source), shell_quote(destination))
}

/// Windows restore step: PowerShell `Move-Item` with single-quote
/// escaping. The `&&` join above is valid in PowerShell 7+.
#[cfg(windows)]
fn render_move(source: &Path, destination: &Path) -> String {
    format!(
        "Move-Item -LiteralPath {} -Destination {}",
        powershell_quote(source),
        powershell_quote(destination)
    )
}

#[cfg(not(windows))]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

/// PowerShell single-quoted literal — the only escape is doubling `'`.
#[cfg(windows)]
fn powershell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
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
        assert!(
            slot_path(tmp.path(), "harveychat").exists(),
            "TOML preserved"
        );
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
        assert!(
            slot_path(tmp.path(), "secretary").exists(),
            "TOML preserved on collision"
        );
    }

    #[test]
    fn scan_detects_secret_ref_literal() {
        let body = r#"
[[transport]]
secret_ref = "agent/secretary/telegram-main/bot_token"
"#;
        let v = scan_secret_refs(body);
        assert_eq!(
            v,
            vec!["agent/secretary/telegram-main/bot_token".to_string()]
        );
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
        assert!(
            v.is_empty(),
            "inline_secret_dev must not match secret_ref scan; got {v:?}"
        );
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

    #[cfg(not(windows))]
    #[test]
    fn restore_one_liner_includes_archive_path() {
        let outcome = DestroyOutcome {
            slot_id: "secretary".into(),
            archive_dir: PathBuf::from("/m/archive/agents/secretary-1700000000"),
            archived_toml: PathBuf::from("/m/archive/agents/secretary-1700000000/secretary.toml"),
            archived_data_dir: Some(PathBuf::from("/m/archive/agents/secretary-1700000000/data")),
            runtime_project_dir: None,
            archived_runtime_dir: None,
            runtime_archive_warning: None,
            detected_secrets: vec![],
        };
        let line = render_restore_one_liner(&outcome, Path::new("/m"));
        let normalized = line.replace('\\', "/");
        assert!(line.contains("mv "));
        assert!(normalized.contains("/m/archive/agents/secretary-1700000000/secretary.toml"));
        assert!(normalized.contains("/m/config/agents"));
        assert!(normalized.contains("/m/archive/agents/secretary-1700000000/data"));
    }

    #[cfg(not(windows))]
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
            runtime_project_dir: None,
            archived_runtime_dir: None,
            runtime_archive_warning: None,
            detected_secrets: vec![],
        };
        let line = render_restore_one_liner(&outcome, Path::new("/m"));
        let normalized = line.replace('\\', "/");
        assert!(line.contains("mv "));
        assert!(normalized.contains("/m/archive/agents/secretary-1700000000/secretary.toml"));
        assert!(
            !line.contains("data"),
            "no data restore arm should appear; got: {line}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn restore_one_liner_shell_quotes_spaces_and_apostrophes() {
        let outcome = DestroyOutcome {
            slot_id: "secretary".into(),
            archive_dir: PathBuf::from("/tmp/Makakoo's Archive/secretary-1"),
            archived_toml: PathBuf::new(),
            archived_data_dir: Some(PathBuf::from("data")),
            runtime_project_dir: None,
            archived_runtime_dir: None,
            runtime_archive_warning: None,
            detected_secrets: vec![],
        };
        let line = render_restore_one_liner(&outcome, Path::new("/tmp/My Makakoo"));
        assert!(line.contains("'/tmp/Makakoo'\"'\"'s Archive/secretary-1/secretary.toml'"));
        assert!(line.contains("'/tmp/My Makakoo/config/agents'"));
    }

    #[cfg(windows)]
    #[test]
    fn restore_one_liner_renders_powershell_move_item() {
        let outcome = DestroyOutcome {
            slot_id: "secretary".into(),
            archive_dir: PathBuf::from("C:\\Makakoo's Archive\\secretary-1"),
            archived_toml: PathBuf::new(),
            archived_data_dir: Some(PathBuf::from("data")),
            runtime_project_dir: None,
            archived_runtime_dir: None,
            runtime_archive_warning: None,
            detected_secrets: vec![],
        };
        let line = render_restore_one_liner(&outcome, Path::new("C:\\My Makakoo"));
        assert!(!line.contains("mv "), "no POSIX mv on Windows; got: {line}");
        // PowerShell escapes a single quote by doubling it.
        assert!(line.contains(
            "Move-Item -LiteralPath 'C:\\Makakoo''s Archive\\secretary-1\\secretary.toml' -Destination 'C:\\My Makakoo\\config\\agents'"
        ));
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

    #[test]
    fn destroy_archives_managed_runtime_and_restore_includes_it() {
        let tmp = TempDir::new().unwrap();
        let runtime = tmp.path().join("agents-dsh/researcher");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join(".env"), "TOKEN=secret").unwrap();
        write_slot(
            tmp.path(),
            "researcher",
            &format!(
                "slot_id = \"researcher\"\n[runtime]\nengine = \"deepseek-harness\"\nproject_dir = {:?}\n",
                runtime
            ),
        );

        let outcome = destroy_slot(tmp.path(), "researcher", false, 1700000008).unwrap();
        let archived = outcome.archived_runtime_dir.as_ref().unwrap();
        assert!(!runtime.exists());
        assert_eq!(
            fs::read_to_string(archived.join(".env")).unwrap(),
            "TOKEN=secret"
        );
        let restore = render_restore_one_liner(&outcome, tmp.path());
        let normalized = restore.replace('\\', "/");
        assert!(normalized.contains("/runtime"));
        assert!(restore.contains(&runtime.display().to_string()));
    }

    #[test]
    fn destroy_preserves_runtime_outside_managed_roots() {
        let tmp = TempDir::new().unwrap();
        let external = tmp.path().join("custom/researcher");
        fs::create_dir_all(&external).unwrap();
        write_slot(
            tmp.path(),
            "researcher",
            &format!(
                "slot_id = \"researcher\"\n[runtime]\nengine = \"deepseek-harness\"\nproject_dir = {:?}\n",
                external
            ),
        );

        let outcome = destroy_slot(tmp.path(), "researcher", false, 1700000009).unwrap();
        assert!(external.exists());
        assert!(outcome.archived_runtime_dir.is_none());
        assert_eq!(
            outcome.runtime_project_dir.as_deref(),
            Some(external.as_path())
        );
    }

    #[test]
    fn destroy_archives_malformed_legacy_toml_without_runtime() {
        let tmp = TempDir::new().unwrap();
        write_slot(tmp.path(), "broken", "slot_id =");
        let outcome = destroy_slot(tmp.path(), "broken", false, 1700000010).unwrap();
        assert!(outcome.archived_toml.exists());
        assert!(outcome
            .runtime_archive_warning
            .unwrap()
            .contains("malformed"));
    }
}
