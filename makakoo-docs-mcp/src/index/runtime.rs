// Runtime index — opens the baked SQLite FTS5 DB that build.rs wrote
// into OUT_DIR, materializes it to a temp file, and exposes a query API.
//
// Phase C: search()/read()/list()/topic() implemented.
// Phase E: prefers `~/.makakoo/docs-cache/index.db` when present and
//          built for the same crate version; falls back to baked corpus.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::tools::{
    list::ListEntry,
    search::SearchHit,
    topic::TopicResult,
};

const BAKED_DB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/docs-corpus.db"));

/// Path to the user-refreshed docs cache (`~/.makakoo/docs-cache/index.db`).
fn cache_index_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".makakoo").join("docs-cache").join("index.db"))
}

#[derive(Clone)]
pub struct Index {
    conn: Arc<Mutex<Connection>>,
    pub doc_count: usize,
}

impl Index {
    /// Open the docs index.
    ///
    /// Preference order:
    /// 1. `~/.makakoo/docs-cache/index.db` — if present **and** built for
    ///    the same crate version (meta.built_for_version == CARGO_PKG_VERSION).
    /// 2. Baked-in corpus (compile-time, always available).
    pub fn open() -> Result<Self> {
        // Phase E: try on-disk cache first.
        if let Some(cache_path) = cache_index_path() {
            if let Some(idx) = try_open_cache(&cache_path) {
                return Ok(idx);
            }
        }
        // Fallback: materialize baked corpus to a temp file.
        Self::open_baked()
    }

