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
use super::manifest::{Manifest, ManifestError, ParseWarnings, PluginKind};
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
            // Graceful degrade (v0.2 A.1): one malformed plugin.toml used to
            // kill kernel boot via `?` propagation. Now we warn and skip —
            // a broken plugin is far better than a dead daemon.
            let (manifest, warnings) = match Manifest::load(&manifest_path) {
                Ok(pair) => pair,
                Err(err) => {
                    warn!(
                        plugin_path = %path.display(),
                        manifest = %manifest_path.display(),
                        error = %err,
                        "skipping plugin — manifest failed to parse",
                    );
                    continue;
                }
            };

            // SPRINT-PATTERN-SUBSTRATE-V1 Phase 1.4: pattern plugins
            // require a sibling system.md (the prompt body). Reject the
            // plugin (graceful skip + warn) if missing — better than
            // failing at first dispatch.
            if manifest.plugin.kind == PluginKind::Pattern
                && !path.join("system.md").exists()
            {
                warn!(
                    plugin = %manifest.plugin.name,
                    plugin_path = %path.display(),
                    "skipping pattern plugin — sibling system.md is missing",
                );
                continue;
            }

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
        // Infect-fragment keys (e.g. `default`, `claude`, `gemini`) are
        // HOST identifiers, not globally-unique fragment IDs. Multiple
        // plugins legitimately declare `default = "fragments/default.md"`
        // — the renderer (makakoo/src/infect/renderer.rs) walks every
        // plugin in load order and concatenates their fragments. The only
        // thing that must stay unique is the map key WITHIN a single
        // manifest, which the TOML parser enforces for free.
        let _ = &fragment_owner; // silence unused while we keep the map
                                 // around for future per-host conflict
                                 // detection (e.g. "two plugins tried to
                                 // own the same `claude`-host slot and
                                 // the host can only take one").
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

    fn pattern_toml(name: &str) -> String {
        format!(
            r#"
[plugin]
name = "{name}"
version = "0.1.0"
kind = "pattern"
language = "shell"

[source]
path = "."

[pattern]
description = "test pattern"
model = "ail-compound"

[[pattern.variables]]
name = "input"
kind = "string"
required = true
"#
        )
    }

    fn seed_pattern(dir: &Path, name: &str, with_system_md: bool) {
        let p = dir.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("plugin.toml"), pattern_toml(name)).unwrap();
        if with_system_md {
            std::fs::write(p.join("system.md"), "# {{input}}\n").unwrap();
        }
    }

    #[test]
    fn empty_plugins_dir_returns_empty_registry() {
        let tmp = TempDir::new().unwrap();
        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        assert!(reg.is_empty());
    }

    // ─────────────────────────────────────────────────────────────────
    // Pattern plugin loader tests — SPRINT-PATTERN-SUBSTRATE-V1 Phase 1.4
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn pattern_with_system_md_loads_cleanly() {
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        seed_pattern(&plugins, "pattern-summarize", true);

        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
        let p = reg.get("pattern-summarize").unwrap();
        assert_eq!(p.manifest.plugin.kind, PluginKind::Pattern);
        assert!(p.manifest.pattern.is_some());
    }

    #[test]
    fn pattern_without_system_md_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        // Pattern dir with plugin.toml but missing system.md — should
        // be skipped with a warning, not crash boot.
        seed_pattern(&plugins, "pattern-broken", false);
        // Add a healthy skill alongside to confirm the loader keeps
        // walking after the bad pattern.
        seed(&plugins, "alpha", &skill("alpha", "1.0.0", &[]));

        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        // Only the skill survives; the pattern is graceful-skipped.
        assert_eq!(reg.len(), 1);
        assert!(reg.get("alpha").is_some());
        assert!(reg.get("pattern-broken").is_none());
    }

    #[test]
    fn skill_without_sibling_files_still_loads() {
        // The system.md requirement is pattern-only — skills don't
        // care about siblings. Regression guard for the kind-check.
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        seed(&plugins, "alpha", &skill("alpha", "1.0.0", &[]));
        let reg = PluginRegistry::load_default(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
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

    /// v0.2 A.1 regression guard: a single malformed `plugin.toml` must NOT
    /// prevent the rest of the registry from loading. Before this fix,
    /// `Manifest::load(...)?` short-circuited `load_from` and one broken
    /// plugin killed kernel boot for every subsystem (SANCHO, MCP gateway,
    /// infect renderer). Now we warn and skip.
    #[test]
    fn malformed_plugin_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();

        // Good plugin.
        seed(&plugins, "alpha", &skill("alpha", "1.0.0", &[]));

        // Three flavors of malformed neighbours:
        //   - outright TOML syntax error
        //   - parses as TOML but manifest schema rejects it
        //   - empty file
        seed(&plugins, "broken-syntax", "[plugin\nname = oops\n");
        seed(
            &plugins,
            "broken-schema",
            "[plugin]\nname = \"broken-schema\"\nversion = \"1.0.0\"\n# no kind/source\n",
        );
        seed(&plugins, "broken-empty", "");

        // And one more healthy plugin AFTER the broken ones in read order to
        // prove we don't just stop at the first fault.
        seed(&plugins, "zeta", &skill("zeta", "1.0.0", &[]));

        let reg = PluginRegistry::load_default(tmp.path())
            .expect("kernel boot must not fail on malformed manifests");
        let names: BTreeSet<&str> = reg
            .plugins()
            .iter()
            .map(|p| p.manifest.plugin.name.as_str())
            .collect();
        assert!(names.contains("alpha"), "alpha missing: {names:?}");
        assert!(names.contains("zeta"), "zeta missing: {names:?}");
        assert!(!names.contains("broken-syntax"));
        assert!(!names.contains("broken-schema"));
        assert!(!names.contains("broken-empty"));
        assert_eq!(reg.len(), 2, "only the healthy pair should load");
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
