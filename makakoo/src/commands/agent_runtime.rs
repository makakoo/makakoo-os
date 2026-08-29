//! Resolve a slot's compiled runtime into a supervisor child process.

use std::path::{Path, PathBuf};

use makakoo_core::agents::llm_override::EffectiveLlm;
use makakoo_core::agents::supervisor::GatewayLaunchSpec;
use makakoo_core::agents::{AgentRuntimeEngine, AgentSlot};

pub fn preflight(slot: &AgentSlot) -> anyhow::Result<()> {
    let Some(runtime) = slot.runtime.as_ref() else {
        return Ok(());
    };
    match runtime.engine {
        AgentRuntimeEngine::DeepseekHarness => {
            let defaults = makakoo_core::agents::llm_override::LlmDefaults::builtin_fallback();
            let over = slot
                .llm
                .as_ref()
                .and_then(|section| section.effective_override());
            let effective =
                makakoo_core::agents::llm_override::resolve_effective(over.as_ref(), &defaults);
            crate::commands::agent_engine::validate_model(runtime.engine, &effective.model.0)?;
            preflight_dsh(&runtime.project_dir)
        }
        AgentRuntimeEngine::Flue => anyhow::bail!(
            "slot '{}' uses the legacy Flue engine; run its proxy and dev scripts from {}",
            slot.slot_id,
            runtime.project_dir.display()
        ),
    }
}

pub fn launch_spec(
    home: &Path,
    slot: &AgentSlot,
    effective_llm: &EffectiveLlm,
) -> anyhow::Result<GatewayLaunchSpec> {
    let Some(runtime) = slot.runtime.as_ref() else {
        return Ok(GatewayLaunchSpec::harveychat_default(
            home,
            &slot.slot_id,
            Some(effective_llm),
        ));
    };
    preflight(slot)?;
    match runtime.engine {
        AgentRuntimeEngine::DeepseekHarness => Ok(dsh_launch_spec(
            home,
            slot,
            &runtime.project_dir,
            effective_llm,
        )?),
        AgentRuntimeEngine::Flue => unreachable!("preflight rejects Flue supervisor launch"),
    }
}

fn preflight_dsh(project_dir: &Path) -> anyhow::Result<()> {
    let runner = project_dir.join("runner.mjs");
    if !runner.is_file() {
        anyhow::bail!(
            "DeepSeek Harness runner missing at {}; recreate the slot or restore the generated project",
            runner.display()
        );
    }
    let sdk = project_dir.join("node_modules/@deepseek-ai/dsh-sdk-client");
    if !sdk.is_dir() {
        anyhow::bail!(
            "DeepSeek Harness dependencies missing at {}; run `cd {} && npm install`",
            sdk.display(),
            project_dir.display()
        );
    }
    preflight_node_version()?;
    Ok(())
}

/// Directory containing the `node` that satisfies the version gate.
///
/// launchd and systemd-user hand the supervisor a minimal PATH that
/// excludes nvm and Homebrew, so the start path must record where the
/// verified interpreter actually lives. Resolved by walking the
/// caller's PATH — the same lookup `Command::new("node")` performs.
pub fn resolve_node_bin_dir() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find(|dir| {
        let candidate = dir.join("node");
        std::fs::metadata(&candidate).is_ok_and(|meta| meta.is_file())
    })
}

fn preflight_node_version() -> anyhow::Result<()> {
    let output = std::process::Command::new("node")
        .arg("--version")
        .output()
        .map_err(|error| {
            anyhow::anyhow!("Node.js 22.9+ is required for DeepSeek Harness: {error}")
        })?;
    if !output.status.success() {
        anyhow::bail!("Node.js 22.9+ is required for DeepSeek Harness; `node --version` failed");
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let version = raw.trim().trim_start_matches('v');
    if !node_version_supported(version) {
        anyhow::bail!(
            "Node.js 22.9+ is required for DeepSeek Harness; found {}",
            raw.trim()
        );
    }
    Ok(())
}

fn node_version_supported(version: &str) -> bool {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u64>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u64>().ok());
    matches!((major, minor), (Some(major), Some(minor)) if major > 22 || (major == 22 && minor >= 9))
}

fn dsh_launch_spec(
    home: &Path,
    slot: &AgentSlot,
    project_dir: &Path,
    effective_llm: &EffectiveLlm,
) -> anyhow::Result<GatewayLaunchSpec> {
    crate::commands::agent_engine::validate_model(
        AgentRuntimeEngine::DeepseekHarness,
        &effective_llm.model.0,
    )?;
    // The runner enforces DSH_MAX_TOKENS as an integer from 1 through 65536;
    // reject anything else here so a bad override fails fast instead of
    // crash-looping the supervised runtime.
    let max_tokens = effective_llm.max_tokens.0;
    if !(1..=65536).contains(&max_tokens) {
        anyhow::bail!(
            "max_tokens {max_tokens} is out of range for DeepSeek Harness; expected 1 through 65536"
        );
    }
    let model = effective_llm
        .model
        .0
        .split_once('/')
        .map(|(_, model)| model)
        .unwrap_or(&effective_llm.model.0);
    Ok(GatewayLaunchSpec::new("node")
        .arg("--env-file-if-exists=.env")
        .arg("runner.mjs")
        .env("MAKAKOO_HOME", home.to_string_lossy())
        .env("MAKAKOO_AGENT_SLOT", &slot.slot_id)
        .env("MAKAKOO_MCP_BIN", resolve_mcp_binary())
        .env("DSH_MODEL", model)
        .env("DSH_MAX_TOKENS", max_tokens.to_string())
        .cwd(project_dir.to_path_buf()))
}