    fn open_baked() -> Result<Self> {
        let mut tmp = tempfile_path("makakoo-docs-corpus.db")?;
        // If a previous run left it around, overwrite — safe because the
        // path is per-process (PID-suffixed) and we own it.
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating temp db at {}", tmp.display()))?;
        f.write_all(BAKED_DB)?;
        f.sync_all()?;
        drop(f);

        let conn = Connection::open(&tmp)?;
        let doc_count: i64 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'doc_count'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        // best-effort cleanup hint; the file is opened so unlink-on-close
        // semantics on POSIX keep it readable until conn drops.
        let _ = std::fs::remove_file(&tmp);
        tmp.pop();

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            doc_count: doc_count as usize,
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let g = self.conn.lock().expect("docs index mutex poisoned");
        let mut stmt = g.prepare(
            "SELECT path, title, snippet(docs, 2, '<<', '>>', '...', 24), bm25(docs) AS score \
             FROM docs WHERE docs MATCH ?1 ORDER BY score LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SearchHit {
                path: row.get(0)?,
                title: row.get(1)?,
                snippet: row.get(2)?,
                score: row.get::<_, f64>(3)?,
            })
        })?;
        let mut hits = Vec::with_capacity(limit);
        for hit in rows {
            hits.push(hit?);
        }
        Ok(hits)
    }

    pub fn read(&self, path: &str) -> Result<Option<String>> {
        let g = self.conn.lock().expect("docs index mutex poisoned");
        let body: Option<String> = g
            .query_row(
                "SELECT body FROM docs WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .ok();
        Ok(body)
    }

    pub fn list(&self, prefix: Option<&str>) -> Result<Vec<ListEntry>> {
        let g = self.conn.lock().expect("docs index mutex poisoned");
        let pattern = match prefix {
            Some(p) => format!("{p}%"),
            None => "%".to_string(),
        };
        let mut stmt = g.prepare(
            "SELECT path, length(body), title FROM docs WHERE path LIKE ?1 ORDER BY path",
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok(ListEntry {
                path: row.get(0)?,
                size_bytes: row.get(1)?,
                title: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn topic(&self, name: &str) -> Result<TopicResult> {
        let g = self.conn.lock().expect("docs index mutex poisoned");
        // Title match: BM25 on the title field. FTS5 column-restricted query.
        let q = format!("title:{}", escape_fts(name));
        let canonical: Option<String> = g
            .query_row(
                "SELECT path FROM docs WHERE docs MATCH ?1 ORDER BY bm25(docs) LIMIT 1",
                params![q],
                |row| row.get(0),
            )
            .ok();

        let breadcrumb = canonical
            .as_deref()
            .map(|p| {
                p.split('/')
                    .scan(String::new(), |acc, part| {
                        if !acc.is_empty() {
                            acc.push('/');
                        }
                        acc.push_str(part);
                        Some(acc.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let related = match canonical.as_deref() {
            Some(p) => {
                let dir = p.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let pattern = format!("{dir}/%");
                let mut stmt = g.prepare(
                    "SELECT path FROM docs WHERE path LIKE ?1 AND path != ?2 ORDER BY path LIMIT 8",
                )?;
                let rows = stmt.query_map(params![pattern, p], |row| row.get::<_, String>(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                out
            }
            None => Vec::new(),
        };

        Ok(TopicResult {
            breadcrumb,
            related,
            canonical,
        })
    }
}

/// Try to open the on-disk docs cache.
///
/// Returns `None` (quietly) when the cache is absent, unreadable, or was
/// built by a different binary version — in all those cases the caller
/// falls back to the baked corpus.
fn try_open_cache(path: &Path) -> Option<Index> {
    if !path.exists() {
        return None;
    }

    let conn = Connection::open(path)
        .map_err(|e| tracing::warn!("docs cache open failed ({}): {e}", path.display()))
        .ok()?;

    // Version gate: only use the cache when it was produced by exactly
    // this binary version.  That guarantees schema + tokenizer stability.
    let cache_version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'built_for_version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();

    if cache_version != env!("CARGO_PKG_VERSION") {
        tracing::info!(
            "docs cache version mismatch (cache={cache_version}, binary={}) — using baked corpus",
            env!("CARGO_PKG_VERSION")
        );
        return None;
    }

    let doc_count: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'doc_count'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    tracing::info!(
        "docs-mcp: using cache ({} docs) from {}",
        doc_count,
        path.display()
    );

    Some(Index {
        conn: Arc::new(Mutex::new(conn)),
        doc_count: doc_count as usize,
    })
}

fn escape_fts(s: &str) -> String {
    // FTS5 wants double quotes around terms with non-alnum chars.
    if s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

fn tempfile_path(name: &str) -> Result<PathBuf> {
    let mut p = std::env::temp_dir();
    p.push(format!("{}-{}", std::process::id(), name));
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_counts_docs() {
        let idx = Index::open().expect("open");
        assert!(idx.doc_count > 50, "expected >50 docs, got {}", idx.doc_count);
    }

    #[test]
    fn search_returns_relevant_hits() {
        let idx = Index::open().expect("open");
        let hits = idx.search("plugin install", 5).expect("search");
        assert!(!hits.is_empty());
        // Top hit's title or path should mention plugin.
        let top = &hits[0];
        assert!(
            top.title.to_lowercase().contains("plugin")
                || top.path.contains("plugin"),
            "top hit unrelated: {top:?}"
        );
    }

    #[test]
    fn read_round_trips() {
        let idx = Index::open().expect("open");
        let any = idx
            .list(Some("docs/"))
            .expect("list")
            .into_iter()
            .next()
            .expect("at least one doc under docs/");
        let body = idx.read(&any.path).expect("read").expect("present");
        assert!(body.len() > 50, "tiny body for {}", any.path);
    }

    #[test]
    fn read_unknown_path_returns_none() {
        let idx = Index::open().expect("open");
        assert!(idx.read("docs/does-not-exist.md").unwrap().is_none());
    }

    #[test]
    fn list_prefix_filters() {
        let idx = Index::open().expect("open");
        let all = idx.list(None).expect("list");
        let docs_only = idx.list(Some("docs/")).expect("list docs/");
        assert!(docs_only.len() <= all.len());
        assert!(docs_only.iter().all(|e| e.path.starts_with("docs/")));
    }

    #[test]
    fn topic_resolves_canonical() {
        let idx = Index::open().expect("open");
        let t = idx.topic("agent").expect("topic");
        assert!(t.canonical.is_some(), "no canonical doc for 'agent'");
        let canonical = t.canonical.as_deref().unwrap();
        // Breadcrumb starts at the first path segment and ends at the canonical doc.
        assert_eq!(t.breadcrumb.last().map(String::as_str), Some(canonical));
    }
}
