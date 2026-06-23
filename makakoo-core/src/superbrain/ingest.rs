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
use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingClient;
use crate::error::{MakakooError, Result};
use crate::superbrain::graph::GraphStore;
use crate::superbrain::store::{SourceMetadata, SuperbrainStore};

const MIN_CONTENT_CHARS: usize = 20;
const EMBED_TRUNCATE_CHARS: usize = 2000;
const EDGE_ENTITY_PREFIX: &str = "__makakoo_edge__|";

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
    home: PathBuf,
    brain_dir: PathBuf,
    auto_memory_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct BrainSourcesFile {
    #[serde(default)]
    canonical: Option<String>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    sources: Vec<BrainSourceEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct BrainSourceEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(rename = "type", default)]
    source_type: String,
    #[serde(default)]
    path: String,
}

#[derive(Debug, Clone)]
struct SourceSpec {
    name: String,
    role: String,
    source_type: String,
    root: PathBuf,
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
            home: home.to_path_buf(),
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
        let mut prune_scopes: HashSet<(String, String)> = HashSet::new();

        for source in self.load_sources()? {
            if !source.root.exists() {
                continue;
            }
            if source.role == "canonical" {
                let pages_dir = source.root.join("pages");
                if pages_dir.exists() {
                    if self.sync_dir(
                        &pages_dir,
                        "page",
                        &source,
                        &existing,
                        opts.force,
                        &mut report,
                        &mut seen,
                    )? {
                        prune_scopes.insert((source.name.clone(), "page".to_string()));
                    }
                }
                let journals_dir = source.root.join("journals");
                if journals_dir.exists() {
                    if self.sync_dir(
                        &journals_dir,
                        "journal",
                        &source,
                        &existing,
                        opts.force,
                        &mut report,
                        &mut seen,
                    )? {
                        prune_scopes.insert((source.name.clone(), "journal".to_string()));
                    }
                }
            } else {
                if self.sync_dir(
                    &source.root,
                    "page",
                    &source,
                    &existing,
                    opts.force,
                    &mut report,
                    &mut seen,
                )? {
                    prune_scopes.insert((source.name.clone(), "page".to_string()));
                }
            }
        }
        if opts.include_auto_memory && self.auto_memory_dir.exists() {
            let memory_source = SourceSpec {
                name: "auto-memory".to_string(),
                role: "canonical".to_string(),
                source_type: "auto-memory".to_string(),
                root: self.auto_memory_dir.clone(),
            };
            if self.sync_dir(
                &self.auto_memory_dir,
                "memory",
                &memory_source,
                &existing,
                opts.force,
                &mut report,
                &mut seen,
            )? {
                prune_scopes.insert((memory_source.name.clone(), "memory".to_string()));
            }
        }

