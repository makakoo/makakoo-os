//! Brain → SQLite indexer. Ports the `sync_brain` / `sync_file` /
//! `_embed_all` paths from `core/superbrain/store.py` +
//! `core/superbrain/superbrain.py`.
//!
//! Walks `data/Brain/{pages,journals}/*.md` and (optionally) the
//! `data/auto-memory/*.md` cross-CLI shared store, upserting each file
//! into `brain_docs` with a sha256 content_hash skip-when-unchanged
//! shortcut. Pruning + entity-graph rebuild happen at the end of every
//! full sync. Embedding is split out into a separate async helper so
//! the sync path stays cheap and synchronous.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::params;
use serde::Serialize;

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};
use crate::superbrain::graph::GraphStore;
use crate::superbrain::store::SuperbrainStore;

const MIN_CONTENT_CHARS: usize = 20;
const EMBED_TRUNCATE_CHARS: usize = 2000;

/// What happened to a single file during sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IngestResult {
    Page,
    Journal,
    Memory,
    Skipped,
    Errors,
}

/// Knobs the caller passes into `sync`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncOptions {
    /// Re-index every file regardless of stored content_hash.
    pub force: bool,
    /// Also index `data/auto-memory/*.md` (off when the dir is missing).
    pub include_auto_memory: bool,
}

/// Counters returned from a full sync.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncReport {
    pub pages: usize,
    pub journals: usize,
    pub memories: usize,
    pub skipped: usize,
    pub removed: usize,
    pub errors: usize,
    /// Populated by the optional `embed_pending` follow-up step.
    pub vectors: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
}

/// Sync engine — bundles the store + graph + brain root.
pub struct IngestEngine {
    store: Arc<SuperbrainStore>,
    graph: Arc<GraphStore>,
    brain_dir: PathBuf,
    auto_memory_dir: PathBuf,
}

impl IngestEngine {
    /// Build an engine rooted at `home`. Brain root resolves to
    /// `home/data/Brain`; auto-memory to `home/data/auto-memory`.
    pub fn new(store: Arc<SuperbrainStore>, graph: Arc<GraphStore>, home: &Path) -> Self {
        let brain_dir = home.join("data").join("Brain");
        let auto_memory_dir = home.join("data").join("auto-memory");
        Self {
            store,
            graph,
            brain_dir,
            auto_memory_dir,
        }
    }

    /// Override the Brain dir explicitly. Used by tests + by callers
    /// running ingest against a non-default location.
    pub fn with_brain_dir(mut self, brain_dir: PathBuf) -> Self {
        self.brain_dir = brain_dir;
        self
    }

    /// Override the auto-memory dir explicitly.
    pub fn with_auto_memory_dir(mut self, auto_memory_dir: PathBuf) -> Self {
        self.auto_memory_dir = auto_memory_dir;
        self
    }

    /// Full sync. Walks pages/, journals/, optionally auto-memory/.
    /// Skips files whose stored hash matches, prunes deleted rows,
    /// and rebuilds the entity graph at the end.
    pub fn sync(&self, opts: SyncOptions) -> Result<SyncReport> {
        let mut report = SyncReport::default();

        let existing = if opts.force {
            HashSet::new()
        } else {
            self.load_existing_paths()?
        };

        let mut seen: HashSet<String> = HashSet::new();

        let pages_dir = self.brain_dir.join("pages");
        if pages_dir.exists() {
            self.sync_dir(&pages_dir, "page", &existing, opts.force, &mut report, &mut seen)?;
        }
        let journals_dir = self.brain_dir.join("journals");
        if journals_dir.exists() {
            self.sync_dir(&journals_dir, "journal", &existing, opts.force, &mut report, &mut seen)?;
        }
        if opts.include_auto_memory && self.auto_memory_dir.exists() {
            self.sync_dir(
                &self.auto_memory_dir,
                "memory",
                &existing,
                opts.force,
                &mut report,
                &mut seen,
            )?;
        }

        report.removed = self.prune_unseen(&seen)?;
        self.rebuild_triples()?;
        let (n, e) = self.graph.rebuild_from_entity_graph()?;
        report.graph_nodes = n;
        report.graph_edges = e;
        Ok(report)
    }

