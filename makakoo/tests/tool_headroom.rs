//! Contract tests for the `tool-headroom` plugin wrapper.

use std::path::PathBuf;

fn plugin_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir)
        .parent()
        .expect("makakoo crate has parent")
        .join("plugins-core/tool-headroom")
}

#[test]
fn wrapper_dir_has_required_files() {
    let dir = plugin_dir();
    assert!(dir.join("plugin.toml").is_file(), "plugin.toml missing");
    assert!(dir.join("install.sh").is_file(), "install.sh missing");
    assert!(dir.join("SKILL.md").is_file(), "SKILL.md missing");
    assert!(
        dir.join("fragments/default.md").is_file(),
        "bootstrap fragment missing"
    );
}

#[test]
fn plugin_toml_parses_as_mcp_tool_with_bootstrap_fragment() {
    use makakoo_core::plugin::{Manifest, PluginKind};

    let (manifest, warnings) = Manifest::load(&plugin_dir().join("plugin.toml")).unwrap();
    assert_eq!(manifest.plugin.name, "tool-headroom");
    assert_eq!(manifest.plugin.kind, PluginKind::McpTool);
    assert!(
        warnings.is_empty(),
        "plugin.toml emits warnings: {warnings:?}"
    );
    assert!(manifest.abi.mcp_tool.is_some(), "missing mcp-tool ABI");
    assert!(
        manifest.abi.bootstrap_fragment.is_some(),
        "missing bootstrap-fragment ABI"
    );
    assert_eq!(
        manifest.infect.fragments.get("default").map(String::as_str),
        Some("fragments/default.md")
    );
    assert!(manifest.install.unix.is_some(), "install.unix missing");
}

#[test]
#[cfg(unix)]
fn install_script_is_syntax_valid_bash() {
    let status = std::process::Command::new("bash")
        .arg("-n")
        .arg(plugin_dir().join("install.sh"))
        .status()
        .expect("bash -n should run");
    assert!(status.success(), "install.sh must parse as bash");
}
