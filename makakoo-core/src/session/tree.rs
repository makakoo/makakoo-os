//! JSONL session tree — Rust port of pi's fork/label/rewind format.
//!
//! v0.2 Phase G.1. Behind feature flag `kernel.session_tree` (default off).
//! The session tree is append-only: every entry lands as one JSON line in
//! `$MAKAKOO_HOME/data/sessions/<session_id>.jsonl`. Callers navigate the
//! tree via `parent_id` pointers — no in-memory mutation needed.
//!
//! Entry types mirror pi's `packages/coding-agent/src/core/session-manager.ts`
//! so a future pi-harvey bridge can read either side's format interchangeably.
//!
//! Contract (version = 1):
//!   * entry.id is a stable string (uuid-ish); callers never re-use ids
//!   * entry.parent_id == None is reserved for the `session` header
//!   * every non-header entry must reference an existing earlier id as parent
//!   * appending is monotonic — reading back preserves insertion order

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Every session-tree entry has a `type` discriminant so callers can
/// pattern-match against the variant they care about. Matching pi's
/// enum exactly simplifies interop.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    /// Session header — exactly one per file, always the first line.
    /// `parent_session` is set when this session was forked from another.
    Session {
        id: String,
        created_at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fork_from_entry: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Human-readable message — user or assistant turn.
    Message {
        id: String,
        #[serde(default, rename = "parentId", skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        role: MessageRole,
        content: String,
        ts: DateTime<Utc>,
    },
    /// Compaction checkpoint — the agent summarized everything up to
    /// `first_kept_entry_id`, reducing the active context from
    /// `tokens_before` down to `tokens_after`.
    Compaction {
        id: String,
        #[serde(rename = "parentId")]
        parent_id: String,
        #[serde(rename = "firstKeptEntryId")]
        first_kept_entry_id: String,
        #[serde(rename = "tokensBefore")]
        tokens_before: u64,
        #[serde(rename = "tokensAfter")]
        tokens_after: u64,
        ts: DateTime<Utc>,
    },
    /// Summary of a branch produced by `session fork`. Stored on the
    /// parent session so tooling can show "this entry has 2 child branches".
    BranchSummary {
        id: String,
        #[serde(rename = "parentId")]
        parent_id: String,
        branch_session_id: String,
        summary: String,
        ts: DateTime<Utc>,
    },
    /// Arbitrary structured data bag — used by extensions to record plan
    /// artifacts, tool-call transcripts, etc. Payload is opaque JSON.
    Custom {
        id: String,
        #[serde(rename = "parentId")]
        parent_id: String,
        kind: String,
        payload: serde_json::Value,
        ts: DateTime<Utc>,
    },
    /// Custom message variant — same shape as Message but flagged with
    /// a `kind` discriminator so consumers can tell apart e.g.
    /// `tool_result` from a plain assistant reply.
    CustomMessage {
        id: String,
        #[serde(rename = "parentId")]
        parent_id: String,
        kind: String,
        role: MessageRole,
        content: String,
        ts: DateTime<Utc>,
    },
    /// Human-readable checkpoint marker — `session label <id> <name>`
    /// writes one of these so `session rewind <name>` has somewhere to
    /// point at.
    Label {
        id: String,
        #[serde(rename = "parentId")]
        parent_id: String,
        name: String,
        ts: DateTime<Utc>,
    },
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Entry::Session { id, .. }
            | Entry::Message { id, .. }
            | Entry::Compaction { id, .. }
            | Entry::BranchSummary { id, .. }
            | Entry::Custom { id, .. }
            | Entry::CustomMessage { id, .. }
            | Entry::Label { id, .. } => id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Entry::Session { .. } => None,
            Entry::Message { parent_id, .. } => parent_id.as_deref(),
            Entry::Compaction { parent_id, .. }
            | Entry::BranchSummary { parent_id, .. }
            | Entry::Custom { parent_id, .. }
            | Entry::CustomMessage { parent_id, .. }
            | Entry::Label { parent_id, .. } => Some(parent_id.as_str()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// Errors surfacing from session tree operations.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json error at line {line} of {path}: {source}")]
    Json {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("session {id} missing required header as first entry")]
    MissingHeader { id: String },
    #[error("session {id} header points at a different id ({found})")]
    HeaderIdMismatch { id: String, found: String },
    #[error("entry {child} references unknown parent {parent}")]
    DanglingParent { child: String, parent: String },
}

pub type Result<T> = std::result::Result<T, SessionError>;

/// Handle to one `<id>.jsonl` file. Cheap to clone — no file handle held;
/// every operation opens and drops the file so writes are durable at the
/// fs layer even without explicit fsync (we flush per write).
#[derive(Debug, Clone)]
pub struct SessionTree {
    root: PathBuf,
    id: String,
}

impl SessionTree {
    /// Path resolver: given `$MAKAKOO_HOME/data/sessions/` root and an id,
    /// returns `<root>/<id>.jsonl`. `root` is created if missing.
    pub fn new(root: impl Into<PathBuf>, id: impl Into<String>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|source| SessionError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(Self {
            root,
            id: id.into(),
        })
    }

    pub fn path(&self) -> PathBuf {
        self.root.join(format!("{}.jsonl", self.id))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn exists(&self) -> bool {
        self.path().is_file()
    }

    /// Append one entry. Creates the file on first call. Flushes before
    /// returning so a crash can't lose the most recent write.
    pub fn append(&self, entry: &Entry) -> Result<()> {
        let path = self.path();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| SessionError::Io {
                path: path.clone(),
                source,
            })?;
        let line = serde_json::to_string(entry).map_err(|source| SessionError::Json {
            path: path.clone(),
            line: 0,
            source,
        })?;
        f.write_all(line.as_bytes()).map_err(|source| SessionError::Io {
            path: path.clone(),
            source,
        })?;
        f.write_all(b"\n").map_err(|source| SessionError::Io {
            path: path.clone(),
            source,
        })?;
        f.flush().map_err(|source| SessionError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(())
    }

    /// Read every entry in insertion order. Validates:
    ///   * first entry is a Session header
    ///   * header id matches this tree's id
    ///   * every non-header parent_id references an earlier entry
    pub fn load(&self) -> Result<Vec<Entry>> {
        let path = self.path();
        let file = File::open(&path).map_err(|source| SessionError::Io {
            path: path.clone(),
            source,
        })?;
        let reader = BufReader::new(file);
        let mut entries: Vec<Entry> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (ix, line_res) in reader.lines().enumerate() {
            let line = line_res.map_err(|source| SessionError::Io {
                path: path.clone(),
                source,
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry = serde_json::from_str(&line).map_err(|source| SessionError::Json {
                path: path.clone(),
                line: ix,
                source,
            })?;

            if ix == 0 {
                match &entry {
                    Entry::Session { id, .. } if id == &self.id => {}
                    Entry::Session { id, .. } => {
                        return Err(SessionError::HeaderIdMismatch {
                            id: self.id.clone(),
                            found: id.clone(),
                        });
                    }
                    _ => {
                        return Err(SessionError::MissingHeader {
                            id: self.id.clone(),
                        });
                    }
                }
            }
            if let Some(parent) = entry.parent_id() {
                if !seen_ids.contains(parent) {
                    return Err(SessionError::DanglingParent {
                        child: entry.id().to_string(),
                        parent: parent.to_string(),
                    });
                }
            }
            seen_ids.insert(entry.id().to_string());
            entries.push(entry);
        }
        if entries.is_empty() {
            return Err(SessionError::MissingHeader {
                id: self.id.clone(),
            });
        }
        Ok(entries)
    }

    /// Return every ancestor of `target_id` up to and including the
    /// session header. Ancestors are returned child-first — reverse to
    /// get root-first order.
    pub fn ancestors(&self, target_id: &str) -> Result<Vec<Entry>> {
        let entries = self.load()?;
        let by_id: std::collections::HashMap<&str, &Entry> =
            entries.iter().map(|e| (e.id(), e)).collect();
        let mut chain: Vec<Entry> = Vec::new();
        let mut cursor = target_id.to_string();
        loop {
            let Some(&entry) = by_id.get(cursor.as_str()) else {
                break;
            };
            chain.push(entry.clone());
            match entry.parent_id() {
                Some(p) => cursor = p.to_string(),
                None => break,
            }
        }
        Ok(chain)
    }
}

/// Fork a session starting at `from_entry_id`. Copies every entry from
/// the source session up to (and including) the fork point into a new
/// session file, rewriting the Session header to point at the parent.
///
/// Non-destructive — the source session file is untouched. Returns the
/// new `SessionTree` handle.
pub fn fork(
    source: &SessionTree,
    new_id: impl Into<String>,
    from_entry_id: &str,
) -> Result<SessionTree> {
    let new_id = new_id.into();
    let entries = source.load()?;
    let dest = SessionTree::new(source.root.clone(), new_id.clone())?;

    // Write the fresh header.
    dest.append(&Entry::Session {
        id: new_id.clone(),
        created_at: Utc::now(),
        parent_session: Some(source.id().to_string()),
        fork_from_entry: Some(from_entry_id.to_string()),
        title: None,
    })?;

    // Copy ancestors of `from_entry_id` (including the fork point itself)
    // in root-first order — skipping the source header; our fresh header
    // above replaces it.
    let mut to_copy: Vec<&Entry> = entries
        .iter()
        .skip_while(|e| !matches!(e, Entry::Session { .. }))
        .skip(1) // skip source header
        .collect();
    // Walk ancestors of from_entry_id to find the keep-set.
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    let by_id: std::collections::HashMap<&str, &Entry> =
        entries.iter().map(|e| (e.id(), e)).collect();
    let mut cursor = Some(from_entry_id.to_string());
    while let Some(c) = cursor.take() {
        if !keep.insert(c.clone()) {
            break;
        }
        if let Some(entry) = by_id.get(c.as_str()) {
            if let Some(p) = entry.parent_id() {
                cursor = Some(p.to_string());
            }
        }
    }
    to_copy.retain(|e| keep.contains(e.id()));

    // When we rewrite a copied entry's parent pointer, any reference to
    // the source session header gets re-rooted at the new fork header so
    // the resulting file passes `load()`'s dangling-parent validation.
    let source_header_id = source.id().to_string();
    let new_header_id = new_id.clone();

    let rewrite_parent = |parent: &str| {
        if parent == source_header_id {
            new_header_id.clone()
        } else {
            parent.to_string()
        }
    };

    for e in to_copy {
        let remapped: Entry = match e {
            Entry::Message {
                id,
                parent_id,
                role,
                content,
                ts,
            } => Entry::Message {
                id: id.clone(),
                parent_id: parent_id.as_deref().map(rewrite_parent),
                role: *role,
                content: content.clone(),
                ts: *ts,
            },
            Entry::Compaction {
                id,
                parent_id,
                first_kept_entry_id,
                tokens_before,
                tokens_after,
                ts,
            } => Entry::Compaction {
                id: id.clone(),
                parent_id: rewrite_parent(parent_id),
                first_kept_entry_id: first_kept_entry_id.clone(),
                tokens_before: *tokens_before,
                tokens_after: *tokens_after,
                ts: *ts,
            },
            Entry::BranchSummary {
                id,
                parent_id,
                branch_session_id,
                summary,
                ts,
            } => Entry::BranchSummary {
                id: id.clone(),
                parent_id: rewrite_parent(parent_id),
                branch_session_id: branch_session_id.clone(),
                summary: summary.clone(),
                ts: *ts,
            },
            Entry::Custom {
                id,
                parent_id,
                kind,
                payload,
                ts,
            } => Entry::Custom {
                id: id.clone(),
                parent_id: rewrite_parent(parent_id),
                kind: kind.clone(),
                payload: payload.clone(),
                ts: *ts,
            },
            Entry::CustomMessage {
                id,
                parent_id,
                kind,
                role,
                content,
                ts,
            } => Entry::CustomMessage {
                id: id.clone(),
                parent_id: rewrite_parent(parent_id),
                kind: kind.clone(),
                role: *role,
                content: content.clone(),
                ts: *ts,
            },
            Entry::Label {
                id,
                parent_id,
                name,
                ts,
            } => Entry::Label {
                id: id.clone(),
                parent_id: rewrite_parent(parent_id),
                name: name.clone(),
                ts: *ts,
            },
            Entry::Session { .. } => continue, // skip any nested header
        };
        dest.append(&remapped)?;
    }
    Ok(dest)
}

/// Find a Label entry by its human-readable name. Returns the Label's
/// entry id, or None if no label with that name exists.
pub fn find_label(tree: &SessionTree, name: &str) -> Result<Option<String>> {
    let entries = tree.load()?;
    for e in entries {
        if let Entry::Label { id, name: n, .. } = e {
            if n == name {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

/// Rewind the session to a labeled checkpoint: copies every entry up
/// to and including the label into a fresh file, then atomically swaps
/// it in. The original file is preserved as `<id>.<rfc3339>.bak.jsonl`.
///
/// Non-destructive — `session rewind` should never lose data. Tooling
/// that asks "what was in my session before I rewound" has the .bak
/// file to consult.
pub fn rewind_to_label(tree: &SessionTree, label_name: &str) -> Result<usize> {
    let entries = tree.load()?;
    let label_id = find_label(tree, label_name)?.ok_or_else(|| SessionError::DanglingParent {
        child: format!("<rewind target: label {label_name:?}>"),
        parent: "(no such label)".into(),
    })?;

    // Build the keep-set — every entry up to AND including the label.
    let mut keep_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &entries {
        keep_ids.insert(e.id().to_string());
        if e.id() == label_id {
            break;
        }
    }

    let path = tree.path();
    let ts_suffix = Utc::now().format("%Y%m%dT%H%M%S%.3fZ").to_string();
    let backup = path.with_extension(format!("jsonl.{ts_suffix}.bak"));
    fs::copy(&path, &backup).map_err(|source| SessionError::Io {
        path: backup.clone(),
        source,
    })?;

    // Rewrite the file with only the kept entries.
    let tmp = path.with_extension("jsonl.rewind-tmp");
    {
        let mut out = File::create(&tmp).map_err(|source| SessionError::Io {
            path: tmp.clone(),
            source,
        })?;
        for e in &entries {
            if !keep_ids.contains(e.id()) {
                continue;
            }
            let line = serde_json::to_string(e).map_err(|source| SessionError::Json {
                path: tmp.clone(),
                line: 0,
                source,
            })?;
            out.write_all(line.as_bytes()).map_err(|source| SessionError::Io {
                path: tmp.clone(),
                source,
            })?;
            out.write_all(b"\n").map_err(|source| SessionError::Io {
                path: tmp.clone(),
                source,
            })?;
        }
        out.flush().map_err(|source| SessionError::Io {
            path: tmp.clone(),
            source,
        })?;
    }
    fs::rename(&tmp, &path).map_err(|source| SessionError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(keep_ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn session_header(id: &str) -> Entry {
        Entry::Session {
            id: id.to_string(),
            created_at: Utc::now(),
            parent_session: None,
            fork_from_entry: None,
            title: Some("test".into()),
        }
    }

    fn msg(id: &str, parent: &str, text: &str) -> Entry {
        Entry::Message {
            id: id.to_string(),
            parent_id: Some(parent.to_string()),
            role: MessageRole::User,
            content: text.to_string(),
            ts: Utc::now(),
        }
    }

    #[test]
    fn append_then_load_roundtrips_entries_in_order() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "sess-1").unwrap();
        tree.append(&session_header("sess-1")).unwrap();
        tree.append(&msg("m1", "sess-1", "hi")).unwrap();
        tree.append(&msg("m2", "m1", "there")).unwrap();

        let entries = tree.load().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id(), "sess-1");
        assert_eq!(entries[1].id(), "m1");
        assert_eq!(entries[2].id(), "m2");
    }

    #[test]
    fn load_rejects_missing_header() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "sess-1").unwrap();
        // Append a non-header entry first — invalid.
        tree.append(&msg("m1", "sess-1", "orphan")).unwrap();
        let err = tree.load().unwrap_err();
        assert!(matches!(err, SessionError::MissingHeader { .. }));
    }

    #[test]
    fn load_rejects_header_id_mismatch() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "sess-1").unwrap();
        // Header claims a different session id.
        tree.append(&session_header("wrong-id")).unwrap();
        let err = tree.load().unwrap_err();
        assert!(matches!(err, SessionError::HeaderIdMismatch { .. }));
    }

    #[test]
    fn load_rejects_dangling_parent() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "sess-1").unwrap();
        tree.append(&session_header("sess-1")).unwrap();
        // Reference a parent that doesn't exist yet.
        tree.append(&msg("m2", "m1-ghost", "oops")).unwrap();
        let err = tree.load().unwrap_err();
        assert!(matches!(err, SessionError::DanglingParent { .. }));
    }

    #[test]
    fn ancestors_returns_chain_child_first() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "sess-1").unwrap();
        tree.append(&session_header("sess-1")).unwrap();
        tree.append(&msg("m1", "sess-1", "a")).unwrap();
        tree.append(&msg("m2", "m1", "b")).unwrap();
        tree.append(&msg("m3", "m2", "c")).unwrap();

        let chain = tree.ancestors("m3").unwrap();
        let ids: Vec<&str> = chain.iter().map(|e| e.id()).collect();
        assert_eq!(ids, vec!["m3", "m2", "m1", "sess-1"]);
    }

    #[test]
    fn fork_creates_new_session_referencing_parent() {
        let tmp = TempDir::new().unwrap();
        let src = SessionTree::new(tmp.path(), "src-1").unwrap();
        src.append(&session_header("src-1")).unwrap();
        src.append(&msg("m1", "src-1", "one")).unwrap();
        src.append(&msg("m2", "m1", "two")).unwrap();
        src.append(&msg("m3", "m2", "three")).unwrap();

        let dest = fork(&src, "fork-1", "m2").unwrap();
        assert_eq!(dest.id(), "fork-1");
        assert!(dest.exists());

        let entries = dest.load().unwrap();
        // Header + ancestors of m2 in root-first order = header,m1,m2
        // The header differs but the ancestors of m2 are {sess-1, m1, m2},
        // we skipped sess-1 (source header) and kept m1 + m2 in insertion
        // order (they were emitted in the order we appended them).
        let ids: Vec<&str> = entries.iter().map(|e| e.id()).collect();
        assert_eq!(ids, vec!["fork-1", "m1", "m2"]);

        if let Entry::Session {
            parent_session,
            fork_from_entry,
            ..
        } = &entries[0]
        {
            assert_eq!(parent_session.as_deref(), Some("src-1"));
            assert_eq!(fork_from_entry.as_deref(), Some("m2"));
        } else {
            panic!("expected Session header");
        }
    }

    #[test]
    fn fork_does_not_mutate_source() {
        let tmp = TempDir::new().unwrap();
        let src = SessionTree::new(tmp.path(), "src").unwrap();
        src.append(&session_header("src")).unwrap();
        src.append(&msg("m1", "src", "a")).unwrap();
        src.append(&msg("m2", "m1", "b")).unwrap();

        let before = fs::read_to_string(src.path()).unwrap();
        let _ = fork(&src, "fork-nomutate", "m1").unwrap();
        let after = fs::read_to_string(src.path()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn label_and_custom_entry_types_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "labels").unwrap();
        tree.append(&session_header("labels")).unwrap();
        tree.append(&msg("m1", "labels", "hi")).unwrap();
        tree.append(&Entry::Label {
            id: "l1".into(),
            parent_id: "m1".into(),
            name: "pre-refactor".into(),
            ts: Utc::now(),
        })
        .unwrap();
        tree.append(&Entry::Custom {
            id: "c1".into(),
            parent_id: "m1".into(),
            kind: "tool_call".into(),
            payload: serde_json::json!({"tool": "brain_search", "args": {"q": "foo"}}),
            ts: Utc::now(),
        })
        .unwrap();

        let entries = tree.load().unwrap();
        assert_eq!(entries.len(), 4);
        assert!(matches!(entries[2], Entry::Label { .. }));
        assert!(matches!(entries[3], Entry::Custom { .. }));
    }

    #[test]
    fn rewind_to_label_preserves_history_up_to_label_and_backs_up() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "rw").unwrap();
        tree.append(&session_header("rw")).unwrap();
        tree.append(&msg("m1", "rw", "a")).unwrap();
        tree.append(&msg("m2", "m1", "b")).unwrap();
        tree.append(&Entry::Label {
            id: "lbl-checkpoint".into(),
            parent_id: "m2".into(),
            name: "checkpoint".into(),
            ts: Utc::now(),
        })
        .unwrap();
        tree.append(&msg("m3", "m2", "c — to be rewound")).unwrap();
        tree.append(&msg("m4", "m3", "d — to be rewound")).unwrap();

        let kept = rewind_to_label(&tree, "checkpoint").unwrap();
        assert_eq!(kept, 4); // header + m1 + m2 + label

        let entries = tree.load().unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id()).collect();
        assert_eq!(ids, vec!["rw", "m1", "m2", "lbl-checkpoint"]);

        // Backup file exists and contains the pre-rewind state.
        let entries_in_dir: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries_in_dir.iter().any(|n| n.contains(".bak")),
            "backup file not found; dir = {entries_in_dir:?}"
        );
    }

    #[test]
    fn rewind_unknown_label_errors() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "nolabel").unwrap();
        tree.append(&session_header("nolabel")).unwrap();
        tree.append(&msg("m1", "nolabel", "hi")).unwrap();
        let err = rewind_to_label(&tree, "missing").unwrap_err();
        assert!(matches!(err, SessionError::DanglingParent { .. }));
    }

    #[test]
    fn find_label_returns_id_when_present() {
        let tmp = TempDir::new().unwrap();
        let tree = SessionTree::new(tmp.path(), "findlbl").unwrap();
        tree.append(&session_header("findlbl")).unwrap();
        tree.append(&msg("m1", "findlbl", "hi")).unwrap();
        tree.append(&Entry::Label {
            id: "L1".into(),
            parent_id: "m1".into(),
            name: "pre-refactor".into(),
            ts: Utc::now(),
        })
        .unwrap();
        assert_eq!(
            find_label(&tree, "pre-refactor").unwrap(),
            Some("L1".into())
        );
        assert_eq!(find_label(&tree, "never-written").unwrap(), None);
    }

    #[test]
    fn append_is_append_only_across_handle_drops() {
        let tmp = TempDir::new().unwrap();
        {
            let t1 = SessionTree::new(tmp.path(), "persist").unwrap();
            t1.append(&session_header("persist")).unwrap();
            t1.append(&msg("a", "persist", "1")).unwrap();
        }
        {
            let t2 = SessionTree::new(tmp.path(), "persist").unwrap();
            t2.append(&msg("b", "a", "2")).unwrap();
            assert_eq!(t2.load().unwrap().len(), 3);
        }
    }
}
