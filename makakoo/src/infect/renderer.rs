//! Bootstrap fragment renderer.
//!
//! Walks the PluginRegistry in load order, reads each bootstrap-fragment
//! plugin's declared fragment files, and assembles them into the base
//! template at the `<!-- makakoo:fragments -->` insertion marker.
//!
//! Result is cached at `$MAKAKOO_HOME/config/bootstrap-cache.md`. The
//! cache is invalidated when the registry changes (plugin install/uninstall).

use std::path::Path;

use anyhow::{anyhow, Result};

use makakoo_core::plugin::manifest::PluginKind;
use makakoo_core::plugin::PluginRegistry;

/// The marker line where fragments are inserted in the base template.
const FRAGMENT_MARKER: &str = "<!-- makakoo:fragments -->";

/// The base template is compiled into the binary so it's always available.
const BASE_TEMPLATE: &str = include_str!("bootstrap-base.md");

/// Render the full bootstrap by inserting plugin fragments into the
/// base template. Fragments are inserted in plugin load order at the
/// `<!-- makakoo:fragments -->` marker.
///
/// If `host` is provided, the renderer prefers a host-specific fragment
/// file (e.g. `fragments/claude.md`) over the `default` fragment.
pub fn render(
    registry: &PluginRegistry,
    makakoo_home: &Path,
    host: Option<&str>,
) -> Result<String> {
    let mut fragments = Vec::new();

    for plugin in registry.plugins() {
        if plugin.manifest.plugin.kind != PluginKind::BootstrapFragment {
            continue;
        }
        let frag_map = &plugin.manifest.infect.fragments;
        if frag_map.is_empty() {
            continue;
        }

        // Prefer host-specific fragment, fall back to "default".
        let frag_key = host
            .and_then(|h| frag_map.get(h).map(|_| h))
            .unwrap_or("default");

        let Some(frag_path) = frag_map.get(frag_key) else {
            // No matching fragment for this host or default — skip.
            continue;
        };

        // Resolve fragment path relative to plugin root or $MAKAKOO_HOME.
        let resolved = if frag_path.starts_with('/') {
            std::path::PathBuf::from(frag_path)
        } else {
            // Try plugin root first (in-tree plugins-core/).
            let in_root = plugin.root.join(frag_path);
            if in_root.exists() {
                in_root
            } else {
                // Fall back to $MAKAKOO_HOME-relative (installed plugins).
                makakoo_home
                    .join("plugins")
                    .join(&plugin.manifest.plugin.name)
                    .join(frag_path)
            }
        };

        match std::fs::read_to_string(&resolved) {
            Ok(content) => {
                fragments.push(content.trim().to_string());
            }
            Err(e) => {
                tracing::warn!(
                    plugin = %plugin.manifest.plugin.name,
                    path = %resolved.display(),
                    error = %e,
                    "skipping unreadable bootstrap fragment"
                );
            }
        }
    }

    // Inject fragments at the marker, or append if marker is missing.
    let fragment_block = fragments.join("\n\n");
    let rendered = if BASE_TEMPLATE.contains(FRAGMENT_MARKER) {
        BASE_TEMPLATE.replace(FRAGMENT_MARKER, &fragment_block)
    } else {
        format!("{}\n\n{}\n", BASE_TEMPLATE.trim(), fragment_block)
    };

    Ok(rendered.trim_end().to_string() + "\n")
}

/// Render and write to cache. Returns the rendered content.
pub fn render_and_cache(
    registry: &PluginRegistry,
    makakoo_home: &Path,
    host: Option<&str>,
) -> Result<String> {
    let rendered = render(registry, makakoo_home, host)?;
    let cache_path = makakoo_home.join("config/bootstrap-cache.md");
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&cache_path, &rendered)
        .map_err(|e| anyhow!("failed to write bootstrap cache: {e}"))?;
    Ok(rendered)
}