fn resolve_mcp_binary() -> String {
    if let Ok(path) = std::env::var("MAKAKOO_MCP_BIN") {
        if !path.trim().is_empty() {
            return path;
        }
    }
    sibling_binary("makakoo-mcp")
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "makakoo-mcp".into())
}

fn sibling_binary(name: &str) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|dir| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::agents::llm_override::{LlmSource, ReasoningEffort};
    use makakoo_core::agents::AgentRuntime;

    fn slot(project_dir: PathBuf) -> AgentSlot {
        AgentSlot {
            slot_id: "researcher".into(),
            name: "Researcher".into(),
            persona: None,
            inherit_baseline: false,
            allowed_paths: vec![],
            forbidden_paths: vec![],
            tools: vec![],
            process_mode: "supervised_pair".into(),
            transports: vec![],
            llm: None,
            runtime: Some(AgentRuntime {
                engine: AgentRuntimeEngine::DeepseekHarness,
                project_dir,
            }),
            triggers: Vec::new(),
        }
    }

    fn effective() -> EffectiveLlm {
        EffectiveLlm {
            model: ("switchailocal/ail-compound".into(), LlmSource::Override),
            max_tokens: (4096, LlmSource::Override),
            temperature: (0.7, LlmSource::SystemDefault),
            reasoning_effort: (ReasoningEffort::Medium, LlmSource::SystemDefault),
            top_p: (1.0, LlmSource::SystemDefault),
        }
    }

    #[test]
    fn dsh_launch_uses_generated_runner_and_scoped_mcp() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("runner.mjs"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/@deepseek-ai/dsh-sdk-client"))
            .unwrap();
        let spec = launch_spec(
            Path::new("/tmp/makakoo"),
            &slot(tmp.path().into()),
            &effective(),
        )
        .unwrap();
        assert_eq!(spec.program, "node");
        assert_eq!(spec.args, ["--env-file-if-exists=.env", "runner.mjs"]);
        assert!(spec
            .envs
            .contains(&("MAKAKOO_AGENT_SLOT".into(), "researcher".into())));
        assert!(spec
            .envs
            .contains(&("DSH_MODEL".into(), "ail-compound".into())));
    }

    #[test]
    fn preflight_requires_npm_install() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("runner.mjs"), "").unwrap();
        let error = preflight(&slot(tmp.path().into())).unwrap_err();
        assert!(error.to_string().contains("npm install"));
    }

    #[test]
    fn node_version_gate_matches_runner_flag_requirement() {
        assert!(!node_version_supported("21.99.0"));
        assert!(!node_version_supported("22.8.0"));
        assert!(node_version_supported("22.9.0"));
        assert!(node_version_supported("23.0.0"));
        assert!(!node_version_supported("bad"));
    }

    #[test]
    fn preflight_rejects_edited_non_switchailocal_model_before_launch() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("runner.mjs"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/@deepseek-ai/dsh-sdk-client"))
            .unwrap();
        let mut candidate = slot(tmp.path().into());
        candidate.llm = Some(makakoo_core::agents::slot::LlmSection {
            inherit: None,
            overrides: Some(makakoo_core::agents::llm_override::LlmOverride {
                model: Some("anthropic/claude-sonnet".into()),
                max_tokens: None,
                temperature: None,
                reasoning_effort: None,
            }),
        });
        let error = preflight(&candidate).unwrap_err();
        assert!(error.to_string().contains("routes through switchAILocal"));
    }

    #[test]
    fn legacy_slot_keeps_harveychat_gateway() {
        let mut legacy = slot(PathBuf::from("/unused"));
        legacy.runtime = None;
        let spec = launch_spec(Path::new("/tmp/makakoo"), &legacy, &effective()).unwrap();
        assert_eq!(spec.program, "python3");
        assert_eq!(spec.args, ["gateway.py", "--slot", "researcher"]);
    }

    #[test]
    fn launch_rejects_non_switchailocal_provider_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("runner.mjs"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/@deepseek-ai/dsh-sdk-client"))
            .unwrap();
        let mut llm = effective();
        llm.model.0 = "anthropic/claude-sonnet".into();
        let error =
            launch_spec(Path::new("/tmp/makakoo"), &slot(tmp.path().into()), &llm).unwrap_err();
        assert!(error.to_string().contains("routes through switchAILocal"));
    }

    #[test]
    fn launch_rejects_out_of_range_max_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("runner.mjs"), "").unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules/@deepseek-ai/dsh-sdk-client"))
            .unwrap();
        for value in [0, 65537] {
            let mut llm = effective();
            llm.max_tokens.0 = value;
            let error =
                launch_spec(Path::new("/tmp/makakoo"), &slot(tmp.path().into()), &llm).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("max_tokens {value} is out of range")),
                "unexpected error for {value}: {error}"
            );
        }
    }

    #[test]
    fn sibling_binary_appends_platform_exe_suffix() {
        let path = sibling_binary("makakoo-mcp").unwrap();
        assert!(
            path.ends_with(format!("makakoo-mcp{}", std::env::consts::EXE_SUFFIX)),
            "unexpected sibling candidate: {}",
            path.display()
        );
    }
}
