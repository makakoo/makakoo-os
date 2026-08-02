//! The infect section — thin wizard wrapper over the existing
//! `makakoo infect` command. Doesn't re-implement any of the
//! bootstrap-block writing logic; it just prompts the user, then
//! shells to `makakoo infect --verify` / `makakoo infect` in the
//! current process's own binary path.

use std::process::{Command, Stdio};

use super::harness::{Section, SectionOutcome, SectionStatus, Ui, YnSkip};

pub struct InfectSection;

impl InfectSection {
    pub fn new() -> Self {
        Self
    }

    /// Path to `makakoo` — the currently running binary. Shelling
    /// back into ourselves keeps the section immune to PATH drift and
    /// future refactors of the `infect` subcommand.
    fn makakoo_bin() -> std::path::PathBuf {
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("makakoo"))
    }
}

impl Default for InfectSection {
    fn default() -> Self {
        Self::new()
    }
}

impl Section for InfectSection {
    fn name(&self) -> &'static str {
        "infect"
    }

    fn description(&self) -> &'static str {
        "Connect Makakoo to your installed AI CLIs"
    }

    fn status(&self) -> SectionStatus {
        // `makakoo infect --verify` exits 0 when every slot is up-to-date,
        // 1 when there's drift. A spawn error (e.g., current_exe() failed)
        // is honest NotStarted.
        let Ok(out) = Command::new(Self::makakoo_bin())
            .arg("infect")
            .arg("--verify")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return SectionStatus::NotStarted;
        };
        if out.status.success() {
            SectionStatus::AlreadySatisfied
        } else {
            SectionStatus::NotStarted
        }
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        let bin = Self::makakoo_bin();

        // 1) Show the user what verify reports right now.
        ui.line("infect: checking which of your AI CLIs are already connected to Makakoo …")?;
        ui.stdout().flush()?;
        let verify = Command::new(&bin).arg("infect").arg("--verify").status()?;
        if verify.success() {
            ui.line("infect: all your AI CLIs are already connected. Nothing to do.")?;
            return Ok(SectionOutcome::AlreadyPresent);
        }

        // 2) Ask whether to fix the drift.
        let answer = ui.ask_ynskip(
            "Connect the remaining AI CLIs now? This adds a small Makakoo section to each CLI's config file (nothing else is changed).",
            YnSkip::Yes,
        )?;
        match answer {
            YnSkip::No => Ok(SectionOutcome::Declined),
            YnSkip::Skip => Ok(SectionOutcome::Skipped),
            YnSkip::Yes => {
                ui.line("infect: connecting your AI CLIs …")?;
                ui.stdout().flush()?;
                let run = Command::new(&bin).arg("infect").status()?;
                if !run.success() {
                    let code = run.code().unwrap_or(-1);
                    return Ok(SectionOutcome::Failed(format!(
                        "makakoo infect exited with code {code}"
                    )));
                }
                // Re-verify to confirm the fix actually landed.
                let reverify = Command::new(&bin)
                    .arg("infect")
                    .arg("--verify")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()?;
                if reverify.success() {
                    ui.line("infect: done — all AI CLIs are connected.")?;
                    Ok(SectionOutcome::Installed)
                } else {
                    Ok(SectionOutcome::Failed(
                        "makakoo infect ran but verify still reports drift — run `makakoo infect --verify` manually to inspect."
                            .to_string(),
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_description_stable() {
        let s = InfectSection::new();
        assert_eq!(s.name(), "infect");
        assert!(!s.description().is_empty());
    }

    #[test]
    fn status_returns_a_variant_without_panicking() {
        // status() shells out to the current binary. In `cargo test` the
        // current_exe is the test harness, which does NOT understand the
        // `infect` subcommand, so the probe returns non-success and we
        // should see NotStarted (NOT a panic).
        let s = InfectSection::new();
        let status = s.status();
        assert!(matches!(
            status,
            SectionStatus::NotStarted | SectionStatus::AlreadySatisfied
        ));
    }
}
