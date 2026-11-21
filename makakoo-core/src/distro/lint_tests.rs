//! Unit tests for strict headless distro linting — one test per forbidden
//! item plus shipped `distros/` and combined bad-distro fixture coverage.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

fn write_plugin(root: &Path, name: &str, extra: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!(
        r#"
[plugin]
name = "{name}"
version = "0.1.0"
kind = "skill"
language = "python"

[source]
path = "plugins-core/{name}"

[abi]
skill = "^1.0"

[entrypoint]
run = "python3 run.py"
{extra}
"#
    );
    std::fs::write(dir.join("plugin.toml"), body).unwrap();
}

fn write_distro(dir: &Path, name: &str, plugins: &[&str]) -> PathBuf {
    let mut s = format!("[distro]\nname = \"{name}\"\n\n[plugins]\n");
    for p in plugins {
        s.push_str(&format!("\"{p}\" = \"*\"\n"));
    }
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(&path, s).unwrap();
    path
}

fn lint_single(name: &str, extra: &str, allowed_net: Vec<String>) -> DistroLintReport {
    let tmp = TempDir::new().unwrap();
    let plugins_root = tmp.path().join("plugins");
    write_plugin(&plugins_root, name, extra);
    let distro = write_distro(tmp.path(), "test-distro", &[name]);
    lint_distro_file(
        &distro,
        &LintOptions {
            plugins_root,
            allowed_net,
        },
    )
    .unwrap()
}

#[test]
fn fails_on_exec_shell() {
    let r = lint_single(
        "bad-shell",
        r#"
[capabilities]
grants = ["exec/shell"]

[state]
dir = "$MAKAKOO_HOME/state/bad-shell"
retention = "purge_on_uninstall"
"#,
        vec![],
    );
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::ExecShell));
}

#[test]
fn fails_on_exec_binary() {
    let r = lint_single(
        "bad-bin",
        r#"
[capabilities]
grants = ["exec/binary:curl"]
"#,
        vec![],
    );
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::ExecBinary));
}

#[test]
fn fails_on_install_script() {
    let r = lint_single(
        "bad-install",
        r#"
[install]
unix = "install.sh"
"#,
        vec![],
    );
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::InstallScript));
}

#[test]
fn fails_on_secrets_read() {
    let r = lint_single(
        "bad-secret",
        r#"
[capabilities]
grants = ["secrets/read:OPENAI_API_KEY"]
"#,
        vec![],
    );
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::SecretRead));
}

#[test]
fn fails_on_mcp_registration() {
    let r = lint_single(
        "bad-mcp",
        r#"
[capabilities]
grants = ["mcp/register:custom"]
"#,
        vec![],
    );
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::McpRegistration));

    let tmp = TempDir::new().unwrap();
    let plugins_root = tmp.path().join("plugins");
    let dir = plugins_root.join("bad-mcp-kind");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        r#"
[plugin]
name = "bad-mcp-kind"
version = "0.1.0"
kind = "mcp-tool"
language = "python"

[source]
path = "plugins-core/bad-mcp-kind"

[abi]
"mcp-tool" = "^1.0"

[mcp]
tools = [{ name = "custom_lookup", handler = "mod:fn" }]
"#,
    )
    .unwrap();
    let distro = write_distro(tmp.path(), "mcp-distro", &["bad-mcp-kind"]);
    let r2 = lint_distro_file(
        &distro,
        &LintOptions {
            plugins_root,
            allowed_net: vec![],
        },
    )
    .unwrap();
    assert!(!r2.ok);
    assert!(r2
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::McpRegistration));
}

#[test]
fn fails_on_brain_delete() {
    let r = lint_single(
        "bad-delete",
        r#"
[capabilities]
grants = ["brain/delete"]
"#,
        vec![],
    );
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::BrainDelete));
}

#[test]
fn fails_on_unlisted_network_grant_and_passes_when_allowlisted() {
    let extra = r#"
[capabilities]
grants = ["net/http:http://127.0.0.1:18080/v1/*"]
"#;
    let denied = lint_single("net-plugin", extra, vec![]);
    assert!(!denied.ok);
    assert!(denied
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::UnlistedNetwork));

    let allowed = lint_single("net-plugin", extra, vec!["http://127.0.0.1:18080/*".into()]);
    assert!(
        allowed.ok,
        "unexpected violations: {:?}",
        allowed.violations
    );

    let wildcard = lint_single(
        "net-wildcard",
        r#"
[capabilities]
grants = ["net/http:*"]
"#,
        vec!["http://127.0.0.1:18080/*".into()],
    );
    assert!(!wildcard.ok);
    assert!(wildcard
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::UnlistedNetwork));
}

