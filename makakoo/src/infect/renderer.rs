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
}
