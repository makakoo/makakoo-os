// Build-time: walk ../docs/ and ../spec/ and bake them into a SQLite
// FTS5 database that the binary embeds via `include_bytes!` at runtime.
//
// Tytus integration (clone github.com/traylinx/tytus-cli at build time
// and vendor README.md + CHANGELOG.md + pkg/SIGNING.md) is wired in
// Phase C — see SPRINT.md and verdicts/Q2-TYTUS-DOCS.md.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;
use walkdir::WalkDir;

fn main() -> Result<()> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest
        .parent()
        .context("makakoo-docs-mcp must live inside the makakoo-os workspace")?
        .to_path_buf();

    let docs_root = workspace_root.join("docs");
    let spec_root = workspace_root.join("spec");

    println!("cargo:rerun-if-changed={}", docs_root.display());
    println!("cargo:rerun-if-changed={}", spec_root.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let db_path = out_dir.join("docs-corpus.db");

    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
    }

    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE docs USING fts5(
            path UNINDEXED,
            title,
            body,
            tokenize = 'porter unicode61'
        );
        CREATE TABLE meta (
            key TEXT PRIMARY KEY,
            value TEXT
        );
        "#,
    )?;

    let mut count = 0_usize;
    let mut bytes = 0_usize;
    for root in [&docs_root, &spec_root] {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let body = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let title = extract_title(&body, path);
            let rel = path
                .strip_prefix(&workspace_root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            conn.execute(
                "INSERT INTO docs(path, title, body) VALUES (?1, ?2, ?3)",
                rusqlite::params![rel, title, body],
            )?;
            count += 1;
            bytes += body.len();
        }
    }

    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('doc_count', ?1)",
        rusqlite::params![count.to_string()],
    )?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('byte_count', ?1)",
        rusqlite::params![bytes.to_string()],
    )?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('built_for_version', ?1)",
        rusqlite::params![env!("CARGO_PKG_VERSION")],
    )?;

    drop(conn);

    println!(
        "cargo:warning=makakoo-docs-mcp: indexed {count} markdown files ({bytes} bytes) into {}",
        db_path.display()
    );

    Ok(())
}

fn extract_title(body: &str, path: &Path) -> String {
    for line in body.lines().take(50) {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("# ") {
            return rest.trim().to_string();
        }
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}