#[test]
fn fails_on_send_tool() {
    let by_name = lint_single("skill-outbound-mailer", "", vec![]);
    assert!(!by_name.ok);
    assert!(by_name
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::SendTool));

    let by_tool = lint_single(
        "helper-notifier",
        r#"
[mcp]
tools = [{ name = "send_email_blast", handler = "mod:send" }]
"#,
        vec![],
    );
    assert!(!by_tool.ok);
    assert!(by_tool
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::SendTool));
}

#[test]
fn fails_closed_on_missing_plugin_manifest() {
    let tmp = TempDir::new().unwrap();
    let plugins_root = tmp.path().join("plugins");
    std::fs::create_dir_all(&plugins_root).unwrap();
    let distro = write_distro(tmp.path(), "ghost-distro", &["ghost-plugin"]);
    let r = lint_distro_file(
        &distro,
        &LintOptions {
            plugins_root,
            allowed_net: vec![],
        },
    )
    .unwrap();
    assert!(!r.ok);
    assert!(r
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::MissingManifest));
}

#[test]
fn clean_plugin_passes_lint() {
    let r = lint_single(
        "clean-brain-helper",
        r#"
[capabilities]
grants = ["brain/read", "brain/write", "llm/chat"]
"#,
        vec![],
    );
    assert!(r.ok, "unexpected violations: {:?}", r.violations);
    assert_eq!(r.checked_plugins, 1);
}

#[test]
fn bad_distro_fixture_fails_all_forbidden_classes() {
    let tmp = TempDir::new().unwrap();
    let plugins_root = tmp.path().join("plugins");
    write_plugin(
        &plugins_root,
        "skill-outbound-everything",
        r#"
[install]
unix = "setup.sh"

[capabilities]
grants = [
    "exec/shell",
    "exec/binary:git",
    "secrets/read:SECRET_TOKEN",
    "mcp/register:evil",
    "brain/delete",
    "net/http:https://exfil.example.com/*",
]

[state]
dir = "$MAKAKOO_HOME/state/skill-outbound-everything"
retention = "purge_on_uninstall"
"#,
    );
    let distro = write_distro(tmp.path(), "bad-fixture", &["skill-outbound-everything"]);
    let r = lint_distro_file(
        &distro,
        &LintOptions {
            plugins_root,
            allowed_net: vec![],
        },
    )
    .unwrap();
    assert!(!r.ok);
    let rules: BTreeSet<DistroLintRule> = r.violations.iter().map(|v| v.rule).collect();
    for expected in [
        DistroLintRule::ExecShell,
        DistroLintRule::ExecBinary,
        DistroLintRule::InstallScript,
        DistroLintRule::SecretRead,
        DistroLintRule::McpRegistration,
        DistroLintRule::BrainDelete,
        DistroLintRule::UnlistedNetwork,
        DistroLintRule::SendTool,
    ] {
        assert!(rules.contains(&expected), "missing rule {expected:?}");
    }
}

#[test]
fn shipped_distros_behave_as_expected() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let distros = repo_root.join("distros");
    let plugins_root = repo_root.join("plugins-core");
    if !distros.is_dir() || !plugins_root.is_dir() {
        return;
    }
    let opts = LintOptions {
        plugins_root,
        allowed_net: vec![],
    };

    let minimal = lint_distro_file(&distros.join("minimal.toml"), &opts).unwrap();
    assert!(
        minimal.ok,
        "minimal.toml must pass strict headless lint: {:?}",
        minimal.violations
    );

    let core = lint_distro_file(&distros.join("core.toml"), &opts).unwrap();
    assert!(!core.ok, "core.toml must fail strict headless lint");
    assert!(core.violations.iter().any(|v| {
        v.rule == DistroLintRule::ExecShell || v.rule == DistroLintRule::McpRegistration
    }));

    let sebastian = lint_distro_file(&distros.join("sebastian.toml"), &opts).unwrap();
    assert!(
        !sebastian.ok,
        "sebastian.toml must fail strict headless lint"
    );

    let fed = lint_distro_file(&distros.join("federation.toml"), &opts).unwrap();
    assert!(!fed.ok, "federation.toml must fail strict headless lint");
    assert!(fed
        .violations
        .iter()
        .any(|v| v.rule == DistroLintRule::SendTool));
}
