//! Plugin registry walker.
//!
//! At daemon start, walks `$MAKAKOO_HOME/plugins/*/plugin.toml`, parses each
//! manifest, enforces cross-plugin uniqueness rules (PLUGIN_MANIFEST.md §17
//! rules 14-16), resolves load order via the resolver, and returns the
//! result. Subsystems (SANCHO, MCP gateway, infect renderer) consume the
//! returned `PluginRegistry` to discover the work they need to do.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, warn};

use super::lock::PluginsLock;
use super::manifest::{Manifest, ManifestError, ParseWarnings};
use super::resolver::{resolve_load_order, ResolverError};

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("plugins dir {path} is not a directory")]
    NotADir { path: PathBuf },
    #[error("failed to read plugins dir {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("manifest error: {0}")]
    Manifest(#[from] ManifestError),
    #[error("resolver error: {0}")]
    Resolver(#[from] ResolverError),
    #[error("duplicate plugin name {name:?} found in {a} and {b}")]
    DuplicateName {
        name: String,
        a: PathBuf,
        b: PathBuf,
    },
    #[error("duplicate sancho task name {name:?} across plugins {a:?} and {b:?}")]
    DuplicateSanchoTask { name: String, a: String, b: String },
    #[error("duplicate mcp tool name {name:?} across plugins {a:?} and {b:?}")]
    DuplicateMcpTool { name: String, a: String, b: String },
    #[error("duplicate infect fragment {name:?} across plugins {a:?} and {b:?}")]
    DuplicateInfectFragment { name: String, a: String, b: String },
}

/// One loaded plugin: manifest + the directory it lives in (so the kernel
/// can resolve relative paths like entrypoints and install scripts).
///
/// `enabled` mirrors the corresponding `plugins.lock` entry's flag —
/// plugins with `enabled = false` are still loaded (so `plugin list`
/// surfaces them) but subsystems like the SANCHO walker and MCP gateway
/// skip them. Plugins with no lock entry (fresh manifest, not yet
/// installed through `makakoo plugin install`) default to enabled.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: Manifest,
    pub root: PathBuf,
    pub warnings: Vec<String>,
    pub enabled: bool,
}

/// The result of walking a plugins directory. Plugins are in load order
/// (deps before dependents), indexed by name for quick lookup.
#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: Vec<LoadedPlugin>,
}

