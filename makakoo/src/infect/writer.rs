//! Atomic bootstrap-block writer — handles both markdown and opencode JSON
//! slots with idempotent, version-aware in-place upgrades.
//!
//! Semantics match `core/orchestration/infect_global.py`:
//!
//!   - **Markdown**: if the START..END block exists, replace it in place;
//!     otherwise append with a blank-line separator.
//!   - **OpenCode JSON**: parse the file, find an existing entry in the
//!     `instructions: []` array that starts with `[harvey:infect-global`,
//!     replace or append.
//!   - Every successful write goes through a temp file + rename so a
//!     partial write never corrupts the existing config.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::{json, Value};

use super::slots::{
    CliSlot, SlotFormat, BLOCK_END, BLOCK_START, BLOCK_VERSION, JSON_TAG_FINGERPRINT,
    JSON_TAG_PREFIX,
};

/// Status of a single slot write. Mirrors the Python
/// `SlotStatus` enum so reports read the same across languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotStatus {
    /// Block did not exist before — freshly installed.
    Installed,
    /// Prior version found and replaced in place.
    Updated,
    /// Same version already present — no write performed.
    Unchanged,
    /// `--dry-run` mode — write was skipped but would have succeeded.
    DryRun,
    /// Write failed or existing state was invalid.
    Error(String),
}

/// Outcome of writing to a single slot.
#[derive(Debug, Clone)]
pub struct SlotWriteResult {
    pub slot_name: &'static str,
    pub path: PathBuf,
    pub status: SlotStatus,
    pub prior_version: Option<String>,
}

/// Expand a leading `~/` to the current user's home directory. Never panics
/// — returns an error if `$HOME` cannot be resolved.
pub fn expand_tilde(path: &str) -> Result<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot resolve $HOME"))?;
        Ok(home.join(rest))
    } else {
        Ok(PathBuf::from(path))
    }
}