    /// Rebuild the `entity_graph` triples table from `brain_docs.entities`.
    /// Mirrors Python `rebuild_entity_graph` so the materialised graph
    /// downstream (`GraphStore::rebuild_from_entity_graph`) sees the same
    /// (subject, links_to, object) shape.
    fn rebuild_triples(&self) -> Result<()> {
        let conn = self.store.conn_arc();
        let mut conn = conn.lock().expect("ingest conn poisoned");
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM entity_graph", [])?;
        let rows: Vec<(String, String, String, String)> = {
            let mut stmt =
                tx.prepare("SELECT name, doc_type, entities, path FROM brain_docs")?;
            let r = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            r
        };
        for (name, doc_type, entities_json, path) in rows {
            let entities: Vec<String> =
                serde_json::from_str(&entities_json).unwrap_or_default();
            let valid_from = if doc_type == "journal" {
                let stem: String = name.replace('_', "-").chars().take(10).collect();
                Some(stem)
            } else {
                None
            };
            for ent in entities {
                tx.execute(
                    "INSERT INTO entity_graph
                     (subject, predicate, object, valid_from, valid_to, confidence, source)
                     VALUES (?1, 'links_to', ?2, ?3, NULL, 1.0, ?4)",
                    params![name, ent, valid_from, path],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Index a single Brain file. Returns the slot it counted toward.
    pub fn sync_file(&self, path: &Path) -> Result<IngestResult> {
        let doc_type = doc_type_for(path).ok_or_else(|| {
            MakakooError::Config(format!(
                "{} is not under pages/ or journals/ — cannot infer doc_type",
                path.display()
            ))
        })?;
        let result = self.ingest_one(path, doc_type, None, true)?;
        let _ = self.rebuild_triples();
        let _ = self.graph.rebuild_from_entity_graph();
        Ok(result)
    }

    /// Embed up to `limit` documents that don't have vectors yet.
    /// Returns the number of vectors written.
    pub async fn embed_pending(
        &self,
        embedder: &EmbeddingClient,
        limit: usize,
    ) -> Result<usize> {
        let pending = self.store.docs_missing_vectors(limit)?;
        let mut written = 0usize;
        for (doc_id, content) in pending {
            let truncated: String = content.chars().take(EMBED_TRUNCATE_CHARS).collect();
            match embedder.embed(&truncated).await {
                Ok(vec) if !vec.is_empty() => {
                    if self.store.store_vector(&doc_id, &vec).is_ok() {
                        written += 1;
                    }
                }
                _ => continue,
            }
        }
        Ok(written)
    }

    // ───────── internal ─────────

    fn sync_dir(
        &self,
        dir: &Path,
        doc_type: &'static str,
        existing: &HashSet<String>,
        force: bool,
        report: &mut SyncReport,
        seen: &mut HashSet<String>,
    ) -> Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_indexable_md(&path) {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            seen.insert(path_str.clone());
            let known = if force {
                false
            } else {
                existing.contains(&path_str)
            };
            match self.ingest_one(&path, doc_type, Some(known), force)? {
                IngestResult::Page => report.pages += 1,
                IngestResult::Journal => report.journals += 1,
                IngestResult::Memory => report.memories += 1,
                IngestResult::Skipped => report.skipped += 1,
                IngestResult::Errors => report.errors += 1,
            }
        }
        Ok(())
    }

    /// Hash-skip + upsert one file. `existed` is a hint — when the
    /// caller already pulled the path/hash table we use it to skip the
    /// extra SELECT.
    fn ingest_one(
        &self,
        path: &Path,
        doc_type: &str,
        existed_hint: Option<bool>,
        force: bool,
    ) -> Result<IngestResult> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(IngestResult::Errors),
        };
        if content.trim().chars().count() < MIN_CONTENT_CHARS {
            return Ok(IngestResult::Skipped);
        }
        // Match `SuperbrainStore::write_document` exactly so the hash we
        // compare here is the same one it persists. Otherwise every
        // second sync run would re-write every doc.
        let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        if !force {
            let path_str = path.to_string_lossy().to_string();
            let stored = self.stored_hash(&path_str)?;
            if let Some(h) = stored {
                if h == content_hash {
                    return Ok(IngestResult::Skipped);
                }
            } else if existed_hint == Some(true) {
                // We thought it existed but the hash row was missing —
                // fall through and re-write.
            }
        }

        let path_str = path.to_string_lossy().to_string();
        let entities_meta = serde_json::Value::Array(
            extract_wikilinks(&content)
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        );
        self.store
            .write_document(&path_str, &content, doc_type, entities_meta)?;

        Ok(match doc_type {
            "page" => IngestResult::Page,
            "journal" => IngestResult::Journal,
            "memory" => IngestResult::Memory,
            _ => IngestResult::Errors,
        })
    }

    fn load_existing_paths(&self) -> Result<HashSet<String>> {
        let conn = self.store.conn_arc();
        let conn = conn.lock().expect("ingest conn poisoned");
        let mut stmt = conn.prepare("SELECT path FROM brain_docs")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().collect())
    }

