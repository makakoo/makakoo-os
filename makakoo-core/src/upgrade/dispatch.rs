//! Per-method upgrade dispatchers.
//!
//! Maps an [`InstallMethod`] plus user overrides into a concrete
//! [`UpgradeAction`] (the planning step), then executes it. Planning
//! is pure — it produces the command line without spawning anything,
//! so `--dry-run` reuses the same code path.

use std::path::PathBuf;
use std::process::Command;

use thiserror::Error;

use super::detect::{CargoSource, InstallMethod};

/// Public entry point: which binary to upgrade. The dispatcher upgrades
/// `makakoo` and `makakoo-mcp` together by default — they're the two
/// halves of the same parasite OS and ship in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryTarget {
    /// Both `makakoo` and `makakoo-mcp` (default).
    Both,
    /// `makakoo` only — rare, mostly for dev iteration.
    KernelOnly,
    /// `makakoo-mcp` only — rare, mostly for dev iteration.
    McpOnly,
}

impl Default for BinaryTarget {
    fn default() -> Self {
        BinaryTarget::Both
    }
}

impl BinaryTarget {
    pub fn includes_kernel(self) -> bool {
        matches!(self, BinaryTarget::Both | BinaryTarget::KernelOnly)
    }
    pub fn includes_mcp(self) -> bool {
        matches!(self, BinaryTarget::Both | BinaryTarget::McpOnly)
    }
}

/// One concrete upgrade action — a single shell command to spawn,
/// plus a human-readable label printed during dry-run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeAction {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

impl UpgradeAction {
    /// Render as a copy-pasteable shell line.
    pub fn render(&self) -> String {
        let mut s = self.program.clone();
        for a in &self.args {
            s.push(' ');
            s.push_str(&shell_quote(a));
        }
        s
    }
}

