//! The updates section — choose Makakoo OS auto-update mode.
//!
//! Fresh installs default to automatic updates through the setup wizard. The
//! SANCHO task reads `$MAKAKOO_HOME/config/updates.toml`; if the file is
//! missing it stays idle so existing installs do not silently opt into
//! unattended self-updates just because they upgraded.

use std::fs;
use std::path::{Path, PathBuf};

use makakoo_core::platform::makakoo_home;

use super::harness::{Section, SectionOutcome, SectionStatus, Ui, YnSkip};

pub const UPDATE_MODE_FILE: &str = "updates.toml";

pub struct UpdatesSection {
    home: PathBuf,
}

impl UpdatesSection {
    pub fn new() -> Self {
        Self {
            home: makakoo_home(),
        }
    }

    #[cfg(test)]
    pub fn with_home(home: PathBuf) -> Self {
        Self { home }
    }

    fn config_path(&self) -> PathBuf {
        self.home.join("config").join(UPDATE_MODE_FILE)
    }
}

impl Default for UpdatesSection {
    fn default() -> Self {
        Self::new()
    }
}

impl Section for UpdatesSection {
    fn name(&self) -> &'static str {
        "updates"
    }

    fn description(&self) -> &'static str {
        "Choose Makakoo OS update mode (auto/manual)"
    }

    fn status(&self) -> SectionStatus {
        match read_update_mode(&self.config_path()).as_deref() {
            Some("auto" | "manual") => SectionStatus::AlreadySatisfied,
            _ => SectionStatus::NotStarted,
        }
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        if let Some(mode @ ("auto" | "manual")) = read_update_mode(&self.config_path()).as_deref() {
            ui.line(format!(
                "updates: already configured as `{mode}` at {}.",
                self.config_path().display()
            ))?;
            ui.line("(delete the file or run `makakoo setup --reset` to re-ask)")?;
            return Ok(SectionOutcome::AlreadyPresent);
        }

        ui.line("updates: Makakoo can keep itself current like normal software.")?;
        ui.line("  Auto mode runs `makakoo update --reinfect` from SANCHO every 24h.")?;
        ui.line("  Manual mode only journals a reminder; you run `makakoo update` yourself.")?;
        ui.line("  Default for fresh setup: auto. Without this config file, scheduled updates stay idle.")?;

        let answer = ui.ask_ynskip(
            "Enable automatic Makakoo OS updates? You can switch to manual later by editing config/updates.toml.",
            YnSkip::Yes,
        )?;
        match answer {
            YnSkip::Skip => Ok(SectionOutcome::Skipped),
            YnSkip::Yes => {
                write_update_mode(&self.config_path(), "auto")?;
                ui.line("updates: mode → auto.")?;
                Ok(SectionOutcome::Installed)
            }
            YnSkip::No => {
                write_update_mode(&self.config_path(), "manual")?;
                ui.line("updates: mode → manual. Run `makakoo update --reinfect` when you want upgrades.")?;
                Ok(SectionOutcome::Installed)
            }
        }
    }
}

fn read_update_mode(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "mode" {
                return Some(value.trim().trim_matches('"').to_lowercase());
            }
        }
    }
    None
}

fn write_update_mode(path: &Path, mode: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("# Makakoo OS update mode. Valid: auto | manual\nmode = \"{mode}\"\n"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn status_not_started_when_missing() {
        let tmp = TempDir::new().unwrap();
        let section = UpdatesSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::NotStarted);
    }

    #[test]
    fn status_already_satisfied_for_valid_mode() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config").join(UPDATE_MODE_FILE);
        write_update_mode(&path, "auto").unwrap();
        let section = UpdatesSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::AlreadySatisfied);
    }

    #[test]
    fn run_writes_auto_on_default_yes() {
        let tmp = TempDir::new().unwrap();
        let mut section = UpdatesSection::with_home(tmp.path().to_path_buf());
        let stdin = Cursor::new(b"\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = section.run(&mut ui).unwrap();
        assert_eq!(outcome, SectionOutcome::Installed);
        assert_eq!(
            read_update_mode(&tmp.path().join("config").join(UPDATE_MODE_FILE)).as_deref(),
            Some("auto")
        );
    }

    #[test]
    fn run_writes_manual_on_no() {
        let tmp = TempDir::new().unwrap();
        let mut section = UpdatesSection::with_home(tmp.path().to_path_buf());
        let stdin = Cursor::new(b"n\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = section.run(&mut ui).unwrap();
        assert_eq!(outcome, SectionOutcome::Installed);
        assert_eq!(
            read_update_mode(&tmp.path().join("config").join(UPDATE_MODE_FILE)).as_deref(),
            Some("manual")
        );
    }
}