impl PluginRegistry {
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins
            .iter()
            .find(|p| p.manifest.plugin.name == name)
    }

    /// Return filesystem paths for all installed `kind = "library"` plugins.
    /// These paths should be prepended to PYTHONPATH (or equivalent) so that
    /// skill and agent plugins can import library code.
    pub fn get_library_paths(&self) -> Vec<PathBuf> {
        use super::manifest::PluginKind;
        self.plugins
            .iter()
            .filter(|p| p.manifest.plugin.kind == PluginKind::Library)
            .map(|p| p.root.clone())
            .collect()
    }

    /// Discover every plugin under `$MAKAKOO_HOME/plugins/`. Returns an
    /// empty registry if the directory doesn't exist (fresh install).
    ///
    /// Overlays the `plugins.lock` enabled flags so subsystems that walk
    /// the registry can skip plugins that `makakoo plugin disable <name>`
    /// turned off. Plugins with no lock entry stay enabled (the registry
    /// is the authoritative "installed" set; the lock is a metadata
    /// overlay that may lag by one install cycle).
    pub fn load_default(makakoo_home: &Path) -> Result<Self, RegistryError> {
        let dir = makakoo_home.join("plugins");
        if !dir.exists() {
            return Ok(Self::default());
        }
        let mut reg = Self::load_from(&dir)?;
        // Lock-file overlay is best-effort — a missing or malformed lock
        // mustn't block boot. Plugins default to enabled if we can't
        // resolve their lock entry.
        if let Ok(lock) = PluginsLock::load(makakoo_home) {
            for plugin in reg.plugins.iter_mut() {
                if let Some(entry) = lock.get(&plugin.manifest.plugin.name) {
                    plugin.enabled = entry.enabled;
                }
            }
        }
        Ok(reg)
    }

    /// Discover every plugin under `dir`. Subdirectories named `.stage/`
    /// (staging area, PLUGIN_MANIFEST.md §6) are ignored.
    pub fn load_from(dir: &Path) -> Result<Self, RegistryError> {
        if !dir.is_dir() {
            return Err(RegistryError::NotADir {
                path: dir.to_path_buf(),
            });
        }
        let mut manifests: Vec<(Manifest, PathBuf, ParseWarnings)> = Vec::new();
        let mut seen_names: HashMap<String, PathBuf> = HashMap::new();

        let entries = std::fs::read_dir(dir).map_err(|source| RegistryError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| RegistryError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if file_name.starts_with('.') {
                // Hidden dirs — includes `.stage/` (in-progress installs)
                // and any dotfile directories users create. Skip silently.
                continue;
            }
            let manifest_path = path.join("plugin.toml");
            if !manifest_path.exists() {
                warn!(
                    "skipping {} — no plugin.toml",
                    path.display()
                );
                continue;
            }
            let (manifest, warnings) = Manifest::load(&manifest_path)?;

            // Rule 14/15/16 rely on plugin name uniqueness too — check
            // before we add to the pile.
            if let Some(prev) = seen_names.get(&manifest.plugin.name) {
                return Err(RegistryError::DuplicateName {
                    name: manifest.plugin.name.clone(),
                    a: prev.clone(),
                    b: path.clone(),
                });
            }
            seen_names.insert(manifest.plugin.name.clone(), path.clone());

            debug!(
                "loaded plugin {} v{} from {}",
                manifest.plugin.name,
                manifest.plugin.version,
                path.display()
            );
            manifests.push((manifest, path, warnings));
        }

        // Rules 14-16: cross-plugin uniqueness of SANCHO tasks, MCP tools,
        // infect fragments. Checked once across the full set.
        check_uniqueness(&manifests)?;

        // Resolver: ABI + deps + topological sort.
        let just_manifests: Vec<Manifest> =
            manifests.iter().map(|(m, _, _)| m.clone()).collect();
        let ordered = resolve_load_order(&just_manifests)?;

        // Zip the sorted manifests back with their roots + warnings.
        let by_name: HashMap<String, (PathBuf, ParseWarnings)> = manifests
            .into_iter()
            .map(|(m, p, w)| (m.plugin.name.clone(), (p, w)))
            .collect();
        let plugins: Vec<LoadedPlugin> = ordered
            .into_iter()
            .map(|m| {
                let (root, warnings) = by_name
                    .get(&m.plugin.name)
                    .cloned()
                    .expect("resolver returned unknown name");
                LoadedPlugin {
                    manifest: m,
                    root,
                    warnings: warnings.0,
                    enabled: true,
                }
            })
            .collect();

        Ok(Self { plugins })
    }
}

