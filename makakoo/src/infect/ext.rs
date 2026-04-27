//! Infect writers for extension-based hosts (VSCode Copilot / Cline /
//! Continue.dev / JetBrains AI).
//!
//! Spec: `spec/INSTALL_MATRIX.md §3.8-3.9`. These hosts aren't in the
//! 7-CLI `SLOTS` table because their config paths are OS-specific
//! application-support dirs, and Continue.dev uses a JSON field
//! rather than a standalone markdown file.
//!
//! **Three of the four are plain markdown** — Copilot's
//! `copilot-instructions.md`, Cline's `CLAUDE.md`, JetBrains'
//! `AI_Assistant/rules.md`. They reuse the existing
//! `upsert_markdown_block` machinery with the same `harvey:infect-global`
//! sentinel markers, so `makakoo infect` can re-run idempotently.
//!
//! **Continue.dev is JSON** — its config.json has a `systemMessage`
//! string field the AI reads. We splice the Bootstrap Block into that
//! string framed by sentinel markers so refresh can locate and replace
//! the prior block without touching the user's own prose.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use super::slots::BLOCK_VERSION;
use super::writer::{
    atomic_write, render_markdown_block, upsert_markdown_block, SlotStatus,
    SlotWriteResult,
};

/// The 4 extension-based hosts shipped at F/5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtHostKind {
    Copilot,
    Continue,
    Cline,
    JetBrains,
}

impl ExtHostKind {
    pub fn slot_name(self) -> &'static str {
        match self {
            ExtHostKind::Copilot => "vscode-copilot",
            ExtHostKind::Continue => "continue-dev",
            ExtHostKind::Cline => "cline",
            ExtHostKind::JetBrains => "jetbrains-ai",
        }
    }
}

/// A resolved target: the file path + which kind of writer to use.
#[derive(Debug, Clone)]
pub struct ExtTarget {
    pub kind: ExtHostKind,
    pub path: PathBuf,
}

/// Top-level entrypoint. Writes the bootstrap to one extension host
/// target. Returns a `SlotWriteResult` in the same shape as the 7-CLI
/// infect flow so reports read identically.
pub fn write_ext_host(
    target: &ExtTarget,
    bootstrap_body: &str,
    dry_run: bool,
) -> SlotWriteResult {
    match target.kind {
        ExtHostKind::Copilot | ExtHostKind::Cline | ExtHostKind::JetBrains => {
            write_markdown_file(target, bootstrap_body, dry_run)
        }
        ExtHostKind::Continue => write_continue_json(target, bootstrap_body, dry_run),
    }
}

fn write_markdown_file(
    target: &ExtTarget,
    bootstrap_body: &str,
    dry_run: bool,
) -> SlotWriteResult {
    let existing = std::fs::read_to_string(&target.path).unwrap_or_default();
    let new_block = render_markdown_block(bootstrap_body);
    let (new_text, status, prior_version) = upsert_markdown_block(&existing, &new_block);

    if matches!(status, SlotStatus::Unchanged) || dry_run {
        let final_status = if dry_run && !matches!(status, SlotStatus::Unchanged) {
            SlotStatus::DryRun
        } else {
            status
        };
        return SlotWriteResult {
            slot_name: target.kind.slot_name(),
            path: target.path.clone(),
            status: final_status,
            prior_version,
        };
    }

    match atomic_write(&target.path, &new_text) {
        Ok(_) => SlotWriteResult {
            slot_name: target.kind.slot_name(),
            path: target.path.clone(),
            status,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: target.kind.slot_name(),
            path: target.path.clone(),
            status: SlotStatus::Error(format!("{e:#}")),
            prior_version,
        },
    }
}

fn write_continue_json(
    target: &ExtTarget,
    bootstrap_body: &str,
    dry_run: bool,
) -> SlotWriteResult {
    let result = splice_continue_json(&target.path, bootstrap_body, dry_run);
    match result {
        Ok((status, prior_version)) => SlotWriteResult {
            slot_name: target.kind.slot_name(),
            path: target.path.clone(),
            status,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: target.kind.slot_name(),
            path: target.path.clone(),
            status: SlotStatus::Error(format!("{e:#}")),
            prior_version: None,
        },
    }
}