/// Atomic write: write to a sibling `.tmp` file then rename into place.
pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp.infect");
    std::fs::write(&tmp, content)
        .with_context(|| format!("failed to write temp file {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Build the fenced markdown block (START marker + body + END marker).
pub fn render_markdown_block(body: &str) -> String {
    let body = body.trim_end();
    format!("{}\n{}\n{}\n", BLOCK_START, body, BLOCK_END)
}

/// Build the tagged JSON-array entry for the opencode slot.
pub fn render_opencode_entry(body: &str) -> String {
    format!("{} {}", JSON_TAG_PREFIX, body.trim())
}

/// Regex that matches any prior version of the harvey infect block so
/// upgrades are in-place regardless of which earlier version (v6 … v9)
/// was installed last. Matches exactly what the Python version matches.
fn block_regex() -> Regex {
    // Match `<!-- harvey:infect-global START v{anything} --> ... <!-- harvey:infect-global END -->`
    // including any surrounding blank lines.
    Regex::new(
        r"(?s)\n*<!--\s*harvey:infect-global\s+START\s+v[^\s>]+\s*-->.*?<!--\s*harvey:infect-global\s+END\s*-->\n*",
    )
    .expect("infect block regex is valid")
}

/// Extract the prior block version from existing text. Returns `None` if no
/// block is present.
fn find_prior_version(text: &str) -> Option<String> {
    let re = block_regex();
    let m = re.find(text)?;
    let block = m.as_str();
    // Pull the `v<version>` token out of the START marker.
    let vre = Regex::new(r"START\s+v(\S+?)\s*-->").ok()?;
    vre.captures(block)
        .and_then(|c| c.get(1))
        .map(|g| g.as_str().to_string())
}

/// Remove the markdown bootstrap block from `text`. Returns
/// (new_text, removed). `removed` is true iff a block was found and
/// stripped. Idempotent — running this against text that has already
/// been uninfected is a no-op.
pub fn remove_markdown_block(text: &str) -> (String, bool) {
    let re = block_regex();
    if let Some(m) = re.find(text) {
        let before = &text[..m.start()];
        let after = &text[m.end()..];
        let mut out = String::with_capacity(before.len() + after.len());
        out.push_str(before);
        // Trim duplicate trailing newlines we may have left behind —
        // the block captures its own surrounding blank lines, so the
        // seam usually stitches cleanly, but users' prior content may
        // already end with one newline that we want to keep.
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !after.is_empty() && after.starts_with('\n') && out.ends_with('\n') {
            out.push_str(&after[1..]);
        } else {
            out.push_str(after);
        }
        (out, true)
    } else {
        (text.to_string(), false)
    }
}

/// Upsert the markdown bootstrap block into `text`. Returns
/// (new_text, status, prior_version_if_any).
///
/// "Unchanged" means the existing block matches `new_block` byte-for-byte
/// (trimmed). Version alone is NOT sufficient — content can drift within
/// a version (e.g. a bootstrap-base.md edit without bumping BLOCK_VERSION),
/// and we want the next infect to pick that up.
pub fn upsert_markdown_block(text: &str, new_block: &str) -> (String, SlotStatus, Option<String>) {
    let re = block_regex();
    if let Some(m) = re.find(text) {
        let prior_version = find_prior_version(text);
        let existing_block = m.as_str().trim_matches('\n');
        if existing_block == new_block.trim_matches('\n') {
            return (text.to_string(), SlotStatus::Unchanged, prior_version);
        }
        let before = &text[..m.start()];
        let after = &text[m.end()..];
        // Preserve a newline separator between surrounding content and the block.
        let mut out = String::with_capacity(text.len() + new_block.len());
        out.push_str(before);
        if !before.ends_with('\n') && !before.is_empty() {
            out.push('\n');
        }
        out.push_str(new_block);
        out.push_str(after);
        (out, SlotStatus::Updated, prior_version)
    } else {
        // Append with a blank-line separator.
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(new_block);
        (out, SlotStatus::Installed, None)
    }
}

/// Write the bootstrap to a single slot. Respects `dry_run`.
pub fn write_bootstrap_to_slot(
    slot: &CliSlot,
    bootstrap_body: &str,
    home: &Path,
    dry_run: bool,
) -> SlotWriteResult {
    let path = slot.absolute(home);
    match slot.format {
        SlotFormat::Markdown => write_markdown(slot, &path, bootstrap_body, dry_run),
        SlotFormat::OpencodeJson => write_opencode(slot, &path, bootstrap_body, dry_run),
    }
}

/// Remove the bootstrap block from a single slot. Mirrors
/// [`write_bootstrap_to_slot`] — markdown slots strip the block, the
/// opencode JSON slot removes the tagged instructions entry. Returns
/// a result with:
/// - `SlotStatus::Updated` when a block was present and removed,
/// - `SlotStatus::Unchanged` when no block was found (nothing to do),
/// - `SlotStatus::DryRun` when `dry_run=true` and a removal would occur,
/// - `SlotStatus::Error(..)` on IO / parse failures.
pub fn remove_bootstrap_from_slot(
    slot: &CliSlot,
    home: &Path,
    dry_run: bool,
) -> SlotWriteResult {
    let path = slot.absolute(home);
    match slot.format {
        SlotFormat::Markdown => remove_markdown(slot, &path, dry_run),
        SlotFormat::OpencodeJson => remove_opencode(slot, &path, dry_run),
    }
}

fn remove_markdown(slot: &CliSlot, path: &Path, dry_run: bool) -> SlotWriteResult {
    if !path.exists() {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    }
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Error(format!("read {}: {e}", path.display())),
                prior_version: None,
            }
        }
    };
    let prior_version = find_prior_version(&existing);
    let (new_text, removed) = remove_markdown_block(&existing);
    if !removed {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    }
    if dry_run {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::DryRun,
            prior_version,
        };
    }
    // If nothing is left, delete the file outright — infect created it,
    // uninfect removes it. If the user had their own prose, keep it.
    let trimmed = new_text.trim();
    let result: anyhow::Result<()> = if trimmed.is_empty() {
        std::fs::remove_file(path).map_err(anyhow::Error::from)
    } else {
        atomic_write(path, &new_text)
    };
    match result {
        Ok(()) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Updated,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Error(e.to_string()),
            prior_version,
        },
    }
}

fn remove_opencode(slot: &CliSlot, path: &Path, dry_run: bool) -> SlotWriteResult {
    if !path.exists() {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Error(format!("read {}: {e}", path.display())),
                prior_version: None,
            }
        }
    };
    let mut data: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Error(format!("invalid opencode JSON: {e}")),
                prior_version: None,
            }
        }
    };
    let Some(obj) = data.as_object_mut() else {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    };
    let Some(instructions) = obj.get_mut("instructions") else {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    };
    let Some(arr) = instructions.as_array_mut() else {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    };
    // Locate + capture the prior version before removing.
    let prior_idx = arr.iter().position(|v| {
        v.as_str()
            .map(|s| s.get(..40).unwrap_or(s).contains(JSON_TAG_FINGERPRINT))
            .unwrap_or(false)
    });
    let Some(idx) = prior_idx else {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Unchanged,
            prior_version: None,
        };
    };
    let prior_version =
        opencode_entry_version(arr[idx].as_str().unwrap_or_default());
    arr.remove(idx);
    // Drop the instructions array entirely if we emptied it — keeps
    // the user's config file tidy instead of leaving an empty array.
    if arr.is_empty() {
        obj.remove("instructions");
    }

    if dry_run {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::DryRun,
            prior_version,
        };
    }
    let serialized = match serde_json::to_string_pretty(&data) {
        Ok(s) => s + "\n",
        Err(e) => {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Error(format!("serialize opencode JSON: {e}")),
                prior_version,
            }
        }
    };
    match atomic_write(path, &serialized) {
        Ok(()) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Updated,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Error(e.to_string()),
            prior_version,
        },
    }
}