    fn stored_hash(&self, path: &str) -> Result<Option<String>> {
        let conn = self.store.conn_arc();
        let conn = conn.lock().expect("ingest conn poisoned");
        let mut stmt = conn.prepare("SELECT content_hash FROM brain_docs WHERE path = ?1")?;
        let mut rows = stmt.query(params![path])?;
        if let Some(r) = rows.next()? {
            Ok(Some(r.get::<_, String>(0)?))
        } else {
            Ok(None)
        }
    }

    fn prune_unseen(&self, seen: &HashSet<String>) -> Result<usize> {
        let conn = self.store.conn_arc();
        let conn = conn.lock().expect("ingest conn poisoned");
        let all: Vec<String> = {
            let mut stmt = conn.prepare("SELECT path FROM brain_docs")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut removed = 0usize;
        for path in all {
            if !seen.contains(&path) {
                conn.execute("DELETE FROM brain_docs WHERE path = ?1", params![path])?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

// ───────── helpers ─────────

fn doc_type_for(path: &Path) -> Option<&'static str> {
    let s = path.to_string_lossy();
    if s.contains("/journals/") {
        Some("journal")
    } else if s.contains("/pages/") {
        Some("page")
    } else if s.contains("/auto-memory/") {
        Some("memory")
    } else {
        None
    }
}

fn is_indexable_md(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return false;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        // MEMORY.md is the auto-memory index, not a memory entry.
        if name == "MEMORY.md" {
            return false;
        }
    }
    true
}

fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let bytes = content.as_bytes();
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let mut end = start;
            while end + 1 < bytes.len() {
                if bytes[end] == b']' && bytes[end + 1] == b']' {
                    if let Ok(s) = std::str::from_utf8(&bytes[start..end]) {
                        let link = s.trim();
                        if !link.is_empty() && !out.iter().any(|x| x == link) {
                            out.push(link.to_string());
                        }
                    }
                    i = end + 2;
                    break;
                }
                end += 1;
            }
            if end + 1 >= bytes.len() {
                break;
            }
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use std::fs;
    use tempfile::tempdir;

    fn make_engine() -> (tempfile::TempDir, IngestEngine) {
        let dir = tempdir().unwrap();
        let db = dir.path().join("sb.db");
        let conn = open_db(&db).unwrap();
        run_migrations(&conn).unwrap();
        drop(conn);
        let store = Arc::new(SuperbrainStore::open(&db).unwrap());
        let graph = Arc::new(GraphStore::new(store.conn_arc()));
        let engine = IngestEngine::new(store, graph, dir.path());
        (dir, engine)
    }

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn extract_wikilinks_picks_unique_targets() {
        let body = "linking to [[Sprint 002]] and [[Sprint 002]] then [[Harvey]]";
        let out = extract_wikilinks(body);
        assert_eq!(out, vec!["Sprint 002".to_string(), "Harvey".to_string()]);
    }

    #[test]
    fn doc_type_for_recognises_path_segments() {
        assert_eq!(doc_type_for(Path::new("/x/data/Brain/pages/a.md")), Some("page"));
        assert_eq!(doc_type_for(Path::new("/x/data/Brain/journals/2026_04_18.md")), Some("journal"));
        assert_eq!(doc_type_for(Path::new("/x/data/auto-memory/foo.md")), Some("memory"));
        assert_eq!(doc_type_for(Path::new("/x/random/file.md")), None);
    }

    #[test]
    fn is_indexable_md_filters_memory_index() {
        assert!(!is_indexable_md(Path::new("/x/data/auto-memory/MEMORY.md")));
        assert!(is_indexable_md(Path::new("/x/data/auto-memory/foo.md")));
        assert!(!is_indexable_md(Path::new("/x/data/Brain/pages/foo.txt")));
    }

    #[test]
    fn sync_indexes_pages_and_journals_then_skips_unchanged() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        write(&brain.join("pages").join("Tytus.md"), "# Tytus\nlinks to [[Harvey]] and [[Makakoo]]");
        write(&brain.join("journals").join("2026_04_18.md"), "- worked on Sprint 006 phase 2");

        let report = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(report.pages, 1);
        assert_eq!(report.journals, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.removed, 0);
        assert!(report.graph_nodes > 0);

        // Second sync — content unchanged → skipped == 2, no new writes.
        let again = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(again.pages, 0);
        assert_eq!(again.journals, 0);
        assert_eq!(again.skipped, 2);
    }

    #[test]
    fn sync_force_reindexes_everything() {
        let (dir, engine) = make_engine();
        let pages = dir.path().join("data").join("Brain").join("pages");
        write(&pages.join("X.md"), "# X — body long enough to clear the min-chars threshold easily");
        let _ = engine.sync(SyncOptions::default()).unwrap();
        let forced = engine.sync(SyncOptions { force: true, include_auto_memory: false }).unwrap();
        assert_eq!(forced.pages, 1);
        assert_eq!(forced.skipped, 0);
    }

    #[test]
    fn sync_prunes_deleted_files() {
        let (dir, engine) = make_engine();
        let pages = dir.path().join("data").join("Brain").join("pages");
        let p1 = pages.join("Keep.md");
        let p2 = pages.join("Drop.md");
        write(&p1, "# Keep — body long enough to clear the min-chars threshold");
        write(&p2, "# Drop — body long enough to clear the min-chars threshold");
        engine.sync(SyncOptions::default()).unwrap();
        fs::remove_file(&p2).unwrap();
        let r = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(r.removed, 1);
    }

    #[test]
    fn sync_includes_auto_memory_when_requested() {
        let (dir, engine) = make_engine();
        let am = dir.path().join("data").join("auto-memory");
        write(&am.join("MEMORY.md"), "- index, must be skipped");
        write(&am.join("project_x.md"), "# Project X\n- body long enough to qualify for indexing");
        let r = engine.sync(SyncOptions { force: false, include_auto_memory: true }).unwrap();
        assert_eq!(r.memories, 1);
    }

    #[test]
    fn sync_file_indexes_single_journal_path() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        let path = brain.join("journals").join("2026_04_18.md");
        write(&path, "- single-file ingest test entry — long enough to stick");
        let result = engine.sync_file(&path).unwrap();
        assert_eq!(result, IngestResult::Journal);
    }

    #[test]
    fn sync_file_rejects_paths_outside_known_subdirs() {
        let (dir, engine) = make_engine();
        let stray = dir.path().join("loose.md");
        write(&stray, "- not under pages/journals/auto-memory — should error");
        assert!(engine.sync_file(&stray).is_err());
    }
}
