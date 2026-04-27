//! Include-chain resolver for distro files.
//!
//! Spec: `spec/DISTRO.md §2` — chains are resolved recursively before
//! applying the current distro, last-write-wins on plugin overrides,
//! `[excludes]` in the current distro strips entries from the merged set,
//! cycles refuse with a clear error.
//!
//! The resolver returns an `EffectivePlugin` list — the flat, deduplicated,
//! exclude-aware plugin set that the installer can iterate over.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::debug;

use super::file::{DistroError, DistroFile, PluginPin};

#[derive(Debug, Error)]
pub enum DistroResolverError {
    #[error("distro file error: {0}")]
    File(#[from] DistroError),
    #[error("include cycle detected: {chain}")]
    IncludeCycle { chain: String },
    #[error("included distro not found: {path}")]
    IncludeNotFound { path: PathBuf },
}

/// One entry in the resolved effective plugin list.
#[derive(Debug, Clone)]
pub struct EffectivePlugin {
    pub name: String,
    pub pin: PluginPin,
    /// Which distro contributed this entry (innermost winner).
    pub source_distro: String,
}

/// Output of `resolve_distro` — the walked distro chain, ready for install.
#[derive(Debug, Clone)]
pub struct ResolvedDistro {
    /// The top-level distro (the one the user asked to install).
    pub root: DistroFile,
    /// Effective plugin list after include-merge and excludes. Ordered by
    /// plugin name (alphabetical) for deterministic install output.
    pub plugins: Vec<EffectivePlugin>,
    /// Absolute paths of every distro file visited, in include order.
    /// Useful for debugging + `distro install --dry-run` output.
    pub chain: Vec<PathBuf>,
}

/// Resolve a distro file + all of its includes.
///
/// * `root` — the distro file the user named. Its `[distro].name` is the
///   install target.
/// * `root_path` — absolute path where `root` was loaded from. Used to
///   resolve relative include paths ("minimal.toml" resolves relative to
///   the directory containing the root file).
pub fn resolve_distro(
    root: &DistroFile,
    root_path: &Path,
) -> Result<ResolvedDistro, DistroResolverError> {
    let base_dir = root_path.parent().unwrap_or_else(|| Path::new("."));

    // Walk includes depth-first. Track visited absolute paths to catch
    // cycles. Each visited file contributes its plugins before the next.
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut merged: BTreeMap<String, EffectivePlugin> = BTreeMap::new();
    let mut visiting: Vec<PathBuf> = Vec::new();

    walk(
        root,
        root_path,
        base_dir,
        &mut merged,
        &mut chain,
        &mut visiting,
    )?;

    // Apply the root's own excludes last — they strip from the merged set
    // per spec §2 "the current distro's entries override included entries
    // and excludes remove from the merged list."
    for ex in &root.excludes.plugins {
        if merged.remove(ex).is_some() {
            debug!("distro {} excludes plugin {}", root.distro.name, ex);
        }
    }

    let plugins: Vec<EffectivePlugin> = merged.into_values().collect();
    Ok(ResolvedDistro {
        root: root.clone(),
        plugins,
        chain,
    })
}

fn walk(
    current: &DistroFile,
    current_path: &Path,
    base_dir: &Path,
    merged: &mut BTreeMap<String, EffectivePlugin>,
    chain: &mut Vec<PathBuf>,
    visiting: &mut Vec<PathBuf>,
) -> Result<(), DistroResolverError> {
    let canon = canonicalize_hint(current_path);

    if visiting.contains(&canon) {
        let mut displayed: Vec<String> = visiting
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        displayed.push(canon.to_string_lossy().to_string());
        return Err(DistroResolverError::IncludeCycle {
            chain: displayed.join(" → "),
        });
    }

    visiting.push(canon.clone());

    // Recurse into every [include] entry first so the current distro's
    // plugins can override included ones (last-write-wins).
    for inc in current.include() {
        let inc_path = base_dir.join(inc);
        if !inc_path.exists() {
            return Err(DistroResolverError::IncludeNotFound {
                path: inc_path.clone(),
            });
        }
        let inc_file = DistroFile::load(&inc_path)?;
        let inc_base = inc_path.parent().unwrap_or(base_dir);
        walk(&inc_file, &inc_path, inc_base, merged, chain, visiting)?;
    }

    // Contribute this distro's plugins (overrides included).
    for (name, pin) in &current.plugins {
        merged.insert(
            name.clone(),
            EffectivePlugin {
                name: name.clone(),
                pin: pin.clone(),
                source_distro: current.distro.name.clone(),
            },
        );
    }

    // Apply this distro's excludes (strip from merged set).
    for ex in &current.excludes.plugins {
        merged.remove(ex);
    }

    chain.push(canon.clone());
    visiting.pop();
    Ok(())
}

/// Best-effort canonicalization. If the path doesn't exist on disk (eg.
/// test with absolute virtual path), fall back to the path as given so the
/// cycle-detection key is still stable.
fn canonicalize_hint(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn resolves_single_file_no_includes() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            tmp.path(),
            "core.toml",
            r#"
[distro]
name = "core"
[plugins]
"plugin-a" = "*"
"plugin-b" = "^1"
"#,
        );
        let root = DistroFile::load(&path).unwrap();
        let r = resolve_distro(&root, &path).unwrap();
        assert_eq!(r.plugins.len(), 2);
        let names: Vec<&str> = r.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["plugin-a", "plugin-b"]);
        assert!(r
            .plugins
            .iter()
            .all(|p| p.source_distro == "core"));
    }

    #[test]
    fn include_merges_and_last_wins() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[distro]