fn write_markdown(
    slot: &CliSlot,
    path: &Path,
    bootstrap_body: &str,
    dry_run: bool,
) -> SlotWriteResult {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let new_block = render_markdown_block(bootstrap_body);
    let (new_text, status, prior_version) = upsert_markdown_block(&existing, &new_block);

    if matches!(status, SlotStatus::Unchanged) {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status,
            prior_version,
        };
    }
    if dry_run {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::DryRun,
            prior_version,
        };
    }
    match atomic_write(path, &new_text) {
        Ok(()) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Error(e.to_string()),
            prior_version,
        },
    }
}

fn write_opencode(
    slot: &CliSlot,
    path: &Path,
    bootstrap_body: &str,
    dry_run: bool,
) -> SlotWriteResult {
    // Load existing JSON (or empty object if the file does not exist).
    let mut data: Value = if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(s) if !s.trim().is_empty() => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => {
                    return SlotWriteResult {
                        slot_name: slot.name,
                        path: path.to_path_buf(),
                        status: SlotStatus::Error(format!("invalid opencode JSON: {e}")),
                        prior_version: None,
                    }
                }
            },
            _ => json!({}),
        }
    } else {
        json!({})
    };

    if !data.is_object() {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Error("opencode config root is not an object".into()),
            prior_version: None,
        };
    }

    let obj = data.as_object_mut().unwrap();
    let instructions = obj
        .entry("instructions".to_string())
        .or_insert_with(|| Value::Array(vec![]));
    let arr = match instructions.as_array_mut() {
        Some(a) => a,
        None => {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Error("opencode `instructions` is not an array".into()),
                prior_version: None,
            }
        }
    };

    let new_entry = render_opencode_entry(bootstrap_body);
    // Locate prior tagged entry (first 40 chars contain the fingerprint).
    let prior_idx = arr.iter().position(|v| {
        v.as_str()
            .map(|s| s.get(..40).unwrap_or(s).contains(JSON_TAG_FINGERPRINT))
            .unwrap_or(false)
    });

    let (status, prior_version) = if let Some(idx) = prior_idx {
        let prior_str = arr[idx].as_str().unwrap_or_default().to_string();
        let pv = opencode_entry_version(&prior_str);
        if pv.as_deref() == Some(BLOCK_VERSION) && prior_str == new_entry {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Unchanged,
                prior_version: pv,
            };
        }
        arr[idx] = Value::String(new_entry);
        (SlotStatus::Updated, pv)
    } else {
        arr.push(Value::String(new_entry));
        (SlotStatus::Installed, None)
    };

    if dry_run {
        return SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::DryRun,
            prior_version,
        };
    }

    let serialized = match serde_json::to_string_pretty(&data) {
        Ok(s) => s + "\n",
        Err(e) => {
            return SlotWriteResult {
                slot_name: slot.name,
                path: path.to_path_buf(),
                status: SlotStatus::Error(format!("serialize opencode JSON: {e}")),
                prior_version,
            }
        }
    };
    match atomic_write(path, &serialized) {
        Ok(()) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status,
            prior_version,
        },
        Err(e) => SlotWriteResult {
            slot_name: slot.name,
            path: path.to_path_buf(),
            status: SlotStatus::Error(e.to_string()),
            prior_version,
        },
    }
}

