//! `makakoo update` / legacy `makakoo upgrade` — self-update the kernel binaries.
//!
//! SPRINT-MAKAKOO-UPGRADE-VERB. Detects install method, dispatches the
//! matching update command, prints the version delta, and — when the version
//! actually changed and a daemon service exists — restarts the daemon via the
//! freshly installed binary so no manual step is left behind. Opt out with
//! `--no-daemon-restart` / `MAKAKOO_UPDATE_NO_DAEMON_RESTART=1`, in which
//! case the manual hint is printed instead.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context};

use makakoo_core::upgrade::{
    capture_version, daemon_restart_hint, detect_install_method, plan_upgrade, run_upgrade,
    BinaryTarget, CargoSource, InstallMethod,
};

use crate::context::CliContext;

const DEFAULT_INSTALL_SCRIPT_URL: &str = "https://makakoo.com/install.sh";

#[allow(clippy::too_many_arguments)]
pub async fn run(
    reinfect: bool,
    dry_run: bool,
    method: Option<String>,
    source: Option<String>,
    install_script_url: Option<String>,
    only_kernel: bool,
    only_mcp: bool,
    no_daemon_restart: bool,
    _ctx: &CliContext,
) -> anyhow::Result<i32> {
    // Detect install method (or honor override).
    let detected = detect_install_method();
    let resolved_method = match method.as_deref() {
        None => detected,
        Some("cargo") => InstallMethod::Cargo {
            source: CargoSource::Unresolved,
        },
        Some("brew") | Some("homebrew") => match &detected {
            InstallMethod::Homebrew { prefix } => InstallMethod::Homebrew {
                prefix: prefix.clone(),
            },
            _ => InstallMethod::Homebrew {
                prefix: default_homebrew_prefix(),
            },
        },
        Some("curl-pipe") | Some("install.sh") => {
            let prefix = std::env::var("MAKAKOO_PREFIX")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local"));
            InstallMethod::CurlPipe { prefix }
        }
        Some(other) => {
            return Err(anyhow!(
                "unknown --method {other:?} — valid: cargo, brew, curl-pipe"
            ));
        }
    };

    // Resolve cargo source override from CLI flag.
    let cargo_source_override = source
        .clone()
        .map(|p| CargoSource::LocalPath(PathBuf::from(p)));

    let target = match (only_kernel, only_mcp) {
        (true, true) => {
            return Err(anyhow!(
                "--only-kernel and --only-mcp are mutually exclusive"
            ))
        }
        (true, false) => BinaryTarget::KernelOnly,
        (false, true) => BinaryTarget::McpOnly,
        (false, false) => BinaryTarget::Both,
    };

    let url = install_script_url
        .as_deref()
        .unwrap_or(DEFAULT_INSTALL_SCRIPT_URL);

    // Print install method banner.
    println!("# install method: {}", describe_method(&resolved_method));

    // Capture pre-update version (best-effort).
    let pre_version = capture_version("makakoo");

    // Plan + (optionally) execute.
    let actions = if dry_run {
        let actions = plan_upgrade(&resolved_method, target, cargo_source_override, url)
            .with_context(|| "planning update")?;
        println!("# DRY RUN — would execute:");
        for a in &actions {
            println!("  $ {}", a.render());
        }
        actions
    } else {
        run_upgrade(
            &resolved_method,
            target,
            cargo_source_override,
            url,
            false,
            |a| println!("$ {}", a.render()),
        )
        .with_context(|| "running update")?
    };

    // Verify version delta (skip on dry-run). `None` = could not verify, in
    // which case we still treat the binary as possibly-new below.
    let mut version_changed = None;
    if !dry_run {
        let post_version = capture_version("makakoo");
        match (&pre_version, &post_version) {
            (Some(pre), Some(post)) if pre == post => {
                version_changed = Some(false);
                println!();
                println!("# version unchanged: {pre}");
                println!("# already up to date — package manager reported no newer build");
            }
            (Some(pre), Some(post)) => {
                version_changed = Some(true);
                println!();
                println!("# version delta:");
                println!("  before: {pre}");
                println!("  after:  {post}");
            }
            _ => {
                eprintln!("\n⚠ could not capture version banner — update succeeded but verification skipped");
            }
        }
    }

    // A daemon running the old image after an update is a support ticket
    // waiting to happen, so restart is automatic. Constraints: only when a
    // daemon service exists (restart on a daemon-less install would *install*
    // one), only when the binary may actually be new, and always via the
    // PATH-resolved `makakoo` so the restart logic comes from the new binary,
    // not this (old) process.
    if !dry_run && version_changed != Some(false) {
        let platform = makakoo_platform::CurrentPlatform::default();
        let daemon_present = {
            use makakoo_platform::PlatformAdapter;
            platform.daemon_is_installed() || platform.daemon_is_running()
        };
        let opted_out = no_daemon_restart
            || std::env::var("MAKAKOO_UPDATE_NO_DAEMON_RESTART")
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false);
        if daemon_present && !opted_out {
            println!();
            println!("# daemon: restarting so it picks up the new binary...");
            let restarted = Command::new("makakoo")
                .args(["daemon", "restart"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !restarted {
                eprintln!("⚠ automatic daemon restart failed — run it manually:");
                eprintln!("    {}", daemon_restart_hint());
            }
        } else if daemon_present {
            println!();
            println!("# daemon: restart skipped (--no-daemon-restart); pick up the new binary with:");
            println!("    {}", daemon_restart_hint());
        }
    }

    // Optional re-infect. A fragment refresh must write the global slots, not
    // only audit drift. v0.1.20 exposed this with tool-headroom: `infect
    // --verify --repair` can return clean while the canonical bootstrap cache
    // or host slots still need a real re-render.
    if reinfect && !dry_run {
        println!();
        println!("# re-infecting CLI hosts to refresh bootstrap fragments...");
        let status = Command::new("makakoo")
            .args(["infect", "--global"])
            .status()
            .with_context(|| "spawning makakoo infect --global")?;
        if !status.success() {
            eprintln!("⚠ re-infect step failed (exit {:?})", status.code());
        } else {
            let verify = Command::new("makakoo")
                .args(["infect", "--verify"])
                .status()
                .with_context(|| "spawning makakoo infect --verify")?;
            if !verify.success() {
                eprintln!("⚠ post-reinfect verify failed (exit {:?})", verify.code());
            }
        }
    }

    if !dry_run {
        maybe_offer_setup_review()?;
    }

    if dry_run {
        println!();
        println!(
            "# dry-run complete — {} action(s) planned, 0 executed",
            actions.len()
        );
    }

    Ok(0)
}

/// Updates should not force users through the full first-run wizard, but an
/// interactive shell can offer a quick review for newly-added setup defaults.
/// Enter keeps the safe update path quiet; explicit yes re-enters `makakoo setup`.
fn maybe_offer_setup_review() -> anyhow::Result<()> {
    if !crate::commands::setup::is_interactive_stdin() {
        return Ok(());
    }
    println!();
    println!("# your existing settings are kept either way — answering `y` only");
    println!("# opens a review of setup sections added or changed in this version");
    print!("Review new setup sections now? [y/N]: ");
    use std::io::Write as _;
    std::io::stdout().flush()?;
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_lowercase();
    if read > 0 && (trimmed == "y" || trimmed == "yes") {
        println!();
        let rc = crate::commands::setup::run(crate::commands::setup::SetupArgs::default())?;
        if rc != 0 {
            eprintln!("⚠ setup wizard returned non-zero exit code {rc}");
        }
    } else {
        println!();
        println!("# skipped — settings unchanged. Review anytime with: makakoo setup");
    }
    Ok(())
}

fn describe_method(m: &InstallMethod) -> String {
    match m {
        InstallMethod::Cargo { .. } => "Cargo (~/.cargo/bin/)".to_string(),
        InstallMethod::Homebrew { prefix } => format!("Homebrew ({})", prefix.display()),
        InstallMethod::CurlPipe { prefix } => format!("curl-pipe ({})", prefix.display()),
        InstallMethod::Unknown { exe_path } => format!("Unknown ({})", exe_path.display()),
    }
}

fn default_homebrew_prefix() -> PathBuf {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        PathBuf::from("/opt/homebrew")
    } else if cfg!(target_os = "linux") {
        PathBuf::from("/home/linuxbrew/.linuxbrew")
    } else if cfg!(target_os = "windows") {
        // Homebrew is not a supported Windows install channel, but `--method
        // brew --dry-run` and tests should still use a platform-absolute path
        // instead of a Unix-rooted string that Windows treats as relative.
        PathBuf::from(r"C:\ProgramData\Homebrew")
    } else {
        PathBuf::from("/usr/local")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_homebrew_prefix_matches_platform_family() {
        let prefix = default_homebrew_prefix();
        assert!(prefix.is_absolute());
    }

    #[test]
    fn describe_homebrew_method_uses_supplied_prefix() {
        let method = InstallMethod::Homebrew {
            prefix: PathBuf::from("/usr/local"),
        };
        assert_eq!(describe_method(&method), "Homebrew (/usr/local)");
    }
}
