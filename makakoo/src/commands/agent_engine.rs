//! Internal execution-engine selection behind the canonical AgentSpec.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use makakoo_core::agents::llm_provider::DiscoveredProvider;
use makakoo_core::agents::spec::AgentSpec;
use makakoo_core::agents::{AgentRuntime, AgentRuntimeEngine};

pub const ENGINE_ENV: &str = "MAKAKOO_AGENT_ENGINE";

pub fn selected_engine() -> anyhow::Result<AgentRuntimeEngine> {
    parse_engine(std::env::var(ENGINE_ENV).ok().as_deref())
}

fn parse_engine(value: Option<&str>) -> anyhow::Result<AgentRuntimeEngine> {
    match value {
        None | Some("") | Some("dsh") | Some("deepseek-harness") => {
            Ok(AgentRuntimeEngine::DeepseekHarness)
        }
        Some("flue") => Ok(AgentRuntimeEngine::Flue),
        Some(other) => anyhow::bail!("{} must be 'dsh' or 'flue', got '{}'", ENGINE_ENV, other),
    }
}

pub fn validate_model(engine: AgentRuntimeEngine, model: &str) -> anyhow::Result<()> {
    if engine != AgentRuntimeEngine::DeepseekHarness {
        return Ok(());
    }
    let model = model.trim();
    if model.is_empty() {
        anyhow::bail!("DeepSeek Harness requires a non-empty switchAILocal model");
    }
    if let Some((provider, id)) = model.split_once('/') {
        if provider != "switchailocal" || id.is_empty() {
            anyhow::bail!(
                "DeepSeek Harness routes through switchAILocal; use 'switchailocal/<model>' or an unprefixed switchAILocal model id, got '{}'",
                model
            );
        }
    }
    Ok(())
}

pub fn default_project_dir(home: &Path, slot: &str, engine: AgentRuntimeEngine) -> PathBuf {
    match engine {
        AgentRuntimeEngine::DeepseekHarness => home.join("agents-dsh").join(slot),
        AgentRuntimeEngine::Flue => home.join("agents-flue").join(slot),
    }
}

pub fn scaffold(
    engine: AgentRuntimeEngine,
    spec: &AgentSpec,
    out_dir: &Path,
    inline_secrets: &HashMap<String, String>,
    provider: Option<&DiscoveredProvider>,
) -> anyhow::Result<AgentRuntime> {
    validate_inline_secrets(inline_secrets)?;
    if out_dir.exists() {
        anyhow::bail!(
            "agent runtime output {} already exists — refusing to overwrite",
            out_dir.display()
        );
    }
    let result = match engine {
        AgentRuntimeEngine::DeepseekHarness => {
            super::dsh_scaffold::scaffold_dsh_project(spec, out_dir, inline_secrets)
        }
        AgentRuntimeEngine::Flue => {
            super::flue_scaffold::scaffold_flue_project(spec, out_dir, inline_secrets, provider)
        }
    };
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(out_dir);
        return Err(error);
    }
    Ok(AgentRuntime {
        engine,
        project_dir: out_dir.to_path_buf(),
    })
}

fn validate_inline_secrets(inline_secrets: &HashMap<String, String>) -> anyhow::Result<()> {
    for (name, value) in inline_secrets {
        if value.contains(['\r', '\n', '\0']) {
            anyhow::bail!(
                "inline secret '{}' contains a forbidden control character",
                name
            );
        }
    }
    Ok(())
}

pub fn remove_generated(runtime: &AgentRuntime) {
    let _ = std::fs::remove_dir_all(&runtime.project_dir);
}

pub fn persist_slot(
    home: &Path,
    slot: &makakoo_core::agents::AgentSlot,
    runtime: &AgentRuntime,
) -> anyhow::Result<()> {
    if let Err(error) = makakoo_core::agents::AgentRegistry::create(home, slot) {
        remove_generated(runtime);
        return Err(anyhow::anyhow!(error));
    }
    Ok(())
}

pub fn print_next(slot: &str, runtime: &AgentRuntime) {
    match runtime.engine {
        AgentRuntimeEngine::DeepseekHarness => println!(
            "Next: cd {} && npm install; then `makakoo agent start {}` and `makakoo agent prompt {} \"hello\"`.",
            runtime.project_dir.display(),
            slot,
            slot
        ),
        AgentRuntimeEngine::Flue => println!(
            "Next: cd {} && npm install; fill .env; then `npm run proxy` + `npx flue dev`.",
            runtime.project_dir.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_parser_defaults_to_dsh_and_rejects_unknown_values() {
        assert_eq!(
            parse_engine(None).unwrap(),
            AgentRuntimeEngine::DeepseekHarness
        );
        assert_eq!(
            parse_engine(Some("flue")).unwrap(),
            AgentRuntimeEngine::Flue
        );
        assert!(parse_engine(Some("unknown")).is_err());
    }

    #[test]
    fn dsh_model_cannot_name_a_different_provider() {
        validate_model(
            AgentRuntimeEngine::DeepseekHarness,
            "switchailocal/ail-compound",
        )
        .unwrap();
        validate_model(AgentRuntimeEngine::DeepseekHarness, "ail-compound").unwrap();
        assert!(validate_model(
            AgentRuntimeEngine::DeepseekHarness,
            "anthropic/claude-sonnet"
        )
        .is_err());
        validate_model(AgentRuntimeEngine::Flue, "anthropic/claude-sonnet").unwrap();
    }

    #[test]
    fn inline_secret_rejects_dotenv_line_injection() {
        let secrets = HashMap::from([("TOKEN".into(), "good\nINJECTED=value".into())]);
        assert!(validate_inline_secrets(&secrets).is_err());
    }

    #[test]
    fn registry_failure_removes_generated_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("agents-dsh/bad-slot");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("runner.mjs"), "").unwrap();
        let runtime = AgentRuntime {
            engine: AgentRuntimeEngine::DeepseekHarness,
            project_dir: project.clone(),
        };
        let slot = makakoo_core::agents::AgentSlot {
            slot_id: "bad/slot".into(),
            name: "Bad".into(),
            persona: None,
            inherit_baseline: false,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools: vec![],
            process_mode: "supervised_pair".into(),
            transports: vec![],
            llm: None,
            runtime: Some(runtime.clone()),
        };
        assert!(persist_slot(tmp.path(), &slot, &runtime).is_err());
        assert!(!project.exists());
    }
}
