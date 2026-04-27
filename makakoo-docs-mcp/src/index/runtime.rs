// Runtime index — opens the baked SQLite FTS5 DB that build.rs wrote
// into OUT_DIR, materializes it to a temp file, and exposes a query API.
//
// Phase C fills in search()/read()/list()/topic() against this Index.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::Connection;

const BAKED_DB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/docs-corpus.db"));

#[derive(Clone)]
pub struct Index {
    conn: Arc<Mutex<Connection>>,
    pub doc_count: usize,
}

impl Index {
    pub fn open() -> Result<Self> {
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

    #[allow(dead_code)] // Phase C wires this up
    pub(crate) fn with_conn<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&Connection) -> T,
    {
        let g = self.conn.lock().expect("docs index mutex poisoned");
        f(&g)
    }
}

fn tempfile_path(name: &str) -> Result<PathBuf> {
    let mut p = std::env::temp_dir();
    p.push(format!("{}-{}", std::process::id(), name));
    Ok(p)
}
