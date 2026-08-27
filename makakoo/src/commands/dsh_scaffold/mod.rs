//! AgentSpec -> DeepSeek Harness execution project.

mod context;
mod cordis;
mod env;
mod package_json;
mod readme;
mod runner;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use makakoo_core::agents::spec::AgentSpec;

use self::context::RenderContext;

pub const DSH_VERSION: &str = "0.1.1-rc.2";
pub const GITIGNORE: &str = "node_modules/\n.env\n.sessions/\nruntime.json\n.runtime-token\n";

pub fn scaffold_dsh_project(
    spec: &AgentSpec,
    out_dir: &Path,
    inline_secrets: &HashMap<String, String>,
) -> Result<()> {
    if out_dir.exists()
        && out_dir
            .read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    {
        anyhow::bail!(
            "deepseek-harness output dir {} already exists and is non-empty — refusing to overwrite",
            out_dir.display()
        );
    }

    let ctx = RenderContext { spec, out_dir };
    ctx.write("package.json", &package_json::render(&ctx))?;
    ctx.write("cordis.yml", cordis::CORDIS)?;
    ctx.write("runner.mjs", &runner::render(spec))?;
    ctx.write(".env.example", &env::render(spec))?;
    if !inline_secrets.is_empty() {
        ctx.write_private(".env", &env::fill(&env::render(spec), inline_secrets))?;
    }
    ctx.write(".gitignore", GITIGNORE)?;
    ctx.write("README.md", &readme::render(&ctx))?;
    ctx.write("spec.yaml", &spec.to_yaml()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::agents::spec::{AgentSpec, ScopeSpec};

    fn spec() -> AgentSpec {
        AgentSpec {
            name: "researcher".into(),
            description: "Research agent".into(),
            model: "switchailocal/ail-compound".into(),
            instructions: "Use primary evidence.".into(),
            tools: vec!["brain_search".into()],
            channels: vec![],
            triggers: vec![],
            scope: ScopeSpec::default(),
        }
    }

    #[test]
    fn scaffold_emits_pinned_mcp_only_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("agent");
        scaffold_dsh_project(&spec(), &out, &HashMap::new()).unwrap();

        let package = std::fs::read_to_string(out.join("package.json")).unwrap();
        let cordis = std::fs::read_to_string(out.join("cordis.yml")).unwrap();
        let runner = std::fs::read_to_string(out.join("runner.mjs")).unwrap();
        assert!(package.contains(DSH_VERSION));
        assert!(!package.contains("\"@deepseek-ai/dsh\""));
        assert!(cordis.contains("@deepseek-ai/dsh-mcp-client"));
        assert!(cordis.contains("MAKAKOO_AGENT_SLOT"));
        assert!(!cordis.contains("dsh-tool-bash"));
        assert!(!cordis.contains("dsh-tool-fs"));
        assert!(runner.contains("http://127.0.0.1:18080/v1"));
        assert!(out.join("spec.yaml").exists());
    }

    #[test]
    fn scaffold_refuses_non_empty_output() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("keep"), "x").unwrap();
        let error = scaffold_dsh_project(&spec(), tmp.path(), &HashMap::new()).unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_writes_inline_secrets_private() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("agent");
        let secrets = HashMap::from([("TELEGRAM_BOT_TOKEN".into(), "secret".into())]);
        scaffold_dsh_project(&spec(), &out, &secrets).unwrap();
        let mode = std::fs::metadata(out.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn generated_runner_passes_node_syntax_check_when_node_is_available() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("agent");
        scaffold_dsh_project(&spec(), &out, &HashMap::new()).unwrap();
        let status = match std::process::Command::new("node")
            .arg("--check")
            .arg(out.join("runner.mjs"))
            .status()
        {
            Ok(status) => status,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("run node --check: {error}"),
        };
        assert!(status.success());
    }
}
