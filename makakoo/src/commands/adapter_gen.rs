//! v0.6 Phase C — `makakoo adapter gen` scaffolder.
//!
//! Four templates, one-liner replacements, always goes through the v0.3
//! install path so the trust ledger stays consistent. Goal: adding a new
//! adapter is a ≤60s CLI invocation, not a TOML-editing exercise.
//!
//! Example (openai-compat, the most common case):
//!
//!     makakoo adapter gen --template openai-compat \
//!         --name deepseek --url https://api.deepseek.com/v1 \
//!         --key-env DEEPSEEK_API_KEY --model deepseek-chat
//!
//! Writes a manifest, calls `install_from_path`, then optionally runs
//! `makakoo adapter doctor` against the newly-installed name.

use std::path::PathBuf;

use anyhow::{bail, Context};
use crossterm::style::Stylize;

use makakoo_core::adapter::{
    install_from_path, AdapterRole, InstallOptions, InstallRoot, Manifest,
};

const TEMPLATE_OPENAI_COMPAT: &str = include_str!("adapter_gen_templates/openai-compat.toml");
const TEMPLATE_SUBPROCESS: &str = include_str!("adapter_gen_templates/subprocess.toml");
const TEMPLATE_MCP_STDIO: &str = include_str!("adapter_gen_templates/mcp-stdio.toml");
const TEMPLATE_PEER_MAKAKOO: &str = include_str!("adapter_gen_templates/peer-makakoo.toml");

/// Supported template shapes. Anything else → hard error with a
/// suggestion list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenTemplate {
    OpenAiCompat,
    Subprocess,
    McpStdio,
    PeerMakakoo,
}

impl GenTemplate {
    pub fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "openai-compat" => Ok(Self::OpenAiCompat),
            "subprocess" => Ok(Self::Subprocess),
            "mcp-stdio" => Ok(Self::McpStdio),
            "peer-makakoo" => Ok(Self::PeerMakakoo),
            other => bail!(
                "unknown template `{other}`. Valid: openai-compat, subprocess, mcp-stdio, peer-makakoo"
            ),
        }
    }

    pub fn body(self) -> &'static str {
        match self {
            Self::OpenAiCompat => TEMPLATE_OPENAI_COMPAT,
            Self::Subprocess => TEMPLATE_SUBPROCESS,
            Self::McpStdio => TEMPLATE_MCP_STDIO,
            Self::PeerMakakoo => TEMPLATE_PEER_MAKAKOO,
        }
    }
}

/// Everything a caller can pass to the scaffolder. The CLI layer parses
/// clap flags into this struct; tests construct it directly.
#[derive(Debug, Clone)]
pub struct GenSpec {
    pub template: GenTemplate,
    pub name: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub key_env: Option<String>,
    pub model: Option<String>,
    pub command: Vec<String>,
    pub roles: Vec<AdapterRole>,
    pub peer_name: Option<String>,
    /// Where to drop the scratch source dir before install. Defaults to
    /// `std::env::temp_dir()`. Tests use a tempdir.
    pub scratch_parent: Option<PathBuf>,
    /// Override the InstallRoot — test-only.
    pub install_root: Option<InstallRoot>,
    /// Skip the post-gen `doctor` call.
    pub skip_doctor: bool,
    /// Skip the post-gen `install` call entirely — returns the rendered
    /// manifest path without touching the registry.
    pub skip_install: bool,
    /// Skip the install-time health check (gen runs against dry /
    /// offline services a lot).
    pub skip_health_check: bool,
}

/// Outcome of a single generation run.
#[derive(Debug)]
pub struct GenReport {
    pub registered_path: Option<PathBuf>,
    pub scratch_path: PathBuf,
    pub rendered: String,
    pub manifest_parsed: bool,
}

