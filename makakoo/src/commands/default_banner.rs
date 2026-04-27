//! Bare `makakoo` (no subcommand) handler.
//!
//! Tytus v0.6 Phase A pattern: when the user types `makakoo` with no
//! arguments, print a friendly banner with the right next step instead
//! of clap's "subcommand required" error.
//!
//! Adapts based on installation state:
//! - First run (`~/.makakoo/` empty or missing) → "run `makakoo install`"
//! - Already set up (data dir exists) → "you're set up; here are the
//!   five most-used commands"
//!
//! All output goes to stdout (banner is documentation, not diagnostic).
//! `~/Library/Logs/makakoo/cli.log` (or `$XDG_STATE_HOME/makakoo/cli.log`
//! on Linux) is the destination for any WARN/ERROR noise per the same
//! Tytus Phase A WARN-suppression rule, but `default_banner` itself
//! emits nothing through `tracing` — it's a one-shot, pure-stdout call.
//!
//! Lives at `commands/default_banner.rs` because the rest of the
//! `commands` tree groups by intent, and "what `makakoo` does with
//! zero args" is a command-shaped concept.

use crate::context::CliContext;

/// Print the bare-invocation banner. Returns nothing — `main.rs` exits 0
/// after calling this.
pub fn run(ctx: &CliContext) {
    let already_set_up = is_already_set_up(ctx);
    print_banner();
    if already_set_up {
        print_returning_user();
    } else {
        print_first_run();
    }
    print_footer();
}

fn print_banner() {
    println!();
    println!("  ╭─────────────────────────────────────────────────╮");
    println!("  │   Makakoo OS — autonomous cognitive extension   │");
    println!("  │   Cross-CLI memory · plugins · the works        │");
    println!("  ╰─────────────────────────────────────────────────╯");
    println!();
}

fn print_first_run() {
    println!("  Welcome! Looks like this is your first run.");
    println!();
    println!("  Get set up in 5 minutes:");
    println!("    \x1b[1mmakakoo install\x1b[0m       Install the daemon + base distro");
    println!("    \x1b[1mmakakoo setup\x1b[0m         Interactive wizard (persona, brain, AI CLI, model)");
    println!();
    println!("  After setup, your AI CLIs (Claude Code, Gemini, OpenCode, …)");
    println!("  share memory through Makakoo. Run any tool — Makakoo remembers.");
    println!();
}

fn print_returning_user() {
    println!("  You're set up. Most-used commands:");
    println!();
    println!("    \x1b[1mmakakoo search <query>\x1b[0m   Full-text search across the Brain");
    println!("    \x1b[1mmakakoo query <q>\x1b[0m         Ask a question (RAG over Brain + plugins)");
    println!("    \x1b[1mmakakoo plugin list\x1b[0m       Show installed plugins");
    println!("    \x1b[1mmakakoo sancho status\x1b[0m     Background watchdog state");
    println!("    \x1b[1mmakakoo sync\x1b[0m              Re-index Brain + auto-memory");
    println!();
}

fn print_footer() {
    println!("  Full reference:    \x1b[1mmakakoo --help\x1b[0m");
    println!("  Documentation:     https://github.com/makakoo/makakoo-os");
    println!();
}

/// Best-effort first-run detector. Returns true when the install dir
/// looks populated. Cheap — checks for the superbrain DB existence at
/// the canonical path, no DB open. False positives are fine (banner
/// just shows the wrong next-step list); false negatives are fine too
/// (a user with a missing DB probably DOES want to see the install
/// hint).
fn is_already_set_up(ctx: &CliContext) -> bool {
    let db = ctx.data_dir().join("superbrain.db");
    db.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_unset_state_when_data_dir_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = CliContext::for_home(PathBuf::from(tmp.path()));
        assert!(!is_already_set_up(&ctx));
    }

    #[test]
    fn detects_set_state_when_db_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = CliContext::for_home(PathBuf::from(tmp.path()));
        std::fs::create_dir_all(ctx.data_dir()).expect("mkdir data");
        std::fs::write(ctx.data_dir().join("superbrain.db"), b"")
            .expect("touch db");
        assert!(is_already_set_up(&ctx));
    }
}
