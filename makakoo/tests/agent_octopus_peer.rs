//! Contract tests for the `agent-octopus-peer` plugin wrapper.

#![cfg(unix)]

use std::path::PathBuf;

fn plugin_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("makakoo crate has parent")
        .join("plugins-core/agent-octopus-peer")
}

#[test]
fn plugin_manifest_entrypoints_are_executable_from_plugin_cwd() {
    use makakoo_core::plugin::{Manifest, PluginKind};

    let (manifest, warnings) = Manifest::load(&plugin_dir().join("plugin.toml")).unwrap();
    assert_eq!(manifest.plugin.name, "agent-octopus-peer");
    assert_eq!(manifest.plugin.kind, PluginKind::Agent);
    assert!(
        warnings.is_empty(),
        "plugin.toml emits warnings: {warnings:?}"
    );
    assert!(
        manifest
            .depends
            .plugins
            .contains(&"lib-harvey-core".to_string()),
        "agent-octopus-peer must install after lib-harvey-core so Octopus Python deps can be bootstrapped"
    );
    assert_eq!(
        manifest.entrypoint.start.as_deref(),
        Some("./install.sh start")
    );
    assert_eq!(
        manifest.entrypoint.stop.as_deref(),
        Some("./install.sh stop")
    );
    assert_eq!(
        manifest.entrypoint.health.as_deref(),
        Some("./install.sh health")
    );
}

#[test]
fn install_script_is_syntax_valid_bash() {
    let status = std::process::Command::new("bash")
        .arg("-n")
        .arg(plugin_dir().join("install.sh"))
        .status()
        .expect("bash -n should run");
    assert!(status.success(), "install.sh must parse as bash");
}

#[test]
fn install_script_sets_makakoo_mcp_bin_for_service_units() {
    let body = std::fs::read_to_string(plugin_dir().join("install.sh")).unwrap();

    assert!(
        body.contains("_resolve_makakoo_mcp_bin"),
        "install.sh must resolve makakoo-mcp across release and cargo install methods"
    );
    assert!(
        body.contains("${HOME}/.local/bin/makakoo-mcp"),
        "curl-pipe release installs place makakoo-mcp under ~/.local/bin"
    );
    assert!(
        body.contains("${HOME}/.cargo/bin/makakoo-mcp"),
        "developer cargo installs place makakoo-mcp under ~/.cargo/bin"
    );
    assert!(
        body.contains("MAKAKOO_MCP_BIN=\"$(_resolve_makakoo_mcp_bin)\""),
        "resolved makakoo-mcp path must be exported into generated units"
    );
    assert!(
        body.contains("_ensure_lib_harvey_core_venv"),
        "install/start must bootstrap lib-harvey-core's venv for cryptography"
    );
    assert!(
        body.contains("cryptography>=41,<46"),
        "Octopus signed-MCP requires cryptography on fresh installs"
    );
    assert!(
        body.contains("LIB_HARVEY_CORE_VENV_PYTHON"),
        "HTTP shim must run under lib-harvey-core's venv when available"
    );
    assert!(
        body.contains("<key>MAKAKOO_MCP_BIN</key><string>${MAKAKOO_MCP_BIN}</string>"),
        "launchd unit must pass MAKAKOO_MCP_BIN to http_shim.py"
    );
    assert!(
        body.contains("Environment=\"MAKAKOO_MCP_BIN=${MAKAKOO_MCP_BIN}\""),
        "systemd unit must pass MAKAKOO_MCP_BIN to http_shim.py"
    );
    assert!(
        body.contains("Always refresh the unit before start"),
        "start must regenerate launchd/systemd descriptors so network activate bind changes take effect"
    );
    assert!(
        body.contains("do_install\n        ;;"),
        "plugin install invokes install.sh with no args, so the blank command must perform do_install"
    );
}

#[test]
fn brain_network_entrypoint_uses_wrapper_with_octopus_venv() {
    use makakoo_core::plugin::{Manifest, PluginKind};

    let repo_root = plugin_dir()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let brain_dir = repo_root.join("plugins-core/skill-brain-network");
    let (manifest, warnings) = Manifest::load(&brain_dir.join("plugin.toml")).unwrap();
    assert_eq!(manifest.plugin.name, "skill-brain-network");
    assert_eq!(manifest.plugin.kind, PluginKind::Skill);
    assert!(
        warnings.is_empty(),
        "plugin.toml emits warnings: {warnings:?}"
    );
    assert_eq!(
        manifest.entrypoint.run.as_deref(),
        Some("bin/brain-network")
    );

    let wrapper = std::fs::read_to_string(brain_dir.join("bin/brain-network")).unwrap();
    assert!(wrapper.contains(".venv/bin/python"));
    assert!(wrapper.contains("cryptography>=41,<46"));
    assert!(wrapper.contains("src/brain_network.py"));
}
