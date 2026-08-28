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

/// Escape hatch for CI and scripted provisioning that must not touch the
/// network. Equivalent to passing `--no-install`.
pub const SKIP_DEPS_INSTALL_ENV: &str = "MAKAKOO_SKIP_DEPS_INSTALL";

/// What `install_deps` actually did, so the closing "Next:" line tells the
/// truth instead of always demanding a manual `npm install`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepsInstall {
    /// `--no-install` or MAKAKOO_SKIP_DEPS_INSTALL. The user asked to run
    /// the install themselves, so we still print the command.
    OptedOut,
    /// The generated project has no `package.json`. Printing an npm command
    /// for a project npm cannot install would be worse than saying nothing:
    /// there is nothing here to install.
    MissingManifest,
    Installed,
    /// npm was missing or exited non-zero. Create still succeeded — the
    /// generated project and registry entry are intact and the user only
    /// needs to run the install by hand.
    Failed,
}

/// `npm` is a shell shim on Windows; `Command::new("npm")` cannot execute a
/// `.cmd` without the extension.
const NPM_BIN: &str = if cfg!(windows) { "npm.cmd" } else { "npm" };

/// `--no-fund --no-audit` keep the output to what a first-time user needs;
/// the audit round-trip is the slowest part of a cold install and its
/// advisories are not actionable inside a generated project.
const NPM_ARGS: [&str; 3] = ["install", "--no-fund", "--no-audit"];

fn deps_manifest(project_dir: &Path) -> PathBuf {
    project_dir.join("package.json")
}

/// Whether a post-scaffold install should run at all. The environment is
/// read by the caller and passed in, so this stays a pure function that is
/// unit-testable without npm on PATH and without racing other tests over a
/// process-global env var.
fn install_decision(project_dir: &Path, opted_out: bool, env_opt_out: bool) -> Option<DepsInstall> {
    if opted_out || env_opt_out {
        return Some(DepsInstall::OptedOut);
    }
    if !deps_manifest(project_dir).is_file() {
        return Some(DepsInstall::MissingManifest);
    }
    None
}

/// POSIX single-quoting: everything is literal inside `'…'`, and an embedded
/// quote is closed, escaped, and reopened. Without this a project path
/// containing a space silently produces a broken command, and one containing
/// `;` or `$(…)` produces a command that does something other than install.
fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', r"'\''"))
}

/// The exact command line to print when the install has to be run by hand,
/// so the fallback is copy-pasteable rather than a description of one.
pub fn manual_install_hint(project_dir: &Path) -> String {
    format!(
        "cd {} && npm install",
        shell_quote(&project_dir.display().to_string())
    )
}

fn npm_command(project_dir: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(NPM_BIN);
    cmd.args(NPM_ARGS).current_dir(project_dir);
    // The node that passed the version gate may live in nvm or Homebrew.
    // Prepending its directory keeps npm and node from resolving to two
    // different installs when several are present.
    if let Some(node_dir) = crate::commands::agent_runtime::resolve_node_bin_dir() {
        if let Some(path) = std::env::var_os("PATH") {
            let mut dirs = vec![node_dir];
            dirs.extend(std::env::split_paths(&path));
            if let Ok(joined) = std::env::join_paths(dirs) {
                cmd.env("PATH", joined);
            }
        }
    }
    cmd
}

/// Install the generated project's Node dependencies in place.
///
/// A create that leaves the user one undocumented `npm install` away from a
/// working agent is a create that half-worked; `agent start` then fails with
/// "dependencies missing" on what looked like a successful setup.
///
/// Never fails the create: the registry entry and the scaffolded project are
/// already durable at this point, and rolling them back over a transient
/// registry outage would destroy more than it fixes.
pub fn install_deps(runtime: &AgentRuntime, opted_out: bool) -> DepsInstall {
    let project_dir = runtime.project_dir.as_path();
    let env_opt_out = std::env::var_os(SKIP_DEPS_INSTALL_ENV).is_some();
    if let Some(skipped) = install_decision(project_dir, opted_out, env_opt_out) {
        return skipped;
    }
    crate::output::print_info(format!(
        "installing runtime dependencies in {} (npm install) …",
        project_dir.display()
    ));
    // Inherited stdio: a cold install is slow enough that silence reads as a
    // hang, and npm's own errors are better than anything we could restate.
    match npm_command(project_dir).status() {
        Ok(status) if status.success() => DepsInstall::Installed,
        Ok(status) => {
            crate::output::print_warn(format!(
                "npm install exited with {} — the slot and project are intact; finish with: {}",
                status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "a signal".to_string()),
                manual_install_hint(project_dir)
            ));
            DepsInstall::Failed
        }
        Err(error) => {
            crate::output::print_warn(format!(
                "could not run npm ({error}) — the slot and project are intact; finish with: {}",
                manual_install_hint(project_dir)
            ));
            DepsInstall::Failed
        }
    }
}