        report.removed = self.prune_unseen(&seen, &prune_scopes)?;
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
            let mut stmt = tx.prepare("SELECT name, doc_type, entities, path FROM brain_docs")?;
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
            let entities: Vec<String> = serde_json::from_str(&entities_json).unwrap_or_default();
            let valid_from = if doc_type == "journal" {
                let stem: String = name.replace('_', "-").chars().take(10).collect();
                Some(stem)
            } else {
                None
            };
            for ent in entities {
                let (predicate, object) = decode_edge_entity(&ent)
                    .unwrap_or_else(|| ("links_to".to_string(), ent.clone()));
                tx.execute(
                    "INSERT INTO entity_graph
                     (subject, predicate, object, valid_from, valid_to, confidence, source)
                     VALUES (?1, ?2, ?3, ?4, NULL, 1.0, ?5)",
                    params![name, predicate, object, valid_from, path],
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
        let source = if doc_type == "memory" {
            SourceSpec {
                name: "auto-memory".to_string(),
                role: "canonical".to_string(),
                source_type: "auto-memory".to_string(),
                root: self.auto_memory_dir.clone(),
            }
        } else {
            SourceSpec {
                name: "default".to_string(),
                role: "canonical".to_string(),
                source_type: "logseq".to_string(),
                root: self.brain_dir.clone(),
            }
        };
        let result = self.ingest_one(path, doc_type, &source, None, true)?;
        let _ = self.rebuild_triples();
        let _ = self.graph.rebuild_from_entity_graph();
        Ok(result)
    }

    /// Embed up to `limit` documents that don't have vectors yet.
    /// Returns the number of vectors written.
    pub async fn embed_pending(&self, embedder: &EmbeddingClient, limit: usize) -> Result<usize> {
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
        source: &SourceSpec,
        existing: &HashSet<String>,
        force: bool,
        report: &mut SyncReport,
        seen: &mut HashSet<String>,
    ) -> Result<bool> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };
        let mut fully_enumerated = true;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    fully_enumerated = false;
                    continue;
                }
            };
            let path = entry.path();
            if path.is_dir() {
                // Recurse into subdirectories (e.g. pages/ai-impact/).
                // Skip app/VCS internals.
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.')
                        || name == "logseq"
                        || name == "bak"
                        || name == ".git"
                        || name == ".trash"
                    {
                        continue;
                    }
                }
                if !self.sync_dir(&path, doc_type, source, existing, force, report, seen)? {
                    fully_enumerated = false;
                }
                continue;
            }
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
            match self.ingest_one(&path, doc_type, source, Some(known), force)? {
                IngestResult::Page => report.pages += 1,
                IngestResult::Journal => report.journals += 1,
                IngestResult::Memory => report.memories += 1,
                IngestResult::Skipped => report.skipped += 1,
                IngestResult::Errors => report.errors += 1,
            }
        }
        Ok(fully_enumerated)
    }

    fn load_sources(&self) -> Result<Vec<SourceSpec>> {
        let config_path = self.home.join("config").join("brain_sources.json");
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        let canonical_root = self.brain_dir.clone();

        if config_path.exists() {
            let raw = std::fs::read_to_string(&config_path)?;
            if let Ok(file) = serde_json::from_str::<BrainSourcesFile>(&raw) {
                let canonical_name = file
                    .canonical
                    .or(file.default)
                    .unwrap_or_else(|| "default".to_string());
                for entry in file.sources {
                    if entry.name.is_empty() || entry.path.is_empty() {
                        continue;
                    }
                    let root = expand_path(&entry.path, &self.home);
                    let role = if entry.name == "default"
                        || (entry.name == canonical_name && same_pathish(&root, &canonical_root))
                    {
                        "canonical".to_string()
                    } else {
                        entry.role.unwrap_or_else(|| "enrichment".to_string())
                    };
                    let source_type = if entry.source_type.is_empty() {
                        "plain".to_string()
                    } else {
                        entry.source_type
                    };
                    if seen.insert(entry.name.clone()) {
                        sources.push(SourceSpec {
                            name: entry.name,
                            role,
                            source_type,
                            root,
                        });
                    }
                }
            }
        }

        if !seen.contains("default") {
            sources.insert(
                0,
                SourceSpec {
                    name: "default".to_string(),
                    role: "canonical".to_string(),
                    source_type: "logseq".to_string(),
                    root: canonical_root,
                },
            );
        }
        for source in &mut sources {
            if source.name == "default" {
                source.role = "canonical".to_string();
                source.source_type = "logseq".to_string();
                source.root = self.brain_dir.clone();
            } else if source.role == "canonical" {
                // Canonical Brain is fixed. Legacy configs that promoted an
                // external vault are treated as enrichment for safety.
                source.role = "enrichment".to_string();
            }
        }
        sources.retain(|source| {
            source.name == "default"
                || source.role == "canonical"
                || !paths_overlapish(&source.root, &self.brain_dir)
        });
        Ok(sources)
    }

    /// Hash-skip + upsert one file. `existed` is a hint — when the
    /// caller already pulled the path/hash table we use it to skip the
    /// extra SELECT.
    fn ingest_one(
        &self,
        path: &Path,
        doc_type: &str,
        source: &SourceSpec,
        existed_hint: Option<bool>,
        force: bool,
    ) -> Result<IngestResult> {
        let raw_content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(IngestResult::Errors),
        };
        let (content, entities) = if is_canvas_file(path) {
            render_canvas_for_index(&raw_content, path)
                .unwrap_or_else(|| (raw_content.clone(), extract_entities(&raw_content)))
        } else {
            (raw_content, Vec::new())
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
            if entities.is_empty() {
                extract_entities(&content)
            } else {
                entities
            }
            .into_iter()
            .map(serde_json::Value::String)
            .collect(),
        );
        let relative_path = path
            .strip_prefix(&source.root)
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        self.store.write_document_with_source(
            &path_str,
            &content,
            doc_type,
            entities_meta,
            SourceMetadata {
                source_name: source.name.clone(),
                source_type: source.source_type.clone(),
                source_role: source.role.clone(),
                relative_path,
            },
        )?;

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

    fn prune_unseen(
        &self,
        seen: &HashSet<String>,
        prune_scopes: &HashSet<(String, String)>,
    ) -> Result<usize> {
        if prune_scopes.is_empty() {
            return Ok(0);
        }
        let conn = self.store.conn_arc();
        let conn = conn.lock().expect("ingest conn poisoned");
        let all: Vec<(i64, String, String, String, String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, path, name, content, entities, source_name, doc_type FROM brain_docs",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?
                            .unwrap_or_else(|| "default".to_string()),
                        r.get::<_, String>(6)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let mut removed = 0usize;
        for (id, path, name, content, entities, source_name, doc_type) in all {
            if prune_scopes.contains(&(source_name, doc_type)) && !seen.contains(&path) {
                // Mirror the delete into the external-content FTS5 shadow,
                // clear the FK-dependent vector row, then drop the doc.
                conn.execute(
                    "INSERT INTO brain_fts(brain_fts, rowid, name, content, entities)
                     VALUES ('delete', ?1, ?2, ?3, ?4)",
                    params![id, name, content, entities],
                )?;
                conn.execute("DELETE FROM brain_vectors WHERE doc_id = ?1", params![id])?;
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

fn expand_path(raw: &str, home: &Path) -> PathBuf {
    let mut s = raw.to_string();
    if let Some(home_s) = home.to_str() {
        s = s.replace("$MAKAKOO_HOME", home_s);
        s = s.replace("$HARVEY_HOME", home_s);
    }
    if s == "~" {
        return std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.to_path_buf());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|_| home.join(rest));
    }
    let path = PathBuf::from(s);
    if path.is_absolute() {
        path
    } else {
        home.join(path)
    }
}

fn same_pathish(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ac), Ok(bc)) => ac == bc,
        _ => false,
    }
}

