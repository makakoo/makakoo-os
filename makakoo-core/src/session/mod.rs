//! v0.2 Phase G — JSONL session-tree subsystem.
//!
//! Entry types + append/read/fork plumbing land first (G.1). Label +
//! rewind (G.3), CLI wiring (G.2/G.4), and feature-flag integration
//! with the agent event loop (G.5) follow in subsequent commits.
//!
//! Default: OFF. The kernel only instantiates this subsystem when
//! `kernel.session_tree = true` in config. `rewind` is non-destructive —
//! the original file is kept as `<id>.<ts>.bak.jsonl` so nothing is lost.

pub mod export;
pub mod tree;

pub use tree::{find_label, fork, rewind_to_label, Entry, MessageRole, SessionError, SessionTree};

use std::path::{Path, PathBuf};

/// Canonical on-disk root for session-tree JSONL files.
///
/// `$MAKAKOO_HOME/data/sessions/`. Created on demand by `SessionTree::new`.
/// Kept here (rather than inside the CLI) so the MCP server, SANCHO
/// tasks, and future daemons all resolve the same path.
pub fn sessions_root(home: &Path) -> PathBuf {
    home.join("data").join("sessions")
}

/// List the session ids that currently live under [`sessions_root`].
///
/// Ignores entries that aren't `<id>.jsonl` (backups, .bak suffixes,
/// scratch files). Returns an empty vec if the dir is missing — a
/// fresh install has no sessions yet.
pub fn list_sessions(home: &Path) -> std::io::Result<Vec<String>> {
    let root = sessions_root(home);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Skip anything that isn't a live `<id>.jsonl`.
        if let Some(id) = name.strip_suffix(".jsonl") {
            // Filter out rewind/backup artifacts that embed `.`s but
            // never live as a valid session id on their own.
            if !id.contains(".bak") && !id.contains(".rewind-tmp") {
                ids.push(id.to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod mod_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_sessions_on_empty_dir_returns_empty() {
        let dir = tempdir().unwrap();
        assert!(list_sessions(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn list_sessions_ignores_backups() {
        let dir = tempdir().unwrap();
        let root = sessions_root(dir.path());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("alpha.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("beta.jsonl"), "{}\n").unwrap();
        std::fs::write(root.join("alpha.jsonl.20260421T000000.000Z.bak"), "{}\n").unwrap();
        let ids = list_sessions(dir.path()).unwrap();
        assert_eq!(ids, vec!["alpha".to_string(), "beta".to_string()]);
    }
}
