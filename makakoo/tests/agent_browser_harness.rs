//! Integration tests for the flagship `agent-browser-harness` plugin.
//!
//! Every test here operates on the plugin wrapper dir shipped in
//! `plugins-core/agent-browser-harness/` (never on a live install).
//!
//! Tests that require real Chrome (`daemon start` / `harvey_browse`
//! end-to-end) live outside this file and are gated `#[ignore]` in
//! the per-handler test module.

#![cfg(unix)]

use std::path::PathBuf;

fn plugin_wrapper_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("makakoo crate has parent")
        .join("plugins-core/agent-browser-harness")
}

#[test]
fn wrapper_dir_has_required_files() {
    let dir = plugin_wrapper_dir();
    assert!(dir.join("plugin.toml").is_file(), "plugin.toml missing");
    assert!(dir.join("install.sh").is_file(), "install.sh missing");
    assert!(
        dir.join("daemon_admin.py").is_file(),
        "daemon_admin.py missing"
    );
}

#[test]
fn install_sh_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let install_sh = plugin_wrapper_dir().join("install.sh");
    let mode = std::fs::metadata(&install_sh).unwrap().permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "install.sh must be executable (mode: {mode:o})"
    );
}

#[test]
fn plugin_toml_parses_as_agent_kind_with_mcp_tool() {
    use makakoo_core::plugin::{Manifest, PluginKind};

    let manifest_path = plugin_wrapper_dir().join("plugin.toml");
    let (manifest, warnings) = Manifest::load(&manifest_path).unwrap();
    assert_eq!(manifest.plugin.name, "agent-browser-harness");
    assert_eq!(manifest.plugin.kind, PluginKind::Agent);
    assert!(warnings.is_empty(), "plugin.toml emits warnings: {warnings:?}");

    // Must declare the harvey_browse MCP tool (Phase E contract).
    let tool_names: Vec<&str> = manifest
        .mcp
        .tools
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        tool_names.contains(&"harvey_browse"),
        "harvey_browse not in plugin.mcp.tools: {tool_names:?}"
    );

    // install.unix must be set so the installer runs install.sh.
    assert!(
        manifest.install.unix.is_some(),
        "install.unix must be declared"
    );

    // Entrypoint triplet must be complete — agent kind requires it.
    assert!(manifest.entrypoint.start.is_some(), "entrypoint.start missing");
    assert!(manifest.entrypoint.stop.is_some(), "entrypoint.stop missing");
    assert!(
        manifest.entrypoint.health.is_some(),
        "entrypoint.health missing"
    );
}

#[test]
fn plugin_toml_declares_required_capability_grants() {
    use makakoo_core::plugin::Manifest;
    let manifest_path = plugin_wrapper_dir().join("plugin.toml");
    let (manifest, _) = Manifest::load(&manifest_path).unwrap();

    // These three verbs are load-bearing — dropping any of them makes
    // the plugin unusable even post-install.
    let required = [
        "net/http:127.0.0.1",
        "fs/write:$MAKAKOO_HOME/plugins/agent-browser-harness",
        "exec/shell",
    ];
    for expected in required {
        assert!(
            manifest.capabilities.grants.iter().any(|g| g == expected),
            "missing required grant `{expected}` — have: {:?}",
            manifest.capabilities.grants
        );
    }
}

#[test]
fn install_sh_references_venv_bootstrap_and_upstream_clone() {
    let install_sh = plugin_wrapper_dir().join("install.sh");
    let body = std::fs::read_to_string(&install_sh).unwrap();
    assert!(
        body.contains("makakoo-venv-bootstrap"),
        "install.sh must call makakoo-venv-bootstrap"
    );
    assert!(
        body.contains("git clone") || body.contains("git -C"),
        "install.sh must clone upstream browser-harness"
    );
    assert!(
        body.contains("browser-use/browser-harness"),
        "install.sh must point at the canonical upstream"
    );
}

#[test]
fn daemon_admin_has_start_stop_health_doctor_commands() {
    let admin = plugin_wrapper_dir().join("daemon_admin.py");
    let body = std::fs::read_to_string(&admin).unwrap();
    for expected in ["cmd_start", "cmd_stop", "cmd_health", "cmd_doctor"] {
        assert!(
            body.contains(expected),
            "daemon_admin.py missing function `{expected}`"
        );
    }
}

#[test]
fn harvey_browse_mcp_tool_is_declared_in_manifest() {
    // The MCP-side registry check lives in
    // makakoo-mcp/src/handlers/tier_b/browse.rs::tests (handler unit
    // tests + the register_all contract suite in handlers/mod.rs).
    // This integration test locks only the manifest → registry-name
    // contract: if anything ever declares a tool name that doesn't
    // match what the MCP handler exposes, one side will drift.
    use makakoo_core::plugin::Manifest;

    let manifest_path = plugin_wrapper_dir().join("plugin.toml");
    let (manifest, _) = Manifest::load(&manifest_path).unwrap();
    let declared: Vec<&str> = manifest
        .mcp
        .tools
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        declared.contains(&"harvey_browse"),
        "plugin.toml must declare harvey_browse — had {declared:?}"
    );
}
