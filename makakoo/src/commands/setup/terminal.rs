//! The terminal section — macOS-only bootstrap for Ghostty via Homebrew.
//!
//! Platform-gated: `is_applicable()` returns false on Linux/Windows, so
//! the section is filtered out of the dispatcher before it ever
//! prompts. Same pattern as `cli_agent.rs`, just with `brew` instead of
//! `npm`.

use std::process::{Command, Stdio};

use super::cli_agent::binary_on_path;
use super::harness::{Section, SectionOutcome, SectionStatus, Ui, YnSkip};

/// Homebrew cask name. Pinned constant; matches the name in
/// `sancho-task-cli-ghostty`'s tick.py.
pub const GHOSTTY_CASK: &str = "ghostty";

pub struct TerminalSection;

impl TerminalSection {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TerminalSection {
    fn default() -> Self {
        Self::new()
    }
}

impl Section for TerminalSection {
    fn name(&self) -> &'static str {
        "terminal"
    }

    fn description(&self) -> &'static str {
        "Install Ghostty (blessed terminal, macOS only)"
    }

    fn is_applicable(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn status(&self) -> SectionStatus {
        if !self.is_applicable() {
            // Non-macOS: section is pre-filtered, but if a caller still
            // probes status() we're honest about it.
            return SectionStatus::AlreadySatisfied;
        }
        if ghostty_installed() {
            SectionStatus::AlreadySatisfied
        } else {
            SectionStatus::NotStarted
        }
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        if !self.is_applicable() {
            ui.line("terminal: Ghostty is macOS-only; skipping on this platform.")?;
            return Ok(SectionOutcome::AlreadyPresent);
        }

        if ghostty_installed() {
            ui.line("terminal: Ghostty already installed via Homebrew. No action needed.")?;
            return Ok(SectionOutcome::AlreadyPresent);
        }

        ui.line("terminal: Ghostty is the blessed terminal — fast, native macOS, low")?;
        ui.line("  latency. Installing via Homebrew cask wires it up so the 24h sancho")?;
        ui.line("  updater keeps it current.")?;
        let answer = ui.ask_ynskip(
            &format!("Install Ghostty now? Runs: brew install --cask {GHOSTTY_CASK}"),
            YnSkip::Yes,
        )?;

        match answer {
            YnSkip::No => Ok(SectionOutcome::Declined),
            YnSkip::Skip => Ok(SectionOutcome::Skipped),
            YnSkip::Yes => install_ghostty(ui),
        }
    }
}

fn install_ghostty(ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
    if !binary_on_path("brew") {
        ui.line("terminal: Homebrew (`brew`) not on PATH.")?;
        ui.line("  Install Homebrew from https://brew.sh, then re-run")?;
        ui.line(&format!(
            "  `makakoo setup terminal` — or run `brew install --cask {GHOSTTY_CASK}` by hand."
        ))?;
        return Ok(SectionOutcome::Failed(
            "brew not found on PATH — install Homebrew first".to_string(),
        ));
    }

    ui.line(&format!(
        "terminal: running brew install --cask {GHOSTTY_CASK} …"
    ))?;
    ui.stdout().flush()?;

    let mut cmd = Command::new("brew");
    cmd.arg("install").arg("--cask").arg(GHOSTTY_CASK);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cmd.status()?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Ok(SectionOutcome::Failed(format!(
            "brew install exited with code {code}"
        )));
    }

    if !ghostty_installed() {
        return Ok(SectionOutcome::Failed(
            "brew install reported success but ghostty is still not listed as an installed cask — check `brew doctor`.".to_string(),
        ));
    }
    ui.line("terminal: installed. Ghostty is registered as a Homebrew cask.")?;
    Ok(SectionOutcome::Installed)
}

/// True when `brew list --cask ghostty` exits 0. Needs brew on PATH;
/// returns false if brew is missing (which is also the accurate answer
/// because brew is required to install the cask in the first place).
fn ghostty_installed() -> bool {
    if !binary_on_path("brew") {
        return false;
    }
    let Ok(out) = Command::new("brew")
        .arg("list")
        .arg("--cask")
        .arg(GHOSTTY_CASK)
        .output()
    else {
        return false;
    };
    out.status.success()
}

// Tests rely on `test_support::shim` which is unix-only (writes a
// `#!/bin/sh` script and chmods it executable via PermissionsExt).
// On Windows the test module is excluded entirely.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use super::super::test_support::{shim, shim_args, PathGuard};
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn is_applicable_respects_platform() {
        let s = TerminalSection::new();
        assert_eq!(s.is_applicable(), cfg!(target_os = "macos"));
    }

    #[test]
    fn name_and_description_stable() {
        let s = TerminalSection::new();
        assert_eq!(s.name(), "terminal");
        assert!(!s.description().is_empty());
    }

    #[test]
    fn cask_constant_stable() {
        assert_eq!(GHOSTTY_CASK, "ghostty");
    }

    #[test]
    fn install_ghostty_happy_path() {
        let dir = TempDir::new().unwrap();
        // brew has two invocations: one from install (install --cask),
        // one from the post-install ghostty_installed() check (list --cask).
        // A single shim that always exits 0 covers both.
        shim(dir.path(), "brew", 0, "");
        let orig = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), orig);
        let _g = PathGuard::new(&new_path);

        let stdin = Cursor::new(Vec::<u8>::new());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = install_ghostty(&mut ui).unwrap();

        assert_eq!(outcome, SectionOutcome::Installed);
        let args = shim_args(dir.path(), "brew");
        // First invocation's args land at the top of the log. Verify
        // install --cask ghostty appears.
        let joined = args.join(" ");
        assert!(
            joined.contains("install") && joined.contains("--cask") && joined.contains(GHOSTTY_CASK),
            "expected install+--cask+ghostty in args: {joined}"
        );
    }

    #[test]
    fn install_ghostty_reports_failed_when_brew_exits_nonzero() {
        let dir = TempDir::new().unwrap();
        shim(dir.path(), "brew", 1, "");
        let orig = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), orig);
        let _g = PathGuard::new(&new_path);

        let stdin = Cursor::new(Vec::<u8>::new());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = install_ghostty(&mut ui).unwrap();

        match outcome {
            SectionOutcome::Failed(msg) => assert!(msg.contains("brew install exited")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn install_ghostty_reports_failed_when_brew_missing() {
        let dir = TempDir::new().unwrap();
        // stage a `which` shim that always reports not-found, so
        // binary_on_path("brew") returns false. Prepend to original
        // PATH (don't replace) so concurrent test readers still see sh
        // etc. — this PathGuard holds the global mutex.
        shim(dir.path(), "which", 1, "");
        let orig = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), orig);
        let _g = PathGuard::new(&new_path);

        let stdin = Cursor::new(Vec::<u8>::new());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = install_ghostty(&mut ui).unwrap();

        match outcome {
            SectionOutcome::Failed(msg) => {
                assert!(msg.contains("brew not found"), "got: {msg}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