/// Returns (status, prior_version). Continue.dev's config.json has a
/// top-level `systemMessage` string field — we frame the Bootstrap Block
/// with sentinel markers inside that string so refresh locates and
/// replaces the prior block without touching the user's other prose.
fn splice_continue_json(
    path: &std::path::Path,
    bootstrap_body: &str,
    dry_run: bool,
) -> Result<(SlotStatus, Option<String>)> {
    let existing_raw = std::fs::read_to_string(path).unwrap_or_default();

    // Empty file → seed a minimal config with just systemMessage.
    let mut root: Value = if existing_raw.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&existing_raw)
            .with_context(|| format!("parse {}", path.display()))?
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Continue config is not a JSON object"))?;

    let previous_system =
        obj.get("systemMessage").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let block = render_markdown_block(bootstrap_body);
    let (new_system, status, prior_version) =
        upsert_markdown_block(&previous_system, &block);

    if matches!(status, SlotStatus::Unchanged) {
        return Ok((status, prior_version));
    }

    obj.insert("systemMessage".into(), Value::String(new_system));

    if dry_run {
        return Ok((SlotStatus::DryRun, prior_version));
    }

    let rendered = serde_json::to_string_pretty(&root)? + "\n";
    atomic_write(path, &rendered)?;
    Ok((status, prior_version))
}

/// Keep clippy happy — the block version constant is referenced in
/// upstream tests even if this module doesn't otherwise touch it.
#[allow(dead_code)]
const _BLOCK_VERSION_ANCHOR: &str = BLOCK_VERSION;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn body() -> &'static str {
        "bootstrap content line 1\nbootstrap content line 2"
    }

    fn tgt(kind: ExtHostKind, p: PathBuf) -> ExtTarget {
        ExtTarget { kind, path: p }
    }

    #[test]
    fn copilot_fresh_install_creates_markdown() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("copilot-instructions.md");
        let r = write_ext_host(&tgt(ExtHostKind::Copilot, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Installed);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("<!-- harvey:infect-global START"));
        assert!(content.contains("bootstrap content line 1"));
        assert!(content.contains("<!-- harvey:infect-global END -->"));
    }

    #[test]
    fn copilot_idempotent_second_run_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("copilot-instructions.md");
        write_ext_host(&tgt(ExtHostKind::Copilot, path.clone()), body(), false);
        let r = write_ext_host(&tgt(ExtHostKind::Copilot, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Unchanged);
    }

    #[test]
    fn copilot_upgrade_replaces_old_block_in_place() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("copilot-instructions.md");
        std::fs::write(
            &path,
            "# My Copilot instructions\n\n<!-- harvey:infect-global START v7 -->\nold v7\n<!-- harvey:infect-global END -->\n\n# More user content\n",
        )
        .unwrap();
        let r = write_ext_host(&tgt(ExtHostKind::Copilot, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Updated);
        assert_eq!(r.prior_version.as_deref(), Some("7"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# My Copilot instructions"));
        assert!(content.contains("# More user content"));
        assert!(!content.contains("old v7"));
        assert!(content.contains("bootstrap content line 1"));
    }

    #[test]
    fn jetbrains_fresh_install() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("IntelliJIdea2025.1/AI_Assistant/rules.md");
        let r = write_ext_host(&tgt(ExtHostKind::JetBrains, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Installed);
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("bootstrap content line 1"));
    }

    #[test]
    fn cline_dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let r = write_ext_host(&tgt(ExtHostKind::Cline, path.clone()), body(), true);
        assert_eq!(r.status, SlotStatus::DryRun);
        assert!(!path.exists());
    }

    #[test]
    fn continue_fresh_install_seeds_minimal_json() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        let r = write_ext_host(&tgt(ExtHostKind::Continue, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Installed);
        let content = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        let sys = v["systemMessage"].as_str().unwrap();
        assert!(sys.contains("bootstrap content line 1"));
        assert!(sys.contains("<!-- harvey:infect-global START"));
    }

    #[test]
    fn continue_preserves_other_json_keys() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "models": [{ "title": "claude", "provider": "anthropic" }],
  "systemMessage": "you are my coding buddy"
}"#,
        )
        .unwrap();
        let r = write_ext_host(&tgt(ExtHostKind::Continue, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Installed);
        let content = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["models"][0]["title"], "claude");
        let sys = v["systemMessage"].as_str().unwrap();
        assert!(sys.contains("you are my coding buddy"));
        assert!(sys.contains("bootstrap content line 1"));
    }

    #[test]
    fn continue_idempotent_refresh() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        write_ext_host(&tgt(ExtHostKind::Continue, path.clone()), body(), false);
        let r = write_ext_host(&tgt(ExtHostKind::Continue, path.clone()), body(), false);
        assert_eq!(r.status, SlotStatus::Unchanged);
    }

    #[test]
    fn continue_rejects_non_object_config() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "[\"not an object\"]").unwrap();
        let r = write_ext_host(&tgt(ExtHostKind::Continue, path.clone()), body(), false);
        match r.status {
            SlotStatus::Error(msg) => assert!(msg.contains("not a JSON object")),
            other => panic!("expected error, got {other:?}"),
        }
    }
}
