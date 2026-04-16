//! `makakoo install` — the one-shot setup umbrella.
//!
//! Spec: `spec/SPRINT-MAKAKOO-OS-MASTER.md §7 Phase F`. A fresh install
//! needs four separate things to happen:
//!
//! 1. **Distro install** — materialise the shipped plugin bundle
//!    (default `core`) into `$MAKAKOO_HOME/plugins/`.
//! 2. **Daemon install** — register the launchd / systemd / Task
//!    Scheduler agent that keeps SANCHO ticking.
//! 3. **Infect** — write the Bootstrap Block into every detected AI
//!    CLI host's global instructions slot.
//! 4. **Health check** — print a summary: what was installed, which
//!    hosts we infected, daemon status.
//!
//! This module is the orchestrator. Each step is a thin call into the
//! existing subcommand module — `distro::install`, `daemon::dispatch`,
//! `infect::run` — so the install umbrella stays small and every
//! step is still individually usable.
//!
//! Flags:
//! - `--dry-run`: print the plan + host detection result, exit without
//!   touching anything. Always safe to run.
//! - `--yes`: forwarded to `distro install --yes`.
//! - `--skip-daemon` / `--skip-infect`: leave those pieces for the
//!   user to do manually.

use crossterm::style::Stylize;

use crate::cli::{Commands, DistroCmd};
use crate::context::CliContext;
use crate::detect::{detect_all, DetectedHost};
use crate::output;

pub async fn run(
    ctx: &CliContext,
    distro: String,
    dry_run: bool,
    yes: bool,
    skip_daemon: bool,
    skip_infect: bool,
) -> anyhow::Result<i32> {
    // Step 0: detection. Always runs, even in --dry-run — it's the
    // whole point of the plan output.
    let home = dirs::home_dir().unwrap_or_else(|| ctx.home().to_path_buf());
    let detected = detect_all(&home);
    let present: Vec<&DetectedHost> =
        detected.iter().filter(|h| h.is_detected()).collect();

    print_plan(&distro, &detected, skip_daemon, skip_infect, dry_run);

    if dry_run {
        println!();
        output::print_info("--dry-run: no changes made.");
        return Ok(0);
    }

    // Step 1: distro install.
    println!();
    println!("{}", format!("[1/3] installing distro {distro}…").green().bold());
    let distro_rc = super::distro::run(
        ctx,
        DistroCmd::Install {
            name: Some(distro.clone()),
            from: None,
            yes,
            dry_run: false,
        },
    )
    .await?;
    if distro_rc != 0 {
        output::print_error("distro install failed — aborting install umbrella");
        return Ok(distro_rc);
    }

    // Step 2: daemon install. Reuse the existing dispatcher.
    if skip_daemon {
        output::print_warn("[2/3] --skip-daemon — skipping daemon install");
    } else {
        println!();
        println!("{}", "[2/3] installing daemon…".green().bold());
        match crate::daemon::dispatch(crate::daemon::DaemonCmd::Install).await {
            Ok(()) => {}
            Err(e) => {
                output::print_warn(format!("daemon install failed: {e:#}"));
                // Don't abort — distro is already in place and the
                // user can re-run `makakoo daemon install` later.
            }
        }
    }

    // Step 3: infect global CLI slots.
    if skip_infect {
        output::print_warn("[3/3] --skip-infect — skipping bootstrap block infect");
    } else {
        println!();
        println!(
            "{}",
            format!(
                "[3/3] infecting {} detected CLI host(s)…",
                present.len()
            )
            .green()
            .bold()
        );
        let report = crate::infect::run(true, false).await?;
        print!("{}", report.human_summary());
    }

    println!();
    print_summary(&detected);
    Ok(0)
}

fn print_plan(
    distro: &str,
    detected: &[DetectedHost],
    skip_daemon: bool,
    skip_infect: bool,
    dry_run: bool,
) {
    let header = if dry_run {
        "install plan (dry-run)".yellow().bold()
    } else {
        "install plan".cyan().bold()
    };
    println!("{header}");
    println!("  distro:   {distro}");
    println!(
        "  daemon:   {}",
        if skip_daemon {
            "skip".dark_grey().to_string()
        } else {
            "install".to_string()
        }
    );
    println!(
        "  infect:   {}",
        if skip_infect {
            "skip".dark_grey().to_string()
        } else {
            "global CLI slots".to_string()
        }
    );

    println!("\n  detected hosts:");
    let mut any = false;
    for h in detected {
        if !h.is_detected() {
            continue;
        }
        any = true;
        let bin = match &h.binary_on_path {
            Some(p) => format!("binary={}", p.display()),
            None => "binary=(not on PATH)".into(),
        };
        let cfg = if h.instructions_exists {
            if h.bootstrap_present {
                format!(
                    "config={} (bootstrap present, will refresh)",
                    h.instructions_path.display()
                )
            } else {
                format!(
                    "config={} (exists, fresh infect)",
                    h.instructions_path.display()
                )
            }
        } else {
            format!("config={} (will create)", h.instructions_path.display())
        };
        println!("    - {}:", h.name);
        println!("        {bin}");
        println!("        {cfg}");
    }
    if !any {
        println!(
            "    {}",
            "(no hosts detected — only distro + daemon will be installed)".dark_grey()
        );
    }
}

fn print_summary(detected: &[DetectedHost]) {
    println!("{}", "install complete".green().bold());
    let infected: Vec<&DetectedHost> =
        detected.iter().filter(|h| h.is_detected()).collect();
    println!("  detected hosts: {}", infected.len());
    if !infected.is_empty() {
        let names: Vec<&str> = infected.iter().map(|h| h.name).collect();
        println!("    {}", names.join(", "));
    }
    println!(
        "\n  next steps:\n    - {}\n    - {}\n    - {}",
        "makakoo sancho status     # see the registered tick tasks",
        "makakoo plugin list       # see what's installed",
        "makakoo secret set AIL_API_KEY  # wire up the LLM gateway"
    );
}

pub async fn dispatch(ctx: &CliContext, cmd: Commands) -> anyhow::Result<i32> {
    match cmd {
        Commands::Install {
            distro,
            dry_run,
            yes,
            skip_daemon,
            skip_infect,
        } => run(ctx, distro, dry_run, yes, skip_daemon, skip_infect).await,
        _ => unreachable!("dispatch called with non-Install variant"),
    }
}
