//! `makakoo docs --update` — fetch latest docs from GitHub and rebuild the
//! FTS5 cache at `~/.makakoo/docs-cache/index.db`.
//!
//! The MCP server (`makakoo-docs-mcp`) prefers this cache when present;
//! it falls back to the baked-in corpus otherwise.
//!
//! Phase E of the MAKAKOO-DOCS-MCP sprint.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::cli::DocsCmd;

/// Default tarball URL for the makakoo-os repo at `main`.
const GITHUB_TARBALL_URL: &str =
    "https://api.github.com/repos/makakoo/makakoo-os/tarball/main";

/// Where the refreshed index is stored on the user's machine.
pub fn cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".makakoo").join("docs-cache"))
}

pub fn cache_index_path() -> Result<PathBuf> {
    Ok(cache_dir()?.join("index.db"))
}

// ─── FTS5 schema — must match build.rs exactly ────────────────────────────

const CREATE_SCHEMA: &str = r#"
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
"#;

// ─── Entry point ──────────────────────────────────────────────────────────

pub async fn run(cmd: DocsCmd) -> anyhow::Result<i32> {
    match cmd {
        DocsCmd::Update {
            from_github,
            from_branch,
        } => update(from_github, from_branch).await,
    }
}

async fn update(from_github: bool, from_branch: Option<String>) -> anyhow::Result<i32> {
    // `--update` always implies `--from-github` if no local alternative
    // is present.  We only have one fetch source right now so we always go
    // to GitHub.
    let _ = from_github; // flag accepted, always true in Phase E

    let branch = from_branch.as_deref().unwrap_or("main");
    let url = if from_branch.is_some() {
        format!(
            "https://api.github.com/repos/makakoo/makakoo-os/tarball/{}",
            branch
        )
    } else {
        GITHUB_TARBALL_URL.to_string()
    };

    println!("Fetching docs tarball from GitHub ({branch})…");

    // 1. Fetch tarball ---------------------------------------------------
    let bytes = fetch_tarball(&url).await?;
    println!("  downloaded {} KB", bytes.len() / 1024);

    // 2. Extract to a staging dir ----------------------------------------
    let cache = cache_dir()?;
    let staging = cache.join("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clearing staging dir {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;

    let extracted_root = extract_tarball(&bytes, &staging)?;
    println!("  extracted to {}", extracted_root.display());

    // 3. Build FTS5 index into index.db.tmp then rename atomically --------
    let tmp_db = cache.join("index.db.tmp");
    if tmp_db.exists() {
        std::fs::remove_file(&tmp_db)?;
    }
    std::fs::create_dir_all(&cache)?;

    let (doc_count, byte_count) = build_index(&extracted_root, &tmp_db)?;

    let final_db = cache.join("index.db");
    std::fs::rename(&tmp_db, &final_db).with_context(|| {
        format!(
            "renaming {} → {}",
            tmp_db.display(),
            final_db.display()
        )
    })?;

    // 4. Clean up staging ------------------------------------------------
    let _ = std::fs::remove_dir_all(&staging);

    println!(
        "  indexed {} docs ({} KB) → {}",
        doc_count,
        byte_count / 1024,
        final_db.display()
    );
    println!("Done. makakoo-docs-mcp will use the cache on next start.");
    Ok(0)
}

// ─── HTTP fetch ───────────────────────────────────────────────────────────

async fn fetch_tarball(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent("makakoo-docs-update/1.0")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let resp = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned {}: {url}", resp.status());
    }

    let bytes = resp
        .bytes()
        .await
        .context("reading tarball response body")?;
    Ok(bytes.to_vec())
}

// ─── Tarball extraction ───────────────────────────────────────────────────

/// Extract the `.tar.gz` bytes into `dest`, stripping the first path
/// component (the `makakoo-makakoo-os-<sha>/` prefix GitHub adds).
/// Returns the effective root inside `dest` (i.e. `dest` itself, since we
/// strip the top component).
fn extract_tarball(bytes: &[u8], dest: &Path) -> Result<PathBuf> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries().context("reading tar entries")? {
        let mut entry = entry.context("bad tar entry")?;
        let entry_path = entry.path().context("bad entry path")?;

        // Strip first component (e.g. `makakoo-makakoo-os-abc1234/`)
        let mut components = entry_path.components();
        components.next(); // drop first component
        let relative: PathBuf = components.collect();

        if relative.as_os_str().is_empty() {
            // This is the top-level dir entry itself — skip.
            continue;
        }

        // Only keep docs/ and spec/ subtrees
        let first = relative
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .unwrap_or("");
        if first != "docs" && first != "spec" {
            continue;
        }

        let out_path = dest.join(&relative);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if entry.header().entry_type().is_file() {
            let mut f = std::fs::File::create(&out_path)
                .with_context(|| format!("creating {}", out_path.display()))?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            f.write_all(&buf)?;
        }
    }

    Ok(dest.to_path_buf())
}

// ─── FTS5 index builder ───────────────────────────────────────────────────

/// Walk `docs/` and `spec/` inside `root`, insert every `.md` file into a
/// fresh FTS5 database at `db_path`.  Returns `(doc_count, byte_count)`.
fn build_index(root: &Path, db_path: &Path) -> Result<(usize, usize)> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening {}", db_path.display()))?;

    conn.execute_batch(CREATE_SCHEMA)
        .context("creating FTS5 schema")?;

    let mut count = 0_usize;
    let mut bytes = 0_usize;

    for subdir in ["docs", "spec"] {
        let dir = root.join(subdir);
        if !dir.exists() {
            continue;
        }
        walk_and_insert(&conn, &dir, root, &mut count, &mut bytes)?;
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

    Ok((count, bytes))
}

fn walk_and_insert(
    conn: &Connection,
    dir: &Path,
    workspace_root: &Path,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<()> {
    let entries = {
        let mut v = Vec::new();
        collect_md_files(dir, &mut v)?;
        v
    };

    for path in entries {
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let title = extract_title(&body, &path);
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        conn.execute(
            "INSERT INTO docs(path, title, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![rel, title, body],
        )?;
        *count += 1;
        *bytes += body.len();
    }
    Ok(())
}

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_md_files(&path, out)?;
        } else if ft.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }
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