fn opencode_entry_version(entry: &str) -> Option<String> {
    let re = Regex::new(r"\[harvey:infect-global v(\S+?)\]").ok()?;
    re.captures(entry)
        .and_then(|c| c.get(1))
        .map(|g| g.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const BODY: &str = "You are Harvey. Test bootstrap body.";

    #[test]
    fn render_markdown_block_shape() {
        let b = render_markdown_block(BODY);
        assert!(b.starts_with(BLOCK_START));
        assert!(b.trim_end().ends_with(BLOCK_END));
        assert!(b.contains(BODY));
    }

    #[test]
    fn upsert_into_empty_file_appends() {
        let block = render_markdown_block(BODY);
        let (out, status, prior) = upsert_markdown_block("", &block);
        assert_eq!(status, SlotStatus::Installed);
        assert!(out.ends_with(&block));
        assert!(prior.is_none());
    }

    #[test]
    fn upsert_preserves_existing_content() {
        let existing = "# My notes\n\nHello world.\n";
        let block = render_markdown_block(BODY);
        let (out, status, _) = upsert_markdown_block(existing, &block);
        assert_eq!(status, SlotStatus::Installed);
        assert!(out.contains("# My notes"));
        assert!(out.contains("Hello world."));
        assert!(out.contains(BLOCK_START));
    }

    #[test]
    fn upsert_replaces_prior_version_in_place() {
        let prior = "# Top\n\n<!-- harvey:infect-global START v7 -->\nold body\n<!-- harvey:infect-global END -->\n\n# Bottom\n";
        let block = render_markdown_block(BODY);
        let (out, status, prior_version) = upsert_markdown_block(prior, &block);
        assert_eq!(status, SlotStatus::Updated);
        assert_eq!(prior_version.as_deref(), Some("7"));
        assert!(out.contains("# Top"));
        assert!(out.contains("# Bottom"));
        assert!(out.contains(BODY));
        assert!(!out.contains("old body"));
    }

    #[test]
    fn upsert_same_version_is_unchanged() {
        let block = render_markdown_block(BODY);
        let existing = format!("before\n\n{}\n\nafter\n", block);
        let (out, status, prior) = upsert_markdown_block(&existing, &block);
        assert_eq!(status, SlotStatus::Unchanged);
        assert_eq!(out, existing);
        assert_eq!(prior.as_deref(), Some(BLOCK_VERSION));
    }

    #[test]
    fn upsert_same_version_different_body_is_updated() {
        // Regression guard: when bootstrap-base.md drifts without a
        // BLOCK_VERSION bump, infect must still detect the change and
        // rewrite the slot. Prior behaviour short-circuited on version
        // alone, which hid a v10 content-edit in Sebastian's install
        // 2026-04-20.
        let old_block = render_markdown_block("old body");
        let new_block = render_markdown_block("BRAND NEW BODY with describe vs ingest guidance");
        let existing = format!("before\n\n{}\n\nafter\n", old_block);
        let (out, status, prior) = upsert_markdown_block(&existing, &new_block);
        assert_eq!(status, SlotStatus::Updated);
        assert_eq!(prior.as_deref(), Some(BLOCK_VERSION));
        assert!(out.contains("BRAND NEW BODY"));
        assert!(!out.contains("old body"));
        assert!(out.contains("before"));
        assert!(out.contains("after"));
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a/b/c/file.md");
        atomic_write(&target, "hi").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hi");
    }

    #[test]
    fn opencode_entry_version_extracts() {
        let e = "[harvey:infect-global v9] body text here";
        assert_eq!(opencode_entry_version(e).as_deref(), Some("9"));
    }

    #[test]
    fn write_bootstrap_to_markdown_slot_dry_run_is_noop() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "test",
            rel_path: ".test/NOTES.md",
            format: SlotFormat::Markdown,
        };
        let result = write_bootstrap_to_slot(&slot, BODY, tmp.path(), true);
        assert_eq!(result.status, SlotStatus::DryRun);
        // File must not have been created.
        assert!(!slot.absolute(tmp.path()).exists());
    }

    #[test]
    fn write_bootstrap_to_markdown_slot_writes_atomically() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "test",
            rel_path: ".test/NOTES.md",
            format: SlotFormat::Markdown,
        };
        let r = write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        assert!(matches!(r.status, SlotStatus::Installed));
        let written = std::fs::read_to_string(slot.absolute(tmp.path())).unwrap();
        assert!(written.contains(BODY));
        assert!(written.contains(BLOCK_START));
    }

    #[test]
    fn write_bootstrap_to_opencode_json_slot() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "opencode",
            rel_path: ".config/opencode/opencode.json",
            format: SlotFormat::OpencodeJson,
        };
        let r = write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        assert!(matches!(r.status, SlotStatus::Installed));
        let written = std::fs::read_to_string(slot.absolute(tmp.path())).unwrap();
        let parsed: Value = serde_json::from_str(&written).unwrap();
        let arr = parsed["instructions"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0].as_str().unwrap().starts_with(JSON_TAG_PREFIX));
    }

    #[test]
    fn opencode_json_upsert_replaces_prior_version() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "opencode",
            rel_path: ".config/opencode/opencode.json",
            format: SlotFormat::OpencodeJson,
        };
        let path = slot.absolute(tmp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let initial = json!({
            "instructions": [
                "[harvey:infect-global v6] old body",
                "user note"
            ]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let r = write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        assert_eq!(r.status, SlotStatus::Updated);

        let written = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&written).unwrap();
        let arr = parsed["instructions"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(
            arr[0]
                .as_str()
                .unwrap()
                .contains(&format!("[harvey:infect-global v{}]", BLOCK_VERSION))
        );
        assert_eq!(arr[1].as_str().unwrap(), "user note");
    }

    #[test]
    fn expand_tilde_resolves_home() {
        let p = expand_tilde("~/foo/bar").unwrap();
        assert!(p.starts_with(dirs::home_dir().unwrap()));
        assert!(p.ends_with("foo/bar"));
    }

    #[test]
    fn remove_markdown_block_strips_the_block() {
        let block = render_markdown_block(BODY);
        let existing = format!("user prose above\n\n{}\n\nuser prose below\n", block);
        let (out, removed) = remove_markdown_block(&existing);
        assert!(removed);
        assert!(!out.contains("harvey:infect-global"));
        assert!(out.contains("user prose above"));
        assert!(out.contains("user prose below"));
    }

    #[test]
    fn remove_markdown_block_idempotent_when_absent() {
        let existing = "just user prose, no block\n";
        let (out, removed) = remove_markdown_block(existing);
        assert!(!removed);
        assert_eq!(out, existing);
    }

    #[test]
    fn remove_bootstrap_from_slot_deletes_file_when_only_block_was_present() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "test",
            rel_path: ".test/NOTES.md",
            format: SlotFormat::Markdown,
        };
        // First infect (normal path), then uninfect.
        write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        let path = slot.absolute(tmp.path());
        assert!(path.exists(), "infect should have created the slot");

        let result = remove_bootstrap_from_slot(&slot, tmp.path(), false);
        assert_eq!(result.status, SlotStatus::Updated);
        assert!(!path.exists(), "infect-only slot should be removed after uninfect");
    }

    #[test]
    fn remove_bootstrap_from_slot_preserves_user_prose() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "test",
            rel_path: ".test/MIXED.md",
            format: SlotFormat::Markdown,
        };
        let path = slot.absolute(tmp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Seed file with user prose, then infect on top of it, then uninfect.
        std::fs::write(&path, "my private project notes\n").unwrap();
        write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        let mid = std::fs::read_to_string(&path).unwrap();
        assert!(mid.contains("my private project notes"));
        assert!(mid.contains("harvey:infect-global"));

        let result = remove_bootstrap_from_slot(&slot, tmp.path(), false);
        assert_eq!(result.status, SlotStatus::Updated);
        assert!(path.exists(), "file with user prose must survive uninfect");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("my private project notes"));
        assert!(!after.contains("harvey:infect-global"));
    }

    #[test]
    fn remove_bootstrap_from_slot_dry_run_is_noop() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "test",
            rel_path: ".test/DRY.md",
            format: SlotFormat::Markdown,
        };
        write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        let path = slot.absolute(tmp.path());
        let before = std::fs::read_to_string(&path).unwrap();

        let result = remove_bootstrap_from_slot(&slot, tmp.path(), true);
        assert_eq!(result.status, SlotStatus::DryRun);
        // File untouched.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn remove_bootstrap_from_slot_unchanged_when_not_infected() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "test",
            rel_path: ".test/ABSENT.md",
            format: SlotFormat::Markdown,
        };
        let result = remove_bootstrap_from_slot(&slot, tmp.path(), false);
        assert_eq!(result.status, SlotStatus::Unchanged);
    }

    #[test]
    fn remove_bootstrap_from_opencode_slot_drops_tagged_entry() {
        let tmp = TempDir::new().unwrap();
        let slot = CliSlot {
            name: "opencode",
            rel_path: ".test/opencode.json",
            format: SlotFormat::OpencodeJson,
        };
        // Infect the opencode slot, confirm the entry landed.
        write_bootstrap_to_slot(&slot, BODY, tmp.path(), false);
        let path = slot.absolute(tmp.path());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(JSON_TAG_FINGERPRINT));

        let result = remove_bootstrap_from_slot(&slot, tmp.path(), false);
        assert_eq!(result.status, SlotStatus::Updated);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains(JSON_TAG_FINGERPRINT));
    }
}