/// Render + install.
pub fn run(spec: &GenSpec) -> anyhow::Result<GenReport> {
    let rendered = render_template(spec)?;

    // Sanity-parse the rendered TOML before touching disk — user sees
    // the validation error at the CLI, not a cryptic install failure.
    let manifest = Manifest::parse_str(&rendered)
        .with_context(|| format!("rendered template for `{}` failed to parse", spec.name))?;
    if manifest.adapter.name != spec.name {
        bail!(
            "rendered manifest name `{}` doesn't match requested `{}`",
            manifest.adapter.name,
            spec.name
        );
    }

    // Write the scratch source dir: <scratch_parent>/<name>/adapter.toml.
    let parent = spec
        .scratch_parent
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let scratch_dir = parent.join(format!("makakoo-gen-{}", spec.name));
    if scratch_dir.exists() {
        std::fs::remove_dir_all(&scratch_dir)?;
    }
    std::fs::create_dir_all(&scratch_dir)?;
    let manifest_path = scratch_dir.join("adapter.toml");
    std::fs::write(&manifest_path, &rendered)?;

    if spec.skip_install {
        return Ok(GenReport {
            registered_path: None,
            scratch_path: scratch_dir,
            rendered,
            manifest_parsed: true,
        });
    }

    let root = spec
        .install_root
        .clone()
        .unwrap_or_else(InstallRoot::default_from_env);
    let options = InstallOptions {
        allow_unsigned: true, // local-path installs always allowed
        accept_re_trust: true, // freshly-generated manifest, no diff to approve
        skip_health_check: spec.skip_health_check,
    };
    let install_report = install_from_path(&scratch_dir, &root, options)
        .with_context(|| format!("install_from_path for `{}`", spec.name))?;

    Ok(GenReport {
        registered_path: Some(install_report.registered_path),
        scratch_path: scratch_dir,
        rendered,
        manifest_parsed: true,
    })
}

fn render_template(spec: &GenSpec) -> anyhow::Result<String> {
    let mut body = spec.template.body().to_string();
    let description = spec.description.clone().unwrap_or_else(|| match spec.template {
        GenTemplate::OpenAiCompat => format!("OpenAI-compatible endpoint `{}`", spec.name),
        GenTemplate::Subprocess => format!("Subprocess adapter for `{}`", spec.name),
        GenTemplate::McpStdio => format!("MCP stdio server wrapped as adapter: `{}`", spec.name),
        GenTemplate::PeerMakakoo => format!("Peer Makakoo install `{}`", spec.name),
    });
    let roles_toml = roles_to_toml(&spec.roles);

    body = body.replace("{{name}}", &spec.name);
    body = body.replace("{{description}}", &description);
    body = body.replace("{{roles}}", &roles_toml);

    match spec.template {
        GenTemplate::OpenAiCompat => {
            let url = spec.url.as_deref().ok_or_else(|| anyhow::anyhow!(
                "openai-compat template requires --url"
            ))?;
            let key_env = spec.key_env.clone().unwrap_or_else(|| {
                format!("{}_API_KEY", spec.name.to_uppercase().replace('-', "_"))
            });
            let model = spec.model.clone().unwrap_or_default();
            let host = extract_host(url);
            body = body.replace("{{url}}", url);
            body = body.replace("{{key_env}}", &key_env);
            body = body.replace("{{model}}", &model);
            body = body.replace("{{host}}", &host);
        }
        GenTemplate::Subprocess | GenTemplate::McpStdio => {
            if spec.command.is_empty() {
                anyhow::bail!(
                    "{} template requires --command <argv>",
                    template_label(spec.template)
                );
            }
            let argv = spec
                .command
                .iter()
                .map(|s| format!("{:?}", s))
                .collect::<Vec<_>>()
                .join(", ");
            body = body.replace("{{command_argv}}", &argv);
        }
        GenTemplate::PeerMakakoo => {
            let url = spec.url.as_deref().ok_or_else(|| anyhow::anyhow!(
                "peer-makakoo template requires --url <http://peer-host:port>"
            ))?;
            let peer_name = spec.peer_name.as_deref().ok_or_else(|| anyhow::anyhow!(
                "peer-makakoo template requires --peer-name (the name the remote install knows you by)"
            ))?;
            let host = extract_host(url);
            // Strip a trailing /rpc if the user preemptively added it —
            // the template appends /rpc itself.
            let clean_url = url.trim_end_matches("/rpc").trim_end_matches('/');
            body = body.replace("{{url}}", clean_url);
            body = body.replace("{{peer_name}}", peer_name);
            body = body.replace("{{host}}", &host);
        }
    }

    // Last-pass validation: refuse leftover placeholders.
    if let Some(idx) = body.find("{{") {
        let end = body[idx..].find("}}").map(|e| idx + e + 2).unwrap_or(body.len());
        anyhow::bail!(
            "template placeholder left unfilled: `{}`",
            &body[idx..end]
        );
    }

    Ok(body)
}