/// Load from cache if it exists, otherwise render fresh.
pub fn load_or_render(
    registry: &PluginRegistry,
    makakoo_home: &Path,
    host: Option<&str>,
) -> Result<String> {
    let cache_path = makakoo_home.join("config/bootstrap-cache.md");
    if cache_path.exists() {
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            if !cached.is_empty() {
                return Ok(cached);
            }
        }
    }
    render_and_cache(registry, makakoo_home, host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn empty_registry() -> PluginRegistry {
        PluginRegistry::default()
    }

    #[test]
    fn render_without_fragments_returns_base() {
        let tmp = TempDir::new().unwrap();
        let result = render(&empty_registry(), tmp.path(), None).unwrap();
        assert!(result.contains("You are Harvey"));
        assert!(!result.contains(FRAGMENT_MARKER));
    }

    #[test]
    fn render_inserts_fragment_at_marker() {
        let tmp = TempDir::new().unwrap();

        // Create a plugin directory with a fragment.
        let plugin_dir = tmp.path().join("plugins/test-fragment");
        std::fs::create_dir_all(plugin_dir.join("fragments")).unwrap();
        std::fs::write(
            plugin_dir.join("fragments/default.md"),
            "<!-- test:fragment -->\n## Test Fragment\nHello world\n<!-- test:fragment-end -->",
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
[plugin]
name = "test-fragment"
version = "0.1.0"
kind = "bootstrap-fragment"
language = "python"

[source]
path = "plugins/test-fragment"

[infect.fragments]
default = "fragments/default.md"
"#,
        )
        .unwrap();

        let registry = PluginRegistry::load_from(tmp.path().join("plugins").as_path())
            .expect("registry should load");

        let result = render(&registry, tmp.path(), None).unwrap();
        assert!(result.contains("## Test Fragment"));
        assert!(result.contains("Hello world"));
        assert!(!result.contains(FRAGMENT_MARKER));
    }

    #[test]
    fn real_plugins_core_tree_renders_browser_harness_fragment() {
        // Locks the v0.4.1 integration: once
        // `bootstrap-fragment-browser-harness` is on disk at its canonical
        // path, its instruction text MUST land in the rendered bootstrap.
        // If this test breaks, either the plugin.toml stopped declaring
        // kind=bootstrap-fragment, the fragment file was renamed, or the
        // renderer changed how it collects fragments. All three are bugs.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let plugins_core = std::path::PathBuf::from(manifest_dir)
            .parent()
            .unwrap()
            .join("plugins-core");
        let plugin_dir = plugins_core.join("bootstrap-fragment-browser-harness");
        assert!(
            plugin_dir.join("plugin.toml").is_file(),
            "bootstrap-fragment-browser-harness plugin.toml missing at {}",
            plugin_dir.display()
        );
        assert!(
            plugin_dir.join("fragments/default.md").is_file(),
            "bootstrap fragment file missing"
        );

        // Spin up a scratch $MAKAKOO_HOME whose plugins/ mirrors just the
        // fragment plugin, so render() sees a registry of exactly 1.
        let tmp = TempDir::new().unwrap();
        let home_plugins = tmp.path().join("plugins").join("bootstrap-fragment-browser-harness");
        std::fs::create_dir_all(home_plugins.join("fragments")).unwrap();
        std::fs::copy(
            plugin_dir.join("plugin.toml"),
            home_plugins.join("plugin.toml"),
        )
        .unwrap();
        std::fs::copy(
            plugin_dir.join("fragments/default.md"),
            home_plugins.join("fragments/default.md"),
        )
        .unwrap();

        let registry = PluginRegistry::load_from(tmp.path().join("plugins").as_path())
            .expect("registry should load");
        let result = render(&registry, tmp.path(), None).unwrap();

        // Every trigger pattern that tells the LLM when to reach for
        // harvey_browse MUST survive renderer changes. Pin the load-bearing
        // phrases.
        assert!(
            result.contains("harvey_browse"),
            "rendered bootstrap must reference the MCP tool name"
        );
        assert!(
            result.contains("Trigger patterns"),
            "rendered bootstrap must include the trigger-pattern section"
        );
        assert!(
            result.contains("agent-browser-harness"),
            "rendered bootstrap must point at the agent plugin"
        );
    }

    #[test]
    fn cache_round_trip() {
        let tmp = TempDir::new().unwrap();
        let rendered = render_and_cache(&empty_registry(), tmp.path(), None).unwrap();
        let cached = std::fs::read_to_string(tmp.path().join("config/bootstrap-cache.md"))
            .unwrap();
        assert_eq!(rendered, cached);

        // load_or_render should return cached version.
        let loaded = load_or_render(&empty_registry(), tmp.path(), None).unwrap();
        assert_eq!(loaded, cached);
    }

    #[test]
    fn real_plugins_core_tree_renders_skill_discovery_fragment() {
        // Locks the v0.5 Phase E integration: the
        // `bootstrap-fragment-skill-discovery` plugin's body MUST land
        // in the rendered bootstrap. Teaches every infected CLI to
        // call `skill_discover` before claiming a capability —
        // load-bearing for the "don't fabricate tools" discipline.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let plugins_core = std::path::PathBuf::from(manifest_dir)
            .parent()
            .unwrap()
            .join("plugins-core");
        let plugin_dir = plugins_core.join("bootstrap-fragment-skill-discovery");
        assert!(
            plugin_dir.join("plugin.toml").is_file(),
            "bootstrap-fragment-skill-discovery plugin.toml missing at {}",
            plugin_dir.display()
        );
        assert!(
            plugin_dir.join("fragments/default.md").is_file(),
            "bootstrap fragment file missing"
        );

        let tmp = TempDir::new().unwrap();
        let home_plugins = tmp
            .path()
            .join("plugins")
            .join("bootstrap-fragment-skill-discovery");
        std::fs::create_dir_all(home_plugins.join("fragments")).unwrap();
        std::fs::copy(
            plugin_dir.join("plugin.toml"),
            home_plugins.join("plugin.toml"),
        )
        .unwrap();
        std::fs::copy(
            plugin_dir.join("fragments/default.md"),
            home_plugins.join("fragments/default.md"),
        )
        .unwrap();

        let registry = PluginRegistry::load_from(tmp.path().join("plugins").as_path())
            .expect("registry should load");
        let result = render(&registry, tmp.path(), None).unwrap();

        // Pin the load-bearing phrases — trigger patterns + hard rule
        // + the tool name. If any of these disappear, the renderer
        // change is eating guidance the LLM needs.
        assert!(
            result.contains("skill_discover"),
            "rendered bootstrap must reference the MCP tool name"
        );
        assert!(
            result.contains("Capability discovery"),
            "rendered bootstrap must include the capability-discovery section header"
        );
        assert!(
            result.contains("Trigger patterns"),
            "rendered bootstrap must include the trigger-pattern section"
        );
        assert!(
            result.contains("Hard rule"),
            "rendered bootstrap must include the don't-fabricate hard rule"
        );
    }
}