fn next_steps_line(slot: &str, runtime: &AgentRuntime, deps: DepsInstall) -> String {
    // Only mention npm when the user still has to run it. Telling someone to
    // install dependencies we just installed is how a correct setup gets
    // mistaken for a broken one.
    let install_step = match deps {
        // Nothing to run, or nothing npm could run: either way, do not ask.
        DepsInstall::Installed | DepsInstall::MissingManifest => String::new(),
        DepsInstall::OptedOut | DepsInstall::Failed => {
            format!("{}; ", manual_install_hint(&runtime.project_dir))
        }
    };
    match runtime.engine {
        AgentRuntimeEngine::DeepseekHarness => format!(
            "Next: {}`makakoo agent start {}` and `makakoo agent prompt {} \"hello\"`.",
            install_step, slot, slot
        ),
        AgentRuntimeEngine::Flue => format!(
            "Next: {}fill {}/.env; then `npm run proxy` + `npx flue dev`.",
            install_step,
            runtime.project_dir.display()
        ),
    }
}

pub fn print_next(slot: &str, runtime: &AgentRuntime, deps: DepsInstall) {
    println!("{}", next_steps_line(slot, runtime, deps));
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

    fn runtime_at(dir: &Path) -> AgentRuntime {
        AgentRuntime {
            engine: AgentRuntimeEngine::DeepseekHarness,
            project_dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn a_manifestless_project_is_distinguished_from_an_opt_out() {
        let tmp = tempfile::tempdir().unwrap();
        // A scaffold that produced no package.json has nothing to install;
        // running npm there would fail noisily for no reason, and telling the
        // user to run it by hand would be advice that cannot work.
        assert_eq!(
            install_decision(tmp.path(), false, false),
            Some(DepsInstall::MissingManifest)
        );
        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
        assert_eq!(install_decision(tmp.path(), false, false), None);
    }

    #[test]
    fn install_opt_outs_win_over_a_present_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
        assert_eq!(
            install_decision(tmp.path(), true, false),
            Some(DepsInstall::OptedOut),
            "--no-install must reproduce the pre-0.4.0 manual flow"
        );
        assert_eq!(
            install_decision(tmp.path(), false, true),
            Some(DepsInstall::OptedOut),
            "{SKIP_DEPS_INSTALL_ENV} must work for offline/CI provisioning"
        );
    }

    #[test]
    fn install_never_runs_when_opted_out_even_if_npm_is_absent() {
        // The opt-out path must not spawn anything: this asserts the outcome
        // without a package.json *and* with one, so a regression that moved
        // the guard below the spawn would show up as a slow, network-touching
        // test rather than silently passing.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
        assert_eq!(
            install_deps(&runtime_at(tmp.path()), true),
            DepsInstall::OptedOut
        );
    }

    #[test]
    fn npm_command_targets_the_project_dir_with_quiet_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = npm_command(tmp.path());
        assert_eq!(cmd.get_program(), NPM_BIN);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args, vec!["install", "--no-fund", "--no-audit"]);
        assert_eq!(cmd.get_current_dir(), Some(tmp.path()));
    }

    #[test]
    fn npm_command_puts_the_gated_node_first_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = npm_command(tmp.path());
        let Some(node_dir) = crate::commands::agent_runtime::resolve_node_bin_dir() else {
            return; // no node on this machine; nothing to assert
        };
        let path = cmd
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("PATH"))
            .and_then(|(_, value)| value)
            .expect("PATH override must be set when a node dir resolves");
        let first = std::env::split_paths(path).next().unwrap();
        assert_eq!(
            first, node_dir,
            "npm must resolve next to the node that passed the version gate"
        );
    }

    #[test]
    fn next_steps_drops_the_install_step_once_deps_are_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = runtime_at(tmp.path());
        let installed = next_steps_line("scout", &runtime, DepsInstall::Installed);
        assert!(
            !installed.contains("npm install"),
            "must not ask for an install that already ran: {installed}"
        );
        assert!(installed.contains("makakoo agent start scout"));

        // OptedOut and Failed both leave the user one manual command away,
        // so both must still print it.
        for deps in [DepsInstall::OptedOut, DepsInstall::Failed] {
            let line = next_steps_line("scout", &runtime, deps);
            assert!(
                line.contains(&manual_install_hint(tmp.path())),
                "{deps:?} must print the copy-pasteable install: {line}"
            );
        }

        // A project with no manifest cannot be npm-installed at all; telling
        // the user to try is advice that is guaranteed to fail.
        let missing = next_steps_line("scout", &runtime, DepsInstall::MissingManifest);
        assert!(!missing.contains("npm install"), "{missing}");
    }

    #[test]
    fn manual_install_hint_quotes_the_path() {
        assert_eq!(
            manual_install_hint(Path::new("/tmp/agents-dsh/scout")),
            "cd '/tmp/agents-dsh/scout' && npm install"
        );
        // A path with a space must not split into two arguments, and a path
        // with shell metacharacters must not execute anything when pasted.
        assert_eq!(
            manual_install_hint(Path::new("/tmp/my agents/scout")),
            "cd '/tmp/my agents/scout' && npm install"
        );
        let nasty = manual_install_hint(Path::new("/tmp/a;rm -rf ~/b"));
        assert_eq!(nasty, "cd '/tmp/a;rm -rf ~/b' && npm install");
        assert_eq!(
            shell_quote("it's"),
            r"'it'\''s'",
            "an embedded quote must close, escape and reopen"
        );
    }
}