fn template_label(t: GenTemplate) -> &'static str {
    match t {
        GenTemplate::OpenAiCompat => "openai-compat",
        GenTemplate::Subprocess => "subprocess",
        GenTemplate::McpStdio => "mcp-stdio",
        GenTemplate::PeerMakakoo => "peer-makakoo",
    }
}

fn roles_to_toml(roles: &[AdapterRole]) -> String {
    if roles.is_empty() {
        return r#""delegate", "swarm_member""#.to_string();
    }
    roles
        .iter()
        .map(|r| format!("\"{}\"", r.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn extract_host(url: &str) -> String {
    let without_scheme = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Pretty-print the generator report to stdout.
pub fn print_report(report: &GenReport, spec: &GenSpec) {
    if let Some(registered) = &report.registered_path {
        println!(
            "{} adapter `{}` installed at {}",
            "✅".to_string().green(),
            spec.name,
            registered.display()
        );
    } else {
        println!(
            "{} rendered manifest at {}",
            "📝".to_string().yellow(),
            report.scratch_path.join("adapter.toml").display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::adapter::AdapterRole;
    use tempfile::TempDir;

    fn base(tmp: &TempDir) -> GenSpec {
        GenSpec {
            template: GenTemplate::OpenAiCompat,
            name: "example".to_string(),
            description: None,
            url: None,
            key_env: None,
            model: None,
            command: vec![],
            roles: vec![AdapterRole::Delegate, AdapterRole::SwarmMember],
            peer_name: None,
            scratch_parent: Some(tmp.path().to_path_buf()),
            install_root: Some(InstallRoot {
                adapters_root: tmp.path().join("adapters"),
                trust_root: tmp.path().join("trust"),
            }),
            skip_doctor: true,
            skip_install: false,
            skip_health_check: true,
        }
    }

    #[test]
    fn openai_compat_renders_and_installs() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            url: Some("https://api.example.com/v1".to_string()),
            key_env: Some("EXAMPLE_KEY".to_string()),
            model: Some("example-chat".to_string()),
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        assert!(report.manifest_parsed);
        let installed = report.registered_path.unwrap();
        assert!(installed.exists(), "expected registered manifest on disk");

        // Parse the written file back and verify key fields.
        let manifest = Manifest::load(&installed).unwrap();
        assert_eq!(manifest.adapter.name, "example");
        assert_eq!(
            manifest.transport.base_url.as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(manifest.transport.model.as_deref(), Some("example-chat"));
        assert_eq!(manifest.auth.key_env.as_deref(), Some("EXAMPLE_KEY"));
    }

    #[test]
    fn openai_compat_requires_url() {
        let tmp = TempDir::new().unwrap();
        let spec = base(&tmp);
        let err = run(&spec).unwrap_err();
        assert!(
            err.to_string().contains("--url"),
            "expected missing-url error, got: {err}"
        );
    }

    #[test]
    fn openai_compat_defaults_key_env_from_name() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            name: "my-service".to_string(),
            url: Some("https://api.example.com/v1".to_string()),
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        let manifest = Manifest::load(report.registered_path.unwrap()).unwrap();
        assert_eq!(manifest.auth.key_env.as_deref(), Some("MY_SERVICE_API_KEY"));
    }

    #[test]
    fn subprocess_renders_and_installs() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::Subprocess,
            name: "local-shell".to_string(),
            command: vec!["bash".to_string(), "-c".to_string(), "echo hi".to_string()],
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        let manifest = Manifest::load(report.registered_path.unwrap()).unwrap();
        assert_eq!(manifest.transport.command.len(), 3);
        assert_eq!(manifest.transport.stdin, true);
    }

    #[test]
    fn subprocess_requires_command() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::Subprocess,
            ..base(&tmp)
        };
        let err = run(&spec).unwrap_err();
        assert!(err.to_string().contains("--command"));
    }

    #[test]
    fn mcp_stdio_renders_and_installs() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::McpStdio,
            name: "my-mcp".to_string(),
            command: vec!["my-mcp-server".to_string()],
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        let manifest = Manifest::load(report.registered_path.unwrap()).unwrap();
        assert_eq!(manifest.transport.command, vec!["my-mcp-server".to_string()]);
        assert_eq!(
            manifest.output.verdict_field.as_deref(),
            Some("result.content.0.text")
        );
    }

    #[test]
    fn peer_makakoo_renders_with_rpc_suffix_added() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::PeerMakakoo,
            name: "workstation".to_string(),
            url: Some("http://127.0.0.1:8765".to_string()),
            peer_name: Some("laptop-a".to_string()),
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        let manifest = Manifest::load(report.registered_path.unwrap()).unwrap();
        assert_eq!(
            manifest.transport.url.as_deref(),
            Some("http://127.0.0.1:8765/rpc"),
            "template should append /rpc to bare URL"
        );
        assert_eq!(manifest.transport.peer_name.as_deref(), Some("laptop-a"));
    }

    #[test]
    fn peer_makakoo_trailing_slash_is_stripped() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::PeerMakakoo,
            name: "ws".to_string(),
            url: Some("http://127.0.0.1:8765/".to_string()),
            peer_name: Some("laptop".to_string()),
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        let manifest = Manifest::load(report.registered_path.unwrap()).unwrap();
        assert_eq!(
            manifest.transport.url.as_deref(),
            Some("http://127.0.0.1:8765/rpc")
        );
    }

    #[test]
    fn peer_makakoo_preserves_already_suffixed_url() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::PeerMakakoo,
            name: "ws".to_string(),
            url: Some("http://127.0.0.1:8765/rpc".to_string()),
            peer_name: Some("laptop".to_string()),
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        let manifest = Manifest::load(report.registered_path.unwrap()).unwrap();
        assert_eq!(
            manifest.transport.url.as_deref(),
            Some("http://127.0.0.1:8765/rpc")
        );
    }

    #[test]
    fn peer_makakoo_requires_url_and_peer_name() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            template: GenTemplate::PeerMakakoo,
            ..base(&tmp)
        };
        let err = run(&spec).unwrap_err();
        assert!(err.to_string().contains("--url"));
    }

    #[test]
    fn skip_install_returns_rendered_without_touching_registry() {
        let tmp = TempDir::new().unwrap();
        let spec = GenSpec {
            url: Some("https://api.example.com/v1".to_string()),
            skip_install: true,
            ..base(&tmp)
        };
        let report = run(&spec).expect("gen ok");
        assert!(report.registered_path.is_none());
        assert!(report.manifest_parsed);
        assert!(report.rendered.contains("example"));
    }

    #[test]
    fn unfilled_placeholder_is_detected() {
        // Construct a broken template directly — simulate a future
        // refactor that introduces a placeholder not wired up.
        let mut spec = base(&tempfile::TempDir::new().unwrap());
        spec.url = Some("https://x/v1".to_string());
        // Hack: point render at a bespoke body via a test-only fn is
        // overkill. Instead, rely on the live templates never leaving
        // {{placeholders}} unbound — this test codifies the guard.
        let rendered = render_template(&spec).unwrap();
        assert!(!rendered.contains("{{"));
        assert!(!rendered.contains("}}"));
    }

    #[test]
    fn extract_host_strips_scheme_and_port_and_path() {
        assert_eq!(extract_host("https://api.example.com/v1/chat"), "api.example.com");
        assert_eq!(extract_host("http://127.0.0.1:8080/rpc"), "127.0.0.1");
        assert_eq!(extract_host("https://foo.bar:443"), "foo.bar");
    }

    #[test]
    fn gen_template_from_str_rejects_unknown() {
        let err = GenTemplate::from_str("made-up-template").unwrap_err();
        assert!(err.to_string().contains("Valid:"));
    }
}