fn check_uniqueness(
    manifests: &[(Manifest, PathBuf, ParseWarnings)],
) -> Result<(), RegistryError> {
    let mut sancho_owner: HashMap<String, String> = HashMap::new();
    let mut mcp_owner: HashMap<String, String> = HashMap::new();
    let mut fragment_owner: HashMap<String, String> = HashMap::new();
    let _: BTreeSet<&str> = BTreeSet::new(); // reserved for future hooks

    for (m, _, _) in manifests {
        let plugin_name = m.plugin.name.clone();
        for task in &m.sancho.tasks {
            if let Some(prev) = sancho_owner.get(&task.name) {
                return Err(RegistryError::DuplicateSanchoTask {
                    name: task.name.clone(),
                    a: prev.clone(),
                    b: plugin_name.clone(),
                });
            }
            sancho_owner.insert(task.name.clone(), plugin_name.clone());
        }
        for tool in &m.mcp.tools {
            if let Some(prev) = mcp_owner.get(&tool.name) {
                return Err(RegistryError::DuplicateMcpTool {
                    name: tool.name.clone(),
                    a: prev.clone(),
                    b: plugin_name.clone(),
                });
            }
            mcp_owner.insert(tool.name.clone(), plugin_name.clone());
        }
        for frag in m.infect.fragments.keys() {
            if let Some(prev) = fragment_owner.get(frag) {
                return Err(RegistryError::DuplicateInfectFragment {
                    name: frag.clone(),
                    a: prev.clone(),
                    b: plugin_name.clone(),
                });
            }
            fragment_owner.insert(frag.clone(), plugin_name.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed(dir: &Path, name: &str, body: &str) {
        let p = dir.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("plugin.toml"), body).unwrap();
    }

    fn skill(name: &str, version: &str, deps: &[(&str, &str)]) -> String {
        let dep_line = if deps.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = deps
                .iter()
                .map(|(n, c)| format!("\"{n} {c}\""))
                .collect();
            format!("\n[depends]\nplugins = [{}]", items.join(", "))
        };
        format!(
            r#"
[plugin]
name = "{name}"
version = "{version}"
kind = "skill"
language = "python"

[source]
path = "."

[abi]
skill = "^1.0"

[entrypoint]
run = "true"
{dep_line}
"#
        )
    }

    #[test]
    fn empty_plugins_dir_returns_empty_registry() {
        let tmp = TempDir::new().unwrap();
        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        assert!(reg.is_empty());
    }

    #[test]
    fn walks_and_loads_two_plugins() {
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        seed(&plugins, "alpha", &skill("alpha", "1.0.0", &[]));
        seed(&plugins, "beta", &skill("beta", "1.0.0", &[("alpha", "^1")]));

        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        assert_eq!(reg.len(), 2);
        let names: Vec<&str> = reg
            .plugins()
            .iter()
            .map(|p| p.manifest.plugin.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert!(reg.get("alpha").is_some());
    }

    #[test]
    fn stage_dir_is_ignored() {
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        seed(&plugins, "alpha", &skill("alpha", "1.0.0", &[]));
        // A "stale" staging dir from a crashed install — must be skipped.
        seed(&plugins, ".stage", &skill("broken", "1.0.0", &[]));

        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.plugins()[0].manifest.plugin.name, "alpha");
    }

    #[test]
    fn duplicate_sancho_task_across_plugins_rejected() {
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let body = |name: &str| {
            format!(
                r#"
[plugin]
name = "{name}"
version = "1.0.0"
kind = "sancho-task"
language = "python"

[source]
path = "."

[abi]
sancho-task = "^0.1"

[entrypoint]
run = "true"

[sancho]
tasks = [{{ name = "shared_task", interval = "5m" }}]
"#
            )
        };
        seed(&plugins, "aa", &body("aa"));
        seed(&plugins, "bb", &body("bb"));
        let err = PluginRegistry::load_default(tmp.path()).unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateSanchoTask { .. }));
    }

    /// Walk the repo-shipped `plugins-core/` tree and assert every
    /// `plugin.toml` parses cleanly. This catches schema drift the moment
    /// anyone edits a core manifest.
    #[test]
    fn shipped_core_plugins_all_parse() {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let repo_root = std::path::Path::new(&manifest_dir)
            .parent()
            .expect("manifest dir has a parent");
        let core_dir = repo_root.join("plugins-core");
        if !core_dir.exists() {
            // Running from an unexpected layout — skip rather than fail.
            return;
        }
        let entries = std::fs::read_dir(&core_dir).unwrap();
        let mut count = 0usize;
        for entry in entries {
            let entry = entry.unwrap();
            let plugin_toml = entry.path().join("plugin.toml");
            if !plugin_toml.exists() {
                continue;
            }
            let (m, warnings) = Manifest::load(&plugin_toml)
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", plugin_toml.display()));
            assert_eq!(
                m.plugin.name,
                entry.file_name().to_string_lossy(),
                "plugin.name must match directory name for {}",
                plugin_toml.display()
            );
            // Core plugins use reserved prefixes freely; swallow that warning.
            for w in &warnings.0 {
                if !w.contains("reserved prefix") {
                    eprintln!("warn: {w}");
                }
            }
            count += 1;
        }
        assert!(
            count >= 4,
            "expected at least 4 plugins-core manifests, found {count}"
        );
    }

    #[test]
    fn duplicate_plugin_name_rejected() {
        // Two plugin dirs where the inner plugin.toml declares the same name.
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        seed(&plugins, "first", &skill("dup", "1.0.0", &[]));
        seed(&plugins, "second", &skill("dup", "1.0.0", &[]));
        let err = PluginRegistry::load_default(tmp.path()).unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateName { .. }));
    }
}