name = "base"
[plugins]
"p-a" = "^0.1"
"p-b" = "^0.1"
"#,
        );
        let top = write(
            tmp.path(),
            "top.toml",
            r#"
[distro]
name = "top"
include = ["base.toml"]
# note: include is a field of [distro]; any [table] headers must come after
[plugins]
"p-b" = "^0.2"
"p-c" = "*"
"#,
        );
        let root = DistroFile::load(&top).unwrap();
        let r = resolve_distro(&root, &top).unwrap();
        assert_eq!(r.plugins.len(), 3);
        let pb = r.plugins.iter().find(|p| p.name == "p-b").unwrap();
        assert_eq!(pb.pin.version(), "^0.2");
        assert_eq!(pb.source_distro, "top");
        let pa = r.plugins.iter().find(|p| p.name == "p-a").unwrap();
        assert_eq!(pa.source_distro, "base");
    }

    #[test]
    fn excludes_from_root_strip_included_entries() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[distro]
name = "base"
[plugins]
"p-a" = "^0.1"
"p-b" = "^0.1"
"#,
        );
        let top = write(
            tmp.path(),
            "top.toml",
            r#"
[distro]
name = "top"
include = ["base.toml"]
# note: include is a field of [distro]; any [table] headers must come after
[excludes]
plugins = ["p-b"]
"#,
        );
        let root = DistroFile::load(&top).unwrap();
        let r = resolve_distro(&root, &top).unwrap();
        let names: Vec<&str> = r.plugins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["p-a"]);
    }

    #[test]
    fn include_cycle_is_detected() {
        let tmp = TempDir::new().unwrap();
        let a = write(
            tmp.path(),
            "a.toml",
            r#"
[distro]
name = "aa"
include = ["b.toml"]
# include field of [distro]
"#,
        );
        write(
            tmp.path(),
            "b.toml",
            r#"
[distro]
name = "bb"
include = ["a.toml"]
# include field of [distro]
"#,
        );
        let root = DistroFile::load(&a).unwrap();
        let err = resolve_distro(&root, &a).unwrap_err();
        assert!(matches!(err, DistroResolverError::IncludeCycle { .. }));
    }

    #[test]
    fn missing_include_errors() {
        let tmp = TempDir::new().unwrap();
        let top = write(
            tmp.path(),
            "top.toml",
            r#"
[distro]
name = "top"
include = ["does-not-exist.toml"]
# note: include is a field of [distro]
"#,
        );
        let root = DistroFile::load(&top).unwrap();
        let err = resolve_distro(&root, &top).unwrap_err();
        assert!(matches!(err, DistroResolverError::IncludeNotFound { .. }));
    }

    #[test]
    fn chain_lists_every_file_visited() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "base.toml",
            r#"
[distro]
name = "base"
"#,
        );
        let top = write(
            tmp.path(),
            "top.toml",
            r#"
[distro]
name = "top"
include = ["base.toml"]
# note: include is a field of [distro]; any [table] headers must come after
"#,
        );
        let root = DistroFile::load(&top).unwrap();
        let r = resolve_distro(&root, &top).unwrap();
        assert_eq!(r.chain.len(), 2);
        // base is visited first (depth-first), then top.
        assert!(r.chain[0].ends_with("base.toml"));
        assert!(r.chain[1].ends_with("top.toml"));
    }
}