fn shell_quote(s: &str) -> String {
    if s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.' | '=' | ':'))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[derive(Debug, Error)]
pub enum UpgradeError {
    #[error("install method is `Unknown` — running binary at {exe_path:?} was installed in a way Makakoo cannot auto-upgrade. Supported: cargo (~/.cargo/bin/), homebrew (/opt/homebrew/ or /usr/local/), curl-pipe ($HOME/.local/bin/). Reinstall via one of those, or run `cargo install --path` from a checkout.")]
    UnknownInstall { exe_path: PathBuf },

    #[error("non-HTTPS install script URL refused: {url}")]
    InsecureUrl { url: String },

    #[error("subprocess failed: {label} (exit code {code:?})")]
    SpawnFailed {
        label: String,
        code: Option<i32>,
    },

    #[error("upgrade error: {0}")]
    Other(String),
}

/// Plan upgrade actions for the given method + override knobs.
///
/// `cargo_source_override` is the resolved Cargo source — Sebastian
/// passes `--source <path>` or `MAKAKOO_SOURCE_PATH`, the CLI
/// resolves it before calling here.
///
/// `install_script_url` is the URL for curl-pipe upgrades. Defaults to
/// `https://makakoo.com/install.sh`. The CLI override goes here.
pub fn plan_upgrade(
    method: &InstallMethod,
    target: BinaryTarget,
    cargo_source_override: Option<CargoSource>,
    install_script_url: &str,
) -> Result<Vec<UpgradeAction>, UpgradeError> {
    match method {
        InstallMethod::Unknown { exe_path } => Err(UpgradeError::UnknownInstall {
            exe_path: exe_path.clone(),
        }),

        InstallMethod::Cargo { source } => {
            // Resolve the source: explicit override > env var > default Git URL.
            let resolved = cargo_source_override
                .clone()
                .or_else(|| {
                    std::env::var("MAKAKOO_SOURCE_PATH")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .map(|p| CargoSource::LocalPath(PathBuf::from(p)))
                })
                .unwrap_or_else(|| match source {
                    CargoSource::LocalPath(p) => CargoSource::LocalPath(p.clone()),
                    CargoSource::Git(url) => CargoSource::Git(url.clone()),
                    CargoSource::Unresolved => {
                        CargoSource::Git("https://github.com/makakoo/makakoo-os".to_string())
                    }
                });

            let mut actions = Vec::new();
            for (binary, want) in [
                ("makakoo", target.includes_kernel()),
                ("makakoo-mcp", target.includes_mcp()),
            ] {
                if !want {
                    continue;
                }
                let action = match &resolved {
                    CargoSource::LocalPath(root) => UpgradeAction {
                        label: format!("cargo install --path (local) for {binary}"),
                        program: "cargo".to_string(),
                        args: vec![
                            "install".into(),
                            "--path".into(),
                            root.join(binary).to_string_lossy().into_owned(),
                            "--locked".into(),
                            "--force".into(),
                        ],
                    },
                    CargoSource::Git(url) => UpgradeAction {
                        label: format!("cargo install --git for {binary}"),
                        program: "cargo".to_string(),
                        args: vec![
                            "install".into(),
                            "--git".into(),
                            url.clone(),
                            "--locked".into(),
                            "--force".into(),
                            binary.to_string(),
                        ],
                    },
                    CargoSource::Unresolved => unreachable!(
                        "resolved CargoSource::Unresolved escaped — bug in dispatch.rs"
                    ),
                };
                actions.push(action);
            }
            Ok(actions)
        }

        InstallMethod::Homebrew { prefix: _ } => Ok(vec![
            UpgradeAction {
                label: "brew update".to_string(),
                program: "brew".to_string(),
                args: vec!["update".into()],
            },
            UpgradeAction {
                label: "brew upgrade traylinx/tap/makakoo".to_string(),
                program: "brew".to_string(),
                args: vec!["upgrade".into(), "traylinx/tap/makakoo".into()],
            },
        ]),

        InstallMethod::CurlPipe { prefix: _ } => {
            if !install_script_url.starts_with("https://") {
                return Err(UpgradeError::InsecureUrl {
                    url: install_script_url.to_string(),
                });
            }
            Ok(vec![UpgradeAction {
                label: format!("curl-pipe install from {install_script_url}"),
                program: "sh".to_string(),
                args: vec![
                    "-c".into(),
                    format!("curl -fsSL {install_script_url} | sh"),
                ],
            }])
        }
    }
}

/// Spawn the planned actions in order, surfacing the first failure.
/// `dry_run = true` returns immediately after planning.
pub fn run_upgrade(
    method: &InstallMethod,
    target: BinaryTarget,
    cargo_source_override: Option<CargoSource>,
    install_script_url: &str,
    dry_run: bool,
    mut on_progress: impl FnMut(&UpgradeAction),
) -> Result<Vec<UpgradeAction>, UpgradeError> {
    let actions = plan_upgrade(method, target, cargo_source_override, install_script_url)?;
    if dry_run {
        for a in &actions {
            on_progress(a);
        }
        return Ok(actions);
    }

    for a in &actions {
        on_progress(a);
        let status = Command::new(&a.program)
            .args(&a.args)
            .status()
            .map_err(|e| UpgradeError::Other(format!("spawn failed for {}: {e}", a.label)))?;
        if !status.success() {
            return Err(UpgradeError::SpawnFailed {
                label: a.label.clone(),
                code: status.code(),
            });
        }
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn cargo() -> InstallMethod {
        InstallMethod::Cargo {
            source: CargoSource::Unresolved,
        }
    }

    fn brew() -> InstallMethod {
        InstallMethod::Homebrew {
            prefix: PathBuf::from("/opt/homebrew"),
        }
    }

    fn curl() -> InstallMethod {
        InstallMethod::CurlPipe {
            prefix: PathBuf::from("/Users/sebastian/.local"),
        }
    }

    fn unknown() -> InstallMethod {
        InstallMethod::Unknown {
            exe_path: PathBuf::from("/opt/weird/makakoo"),
        }
    }

    fn url() -> &'static str {
        "https://makakoo.com/install.sh"
    }

    #[test]
    fn cargo_default_uses_git() {
        std::env::remove_var("MAKAKOO_SOURCE_PATH");
        let actions = plan_upgrade(&cargo(), BinaryTarget::Both, None, url()).unwrap();
        assert_eq!(actions.len(), 2);
        for a in &actions {
            assert_eq!(a.program, "cargo");
            assert!(a.args.contains(&"--git".to_string()));
            assert!(a
                .args
                .contains(&"https://github.com/makakoo/makakoo-os".to_string()));
            assert!(a.args.contains(&"--locked".to_string()));
            assert!(a.args.contains(&"--force".to_string()));
        }
        assert!(actions[0].args.contains(&"makakoo".to_string()));
        assert!(actions[1].args.contains(&"makakoo-mcp".to_string()));
    }

    #[test]
    fn cargo_local_path_override_via_arg() {
        let override_src = Some(CargoSource::LocalPath(PathBuf::from("/repo")));
        let actions = plan_upgrade(&cargo(), BinaryTarget::Both, override_src, url()).unwrap();
        assert_eq!(actions.len(), 2);
        assert!(actions[0].args.contains(&"--path".to_string()));
        assert_eq!(actions[0].args[2], "/repo/makakoo");
        assert_eq!(actions[1].args[2], "/repo/makakoo-mcp");
    }

    #[test]
    fn cargo_local_path_override_via_env() {
        std::env::set_var("MAKAKOO_SOURCE_PATH", "/from/env");
        let actions = plan_upgrade(&cargo(), BinaryTarget::Both, None, url()).unwrap();
        std::env::remove_var("MAKAKOO_SOURCE_PATH");
        assert_eq!(actions[0].args[2], "/from/env/makakoo");
    }

    #[test]
    fn cargo_explicit_arg_overrides_env() {
        std::env::set_var("MAKAKOO_SOURCE_PATH", "/from/env");
        let override_src = Some(CargoSource::LocalPath(PathBuf::from("/from/arg")));
        let actions = plan_upgrade(&cargo(), BinaryTarget::Both, override_src, url()).unwrap();
        std::env::remove_var("MAKAKOO_SOURCE_PATH");
        assert_eq!(actions[0].args[2], "/from/arg/makakoo");
    }

    #[test]
    fn cargo_kernel_only_skips_mcp() {
        std::env::remove_var("MAKAKOO_SOURCE_PATH");
        let actions = plan_upgrade(&cargo(), BinaryTarget::KernelOnly, None, url()).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].args.contains(&"makakoo".to_string()));
        assert!(!actions[0].args.contains(&"makakoo-mcp".to_string()));
    }

    #[test]
    fn brew_emits_two_steps() {
        let actions = plan_upgrade(&brew(), BinaryTarget::Both, None, url()).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].program, "brew");
        assert_eq!(actions[0].args, vec!["update"]);
        assert_eq!(actions[1].args, vec!["upgrade", "traylinx/tap/makakoo"]);
    }

    #[test]
    fn curl_pipe_uses_sh_c() {
        let actions = plan_upgrade(&curl(), BinaryTarget::Both, None, url()).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].program, "sh");
        assert_eq!(actions[0].args[0], "-c");
        assert!(actions[0].args[1].contains(url()));
        assert!(actions[0].args[1].starts_with("curl -fsSL"));
    }

    #[test]
    fn curl_pipe_rejects_non_https() {
        let err = plan_upgrade(
            &curl(),
            BinaryTarget::Both,
            None,
            "http://insecure.example.com/install.sh",
        )
        .unwrap_err();
        assert!(matches!(err, UpgradeError::InsecureUrl { .. }));
    }

    #[test]
    fn unknown_returns_unknown_install_error() {
        let err = plan_upgrade(&unknown(), BinaryTarget::Both, None, url()).unwrap_err();
        match err {
            UpgradeError::UnknownInstall { exe_path } => {
                assert_eq!(exe_path, Path::new("/opt/weird/makakoo"));
            }
            other => panic!("expected UnknownInstall, got {other:?}"),
        }
    }

    #[test]
    fn upgrade_action_render_quotes_paths_with_spaces() {
        let action = UpgradeAction {
            label: "test".into(),
            program: "cargo".into(),
            args: vec!["install".into(), "--path".into(), "/path with spaces".into()],
        };
        let rendered = action.render();
        assert!(rendered.contains("'/path with spaces'"));
    }

    #[test]
    fn upgrade_action_render_does_not_quote_simple_args() {
        let action = UpgradeAction {
            label: "test".into(),
            program: "cargo".into(),
            args: vec!["install".into(), "--locked".into()],
        };
        assert_eq!(action.render(), "cargo install --locked");
    }

    #[test]
    fn dry_run_does_not_spawn() {
        std::env::remove_var("MAKAKOO_SOURCE_PATH");
        let mut count = 0;
        let actions = run_upgrade(
            &cargo(),
            BinaryTarget::Both,
            None,
            url(),
            true,
            |_| count += 1,
        )
        .unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(count, 2);
    }
}
