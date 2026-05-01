//! `makakoo upgrade` — self-update the kernel binaries.
//!
//! SPRINT-MAKAKOO-UPGRADE-VERB. Detects install method, dispatches the
//! matching update command, prints version delta, surfaces a manual
//! daemon-restart command (v1 doesn't auto-restart — see Phase 0).

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
    _ctx: &CliContext,
) -> anyhow::Result<i32> {
    // Detect install method (or honor override).
    let detected = detect_install_method();
    let resolved_method = match method.as_deref() {
        None => detected,
        Some("cargo") => InstallMethod::Cargo {
            source: CargoSource::Unresolved,
        },
        Some("brew") | Some("homebrew") => InstallMethod::Homebrew {
            prefix: PathBuf::from("/opt/homebrew"),
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
    let cargo_source_override = source.clone().map(|p| CargoSource::LocalPath(PathBuf::from(p)));

    let target = match (only_kernel, only_mcp) {
        (true, true) => return Err(anyhow!("--only-kernel and --only-mcp are mutually exclusive")),
        (true, false) => BinaryTarget::KernelOnly,
        (false, true) => BinaryTarget::McpOnly,
        (false, false) => BinaryTarget::Both,
    };

    let url = install_script_url
        .as_deref()
        .unwrap_or(DEFAULT_INSTALL_SCRIPT_URL);

    // Print install method banner.
    println!("# install method: {}", describe_method(&resolved_method));

    // Capture pre-upgrade version (best-effort).
    let pre_version = capture_version("makakoo");

    // Plan + (optionally) execute.
    let actions = if dry_run {
        let actions = plan_upgrade(&resolved_method, target, cargo_source_override, url)
            .with_context(|| "planning upgrade")?;
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
        .with_context(|| "running upgrade")?
    };

    // Verify version delta (skip on dry-run).
    if !dry_run {
        let post_version = capture_version("makakoo");
        match (&pre_version, &post_version) {
            (Some(pre), Some(post)) if pre == post => {
                eprintln!("\n⚠ version unchanged after upgrade: {pre}");
                eprintln!("  (the upgrade command may have been a no-op — check the upstream source)");
                return Ok(1);
            }
            (Some(pre), Some(post)) => {
                println!();
                println!("# version delta:");
                println!("  before: {pre}");
                println!("  after:  {post}");
            }
            _ => {
                eprintln!("\n⚠ could not capture version banner — upgrade succeeded but verification skipped");
            }
        }
    }

    // Daemon-restart hint (always print after non-dry-run success).
    if !dry_run {
        println!();
        println!("# daemon: pick up the new binary in any running daemon with:");
        println!("    {}", daemon_restart_hint());
    }

    // Optional re-infect.
    if reinfect && !dry_run {
        println!();
        println!("# re-infecting CLI hosts to refresh bootstrap fragments...");
        let status = Command::new("makakoo")
            .args(["infect", "--verify", "--repair"])
            .status()
            .with_context(|| "spawning makakoo infect --verify --repair")?;
        if !status.success() {
            eprintln!("⚠ re-infect step failed (exit {:?})", status.code());
        }
    }

    if dry_run {
        println!();
        println!("# dry-run complete — {} action(s) planned, 0 executed", actions.len());
    }

    Ok(0)
}

fn describe_method(m: &InstallMethod) -> String {
    match m {
        InstallMethod::Cargo { .. } => "Cargo (~/.cargo/bin/)".to_string(),
        InstallMethod::Homebrew { prefix } => format!("Homebrew ({})", prefix.display()),
        InstallMethod::CurlPipe { prefix } => format!("curl-pipe ({})", prefix.display()),
        InstallMethod::Unknown { exe_path } => format!("Unknown ({})", exe_path.display()),
    }
}
