//! Wiki subsystem — Logseq-shaped Brain page lint / compile / save.
//!
//! Python source of truth: `core/superbrain/wiki.py` (`WikiOps`). This
//! port carves out the three pure operations the Rust stack actually
//! needs today:
//!
//! * `save()` — atomic, fs2-locked write of a single page file. The
//!   canonical writer for every Rust-side surface that mutates a Brain
//!   page (MCP handlers, SANCHO tasks, CLI).
//! * `WikiLinter` — enforces the Logseq outliner conventions so drift
//!   (empty bullets, unbalanced wikilinks, misshapen journal filenames)
//!   is caught before it leaks into the FTS index.
//! * `WikiCompiler` — normalises loose markdown into a Logseq-ready
//!   bullet tree with optional property header injection.
//!
//! Python's heavier operations (`build_index`, `compile_journal`,
//! `detect_contradictions`, `log_op`) stay on the Python side for now —
//! they reach into the Brain layout and knowledge graph and are the
//! subject of a later wave. This module is deliberately side-effect-free
//! on the filesystem except for `save()`.

pub mod compile;
pub mod lint;

pub use compile::{CompileOptions, CompiledPage, WikiCompiler};
pub use lint::{LintIssue, LintReport, LintRule, WikiLinter};

use std::io::Write;
use std::path::Path;

use fs2::FileExt;

use crate::error::{MakakooError, Result};

/// Save a wiki page atomically with an exclusive file lock.
///
/// Writes `content` to `path` via a sibling `.tmpname.tmp` then renames
/// it over the target, matching the Python `wiki.py` pattern for journal
/// and page appends. The temp file holds an `fs2` exclusive lock while
/// it's being written so two concurrent Harvey processes can never tear
/// each other's bytes. The parent directory is created if missing.
pub fn save(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| MakakooError::internal("wiki::save path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .ok_or_else(|| MakakooError::internal("wiki::save path has no file name"))?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!(".{file_name}.tmp"));

    // Scope the file handle so it drops (and implicitly unlocks) before
    // the atomic rename — on macOS+APFS holding an exclusive advisory
    // lock across a rename is fine, but dropping the handle first keeps
    // us defensive against filesystems that reject rename on an open fd.
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.lock_exclusive()?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        // `unlock()` is fallible on some platforms but the handle drop
        // also unlocks, so we ignore the result to avoid double-unlock
        // noise on close.
        let _ = f.unlock();
    }

    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_writes_atomic_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pages").join("Harvey.md");
        save(&target, "- Harvey\n  - autonomous cognitive extension\n").unwrap();

        assert!(target.exists());
        let got = std::fs::read_to_string(&target).unwrap();
        assert_eq!(got, "- Harvey\n  - autonomous cognitive extension\n");
    }

    #[test]
    fn save_overwrites_existing_without_leftover_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        save(&target, "- first\n").unwrap();
        save(&target, "- second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "- second\n");

        // No .tmp sibling should survive a successful save.
        let sibling = dir.path().join(".note.md.tmp");
        assert!(!sibling.exists());
    }
}