fn paths_overlapish(a: &Path, b: &Path) -> bool {
    if same_pathish(a, b) {
        return true;
    }
    let a = normalize_pathish(a);
    let b = normalize_pathish(b);
    a.starts_with(&b) || b.starts_with(&a)
}

fn normalize_pathish(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_indexable_md(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str());
    if !matches!(ext, Some("md") | Some("canvas")) {
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

fn is_canvas_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("canvas")
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

fn extract_entities(content: &str) -> Vec<String> {
    let mut out = extract_wikilinks(content);
    for tag in extract_tags(content) {
        push_unique(&mut out, format!("#{tag}"));
    }
    for item in extract_frontmatter_values(content, &["aliases", "alias"]) {
        push_unique(&mut out, item);
    }
    for item in extract_frontmatter_values(content, &["tags", "tag"]) {
        let clean = item.trim_start_matches('#').to_string();
        if !clean.is_empty() {
            push_unique(&mut out, format!("#{clean}"));
        }
    }
    out
}

fn encode_edge_entity(predicate: &str, object: &str) -> Option<String> {
    let object = clean_entity_label(object);
    if object.is_empty() {
        return None;
    }
    let predicate = sanitize_predicate(predicate);
    Some(format!("{EDGE_ENTITY_PREFIX}{predicate}|{object}"))
}

fn decode_edge_entity(entity: &str) -> Option<(String, String)> {
    let rest = entity.strip_prefix(EDGE_ENTITY_PREFIX)?;
    let (predicate, object) = rest.split_once('|')?;
    let predicate = sanitize_predicate(predicate);
    let object = clean_entity_label(object);
    if object.is_empty() {
        None
    } else {
        Some((predicate, object))
    }
}

fn sanitize_predicate(raw: &str) -> String {
    let mut out = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_');
    if out.is_empty() {
        "canvas_link".to_string()
    } else if out.starts_with("canvas_") {
        out.to_string()
    } else {
        format!("canvas_{out}")
    }
}

fn clean_entity_label(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches("[[")
        .trim_end_matches("]]")
        .trim()
        .chars()
        .take(80)
        .collect::<String>()
}

fn render_canvas_for_index(raw: &str, path: &Path) -> Option<(String, Vec<String>)> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;
    let nodes = obj
        .get("nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let edges = obj
        .get("edges")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Canvas");
    let mut lines = vec![format!("# {title} Canvas")];
    let mut entities = Vec::new();
    let mut labels = std::collections::HashMap::new();

    if !nodes.is_empty() {
        lines.push("## Nodes".to_string());
    }
    for node in nodes {
        let Some(node_obj) = node.as_object() else {
            continue;
        };
        let id = node_obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let Some(label) = canvas_node_label(node_obj) else {
            continue;
        };
        if !id.is_empty() {
            labels.insert(id, label.clone());
        }
        lines.push(format!("- {label}"));
        push_unique(&mut entities, label);
    }

    if !edges.is_empty() {
        lines.push("## Edges".to_string());
    }
    for edge in edges {
        let Some(edge_obj) = edge.as_object() else {
            continue;
        };
        let from = edge_obj
            .get("fromNode")
            .and_then(|v| v.as_str())
            .and_then(|id| labels.get(id))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let to = edge_obj
            .get("toNode")
            .and_then(|v| v.as_str())
            .and_then(|id| labels.get(id))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        if to == "unknown" {
            continue;
        }
        let label = edge_obj
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("link");
        lines.push(format!("- {from} --{label}--> {to}"));
        push_unique(&mut entities, to.clone());
        if let Some(edge_entity) = encode_edge_entity(label, &to) {
            push_unique(&mut entities, edge_entity);
        }
    }

    Some((lines.join("\n"), entities))
}

fn canvas_node_label(node: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let node_type = node.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let raw = match node_type {
        "file" => node
            .get("file")
            .and_then(|v| v.as_str())
            .and_then(|file| {
                Path::new(file)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .or(Some(file))
            })
            .unwrap_or(""),
        "link" => node
            .get("url")
            .and_then(|v| v.as_str())
            .or_else(|| node.get("label").and_then(|v| v.as_str()))
            .unwrap_or(""),
        "group" => node.get("label").and_then(|v| v.as_str()).unwrap_or(""),
        _ => node
            .get("text")
            .and_then(|v| v.as_str())
            .and_then(first_nonempty_line)
            .or_else(|| node.get("label").and_then(|v| v.as_str()))
            .unwrap_or(""),
    };
    let label = clean_entity_label(raw.trim_start_matches('#').trim());
    if label.is_empty() {
        None
    } else {
        Some(label)
    }
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn push_unique(out: &mut Vec<String>, value: String) {
    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if !value.is_empty() && !out.iter().any(|x| x == &value) {
        out.push(value);
    }
}

fn extract_tags(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = content.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch != '#' {
            continue;
        }
        if idx > 0 {
            let prev = content[..idx].chars().next_back().unwrap_or(' ');
            if !prev.is_whitespace() && prev != '(' && prev != '[' {
                continue;
            }
        }
        let start = idx + 1;
        let mut end = start;
        while let Some((j, c)) = chars.peek().copied() {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '/' {
                end = j + c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        if end > start {
            let tag = content[start..end].to_string();
            if !out.iter().any(|x| x == &tag) {
                out.push(tag);
            }
        }
    }
    out
}

fn extract_frontmatter_values(content: &str, keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(rest) = content.strip_prefix("---") else {
        return out;
    };
    let Some(end) = rest.find("\n---") else {
        return out;
    };
    let fm = &rest[..end];
    let mut active_key: Option<String> = None;
    for raw in fm.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("- ") {
            if active_key
                .as_deref()
                .is_some_and(|k| keys.iter().any(|wanted| wanted == &k))
            {
                push_unique(&mut out, stripped.to_string());
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            active_key = None;
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        active_key = Some(key.clone());
        if !keys.iter().any(|wanted| wanted == &key.as_str()) {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if value.starts_with('[') && value.ends_with(']') {
            for part in value.trim_matches(&['[', ']'][..]).split(',') {
                push_unique(&mut out, part.to_string());
            }
        } else {
            push_unique(&mut out, value.to_string());
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

    fn write_sources_config(home: &Path, external: &Path) {
        let cfg = home.join("config");
        fs::create_dir_all(&cfg).unwrap();
        let body = serde_json::json!({
            "canonical": "default",
            "default": "default",
            "sources": [
                {"name": "default", "role": "canonical", "type": "logseq", "path": "$MAKAKOO_HOME/data/Brain", "writable": true},
                {"name": "obsidian", "role": "enrichment", "type": "obsidian", "path": external.to_string_lossy(), "writable": false}
            ]
        });
        fs::write(
            cfg.join("brain_sources.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn extract_wikilinks_picks_unique_targets() {
        let body = "linking to [[Sprint 002]] and [[Sprint 002]] then [[Harvey]]";
        let out = extract_wikilinks(body);
        assert_eq!(out, vec!["Sprint 002".to_string(), "Harvey".to_string()]);
    }

    #[test]
    fn extract_entities_includes_obsidian_tags_and_aliases() {
        let body = "---\naliases: [HoCa Client, EKSI]\ntags:\n  - project/hoca\n---\n# Note\nLinks [[Makakoo OS]] #status/active";
        let out = extract_entities(body);
        assert!(out.contains(&"Makakoo OS".to_string()));
        assert!(out.contains(&"HoCa Client".to_string()));
        assert!(out.contains(&"EKSI".to_string()));
        assert!(out.contains(&"#project/hoca".to_string()));
        assert!(out.contains(&"#status/active".to_string()));
    }

    #[test]
    fn doc_type_for_recognises_path_segments() {
        assert_eq!(
            doc_type_for(Path::new("/x/data/Brain/pages/a.md")),
            Some("page")
        );
        assert_eq!(
            doc_type_for(Path::new("/x/data/Brain/journals/2026_04_18.md")),
            Some("journal")
        );
        assert_eq!(
            doc_type_for(Path::new("/x/data/auto-memory/foo.md")),
            Some("memory")
        );
        assert_eq!(doc_type_for(Path::new("/x/random/file.md")), None);
    }

    #[test]
    fn is_indexable_md_filters_memory_index() {
        assert!(!is_indexable_md(Path::new("/x/data/auto-memory/MEMORY.md")));
        assert!(is_indexable_md(Path::new("/x/data/auto-memory/foo.md")));
        assert!(is_indexable_md(Path::new("/x/data/Brain/pages/Map.canvas")));
        assert!(!is_indexable_md(Path::new("/x/data/Brain/pages/foo.txt")));
    }

    #[test]
    fn render_canvas_extracts_content_and_labeled_edge_entity() {
        let body = serde_json::json!({
            "nodes": [
                {"id": "a", "type": "text", "text": "Project Alpha canvas-marker\nDetails"},
                {"id": "b", "type": "file", "file": "Project Beta.md"}
            ],
            "edges": [
                {"id": "e", "fromNode": "a", "toNode": "b", "label": "depends_on"}
            ]
        })
        .to_string();
        let (content, entities) =
            render_canvas_for_index(&body, Path::new("/tmp/Roadmap.canvas")).unwrap();
        assert!(content.contains("Project Alpha canvas-marker"));
        assert!(content.contains("Project Alpha canvas-marker --depends_on--> Project Beta"));
        assert!(entities.contains(&"Project Beta".to_string()));
        assert!(entities.contains(&"__makakoo_edge__|canvas_depends_on|Project Beta".to_string()));
    }

    #[test]
    fn sync_indexes_pages_and_journals_then_skips_unchanged() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        write(
            &brain.join("pages").join("Tytus.md"),
            "# Tytus\nlinks to [[Harvey]] and [[Makakoo]]",
        );
        write(
            &brain.join("journals").join("2026_04_18.md"),
            "- worked on Sprint 006 phase 2",
        );

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
        write(
            &pages.join("X.md"),
            "# X — body long enough to clear the min-chars threshold easily",
        );
        let _ = engine.sync(SyncOptions::default()).unwrap();
        let forced = engine
            .sync(SyncOptions {
                force: true,
                include_auto_memory: false,
            })
            .unwrap();
        assert_eq!(forced.pages, 1);
        assert_eq!(forced.skipped, 0);
    }

    #[test]
    fn sync_prunes_deleted_files() {
        let (dir, engine) = make_engine();
        let pages = dir.path().join("data").join("Brain").join("pages");
        let p1 = pages.join("Keep.md");
        let p2 = pages.join("Drop.md");
        write(
            &p1,
            "# Keep — body long enough to clear the min-chars threshold",
        );
        write(
            &p2,
            "# Drop — body long enough to clear the min-chars threshold",
        );
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
        write(
            &am.join("project_x.md"),
            "# Project X\n- body long enough to qualify for indexing",
        );
        let r = engine
            .sync(SyncOptions {
                force: false,
                include_auto_memory: true,
            })
            .unwrap();
        assert_eq!(r.memories, 1);
    }

    #[test]
    #[cfg(unix)]
    fn sync_file_indexes_single_journal_path() {
        // The journal-path detection uses POSIX-style path-segment
        // matching (`/journals/`) in the hot path. Windows uses
        // backslashes; Phase H.4 normalizes the detection or adds a
        // Windows-sibling test.
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        let path = brain.join("journals").join("2026_04_18.md");
        write(
            &path,
            "- single-file ingest test entry — long enough to stick",
        );
        let result = engine.sync_file(&path).unwrap();
        assert_eq!(result, IngestResult::Journal);
    }

    #[test]
    fn sync_file_rejects_paths_outside_known_subdirs() {
        let (dir, engine) = make_engine();
        let stray = dir.path().join("loose.md");
        write(
            &stray,
            "- not under pages/journals/auto-memory — should error",
        );
        assert!(engine.sync_file(&stray).is_err());
    }

    #[test]
    fn sync_indexes_pages_in_subdirectories() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        // Simulate pages/ai-impact/ subdirectory layout.
        let subdir = brain.join("pages").join("ai-impact");
        fs::create_dir_all(&subdir).unwrap();
        write(
            &subdir.join("hoCa-Technical-Spec.md"),
            "# hoCa Technical Spec\n\nTech proposal for ai.impact GmbH. Long enough to index.",
        );
        write(
            &subdir.join("ai-impact.md"),
            "# ai.impact\n\nCompany page for [[ai.impact]] client project. Long enough to index.",
        );
        let report = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(report.pages, 2, "subdirectory pages must be indexed");
    }

    #[test]
    fn sync_skips_logseq_bak_subdirectories() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        let bak = brain.join("pages").join("logseq").join("bak");
        fs::create_dir_all(&bak).unwrap();
        write(
            &bak.join("backup.md"),
            "# Backup\n\nThis should not be indexed — lives in logseq/bak/.",
        );
        let report = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(report.pages, 0, "logseq/bak files must be skipped");
    }

    #[test]
    fn sync_indexes_enrichment_source_with_source_labels() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        let external = dir.path().join("External Obsidian");
        write_sources_config(dir.path(), &external);
        write(
            &brain.join("pages").join("Canonical.md"),
            "# Canonical\n\nCanonical Brain page with enough body to index.",
        );
        write(
            &external.join("External.md"),
            "# External\n\nUnique obsidian enrichment term zettelkasten-signal.",
        );

        let report = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(report.pages, 2);
        let hits = engine
            .store
            .search("zettelkasten-signal", 10)
            .expect("search enrichment");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_name, "obsidian");
        assert_eq!(hits[0].source_type, "obsidian");
        assert_eq!(hits[0].source_role, "enrichment");
        assert_eq!(hits[0].relative_path.as_deref(), Some("External.md"));
    }

    #[test]
    fn sync_indexes_canvas_files_as_graph_hints() {
        let (dir, engine) = make_engine();
        let external = dir.path().join("External Obsidian");
        write_sources_config(dir.path(), &external);
        let canvas = serde_json::json!({
            "nodes": [
                {"id": "a", "type": "text", "text": "Project Alpha canvas-marker"},
                {"id": "b", "type": "file", "file": "Project Beta.md"}
            ],
            "edges": [
                {"id": "e", "fromNode": "a", "toNode": "b", "label": "depends_on"}
            ]
        })
        .to_string();
        write(&external.join("Roadmap.canvas"), &canvas);

        let report = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(report.pages, 1);
        let hits = engine.store.search("canvas-marker", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_name, "obsidian");
        assert_eq!(hits[0].relative_path.as_deref(), Some("Roadmap.canvas"));
        assert!(
            engine.store.search("makakoo_edge", 10).unwrap().is_empty(),
            "internal graph markers must not leak into FTS results"
        );

        let conn = engine.store.conn_arc();
        let conn = conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entity_graph
                 WHERE subject = 'Roadmap'
                   AND predicate = 'canvas_depends_on'
                   AND object = 'Project Beta'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn missing_enrichment_source_does_not_prune_existing_rows() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        let external = dir.path().join("External Obsidian");
        write_sources_config(dir.path(), &external);
        write(
            &brain.join("pages").join("Canonical.md"),
            "# Canonical\n\nCanonical Brain page with enough body to index.",
        );
        write(
            &external.join("External.md"),
            "# External\n\nPersistent external-only term chrysalis-marker.",
        );
        engine.sync(SyncOptions::default()).unwrap();
        fs::remove_dir_all(&external).unwrap();

        let report = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(report.removed, 0);
        let hits = engine.store.search("chrysalis-marker", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_role, "enrichment");
    }

    #[test]
    fn missing_canonical_subdirs_do_not_prune_existing_canonical_rows() {
        let (dir, engine) = make_engine();
        let brain = dir.path().join("data").join("Brain");
        write(
            &brain.join("pages").join("Canonical.md"),
            "# Canonical\n\nDurable canonical term azimuth-memory-marker.",
        );
        let first = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(first.pages, 1);

        fs::remove_dir_all(brain.join("pages")).unwrap();
        fs::create_dir_all(&brain).unwrap();
        let second = engine.sync(SyncOptions::default()).unwrap();
        assert_eq!(second.removed, 0);
        let hits = engine.store.search("azimuth-memory-marker", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_name, "default");
        assert_eq!(hits[0].source_role, "canonical");
    }

    #[test]
    fn enrichment_sources_overlapping_canonical_brain_are_skipped() {
        let (dir, engine) = make_engine();
        let cfg = dir.path().join("config");
        fs::create_dir_all(&cfg).unwrap();
        let body = serde_json::json!({
            "canonical": "default",
            "sources": [
                {"name": "default", "role": "canonical", "type": "logseq", "path": "$MAKAKOO_HOME/data/Brain", "writable": true},
                {"name": "obsidian", "role": "enrichment", "type": "obsidian", "path": "$MAKAKOO_HOME/data/Brain/pages", "writable": false}
            ]
        });
        fs::write(
            cfg.join("brain_sources.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();

        let sources = engine.load_sources().unwrap();
        assert!(sources.iter().any(|source| source.name == "default"));
        assert!(
            sources.iter().all(|source| source.name != "obsidian"),
            "enrichment source inside canonical Brain must not retag canonical files"
        );
    }
}
