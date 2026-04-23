//! The brain-sources section — shells to the existing Python picker at
//! `$MAKAKOO_HOME/plugins/skill-brain-multi-source/src/brain_cli.py init`.
//!
//! We don't reimplement the picker here. It already has the
//! batched+confirm+post-sync flow shipped by SPRINT-BRAIN-MEMORY-UNIFIED
//! (2026-04-23). This section is a wrapper that:
//!
//! 1. Detects whether the user already has non-default sources registered
//!    (`status()` reads `brain_sources.json`).
//! 2. Spawns the picker with stdio inherited so the user drives it
//!    directly.
//! 3. Reports the outcome to the dispatcher based on the subprocess's
//!    exit code.

use std::path::{Path, PathBuf};
use std::process::Command;

use makakoo_core::platform::makakoo_home;
use serde::Deserialize;

use super::harness::{Section, SectionOutcome, SectionStatus, Ui};

pub struct BrainSection {
    home: PathBuf,
}

impl BrainSection {
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
        self.home.join("config").join("brain_sources.json")
    }

    fn picker_path(&self) -> PathBuf {
        self.home
            .join("plugins")
            .join("skill-brain-multi-source")
            .join("src")
            .join("brain_cli.py")
    }
}

impl Default for BrainSection {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct BrainSourcesFile {
    #[serde(default)]
    sources: Vec<BrainSourceEntry>,
}

#[derive(Debug, Deserialize)]
struct BrainSourceEntry {
    #[serde(default)]
    name: String,
}

impl Section for BrainSection {
    fn name(&self) -> &'static str {
        "brain"
    }

    fn description(&self) -> &'static str {
        "Connect Logseq / Obsidian / plain markdown vaults"
    }

    fn status(&self) -> SectionStatus {
        read_brain_status(&self.config_path())
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        let picker = self.picker_path();
        if !picker.exists() {
            ui.line(format!(
                "brain: picker not found at {} — skipping section.",
                picker.display()
            ))?;
            ui.line("(is `skill-brain-multi-source` installed into $MAKAKOO_HOME/plugins/?)")?;
            return Ok(SectionOutcome::Failed(format!(
                "picker missing: {}",
                picker.display()
            )));
        }

        ui.line("brain: launching the picker (you drive it from here) …")?;
        ui.stdout().flush()?;

        let status = Command::new("python3")
            .arg(&picker)
            .arg("init")
            .env("MAKAKOO_HOME", &self.home)
            .status()?;

        if status.success() {
            // Re-read config to tell "user added new sources" from "user skipped".
            match read_brain_status(&self.config_path()) {
                SectionStatus::AlreadySatisfied => Ok(SectionOutcome::Installed),
                _ => Ok(SectionOutcome::Declined),
            }
        } else {
            let code = status.code().unwrap_or(-1);
            Ok(SectionOutcome::Failed(format!(
                "brain picker exited with code {code}"
            )))
        }
    }
}

/// Read the user's `brain_sources.json` and decide whether there's
/// setup work still to do. Factored out so tests can target it
/// directly with synthetic config paths.
fn read_brain_status(config_path: &Path) -> SectionStatus {
    if !config_path.exists() {
        return SectionStatus::NotStarted;
    }
    let raw = match std::fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(_) => return SectionStatus::NotStarted,
    };
    let parsed: BrainSourcesFile = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(_) => return SectionStatus::NotStarted,
    };
    let non_default: Vec<&str> = parsed
        .sources
        .iter()
        .map(|e| e.name.as_str())
        .filter(|n| !n.is_empty() && *n != "default")
        .collect();
    if non_default.is_empty() {
        SectionStatus::NotStarted
    } else {
        SectionStatus::AlreadySatisfied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn home_with_config(contents: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        let cfg_dir = tmp.path().join("config");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(cfg_dir.join("brain_sources.json"), contents).unwrap();
        tmp
    }

    #[test]
    fn status_is_notstarted_when_config_missing() {
        let tmp = TempDir::new().unwrap();
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::NotStarted);
    }

    #[test]
    fn status_is_notstarted_when_only_baseline_default() {
        let tmp = home_with_config(
            r#"{"default":"default","sources":[{"name":"default","type":"logseq","path":"X"}]}"#,
        );
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::NotStarted);
    }

    #[test]
    fn status_is_alreadysatisfied_when_multi_source() {
        let tmp = home_with_config(
            r#"{"default":"default","sources":[
                {"name":"default","type":"logseq","path":"X"},
                {"name":"obsidian","type":"obsidian","path":"Y"}
            ]}"#,
        );
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::AlreadySatisfied);
    }

    #[test]
    fn status_is_notstarted_on_malformed_json() {
        let tmp = home_with_config("{not json");
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::NotStarted);
    }

    #[test]
    fn status_is_notstarted_on_empty_sources_array() {
        let tmp = home_with_config(r#"{"default":"default","sources":[]}"#);
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::NotStarted);
    }

    #[test]
    fn picker_path_resolves_under_home() {
        let tmp = TempDir::new().unwrap();
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        let p = section.picker_path();
        assert!(p.to_string_lossy().contains("plugins"));
        assert!(p.to_string_lossy().contains("skill-brain-multi-source"));
        assert!(p.to_string_lossy().ends_with("brain_cli.py"));
    }

    #[test]
    fn name_and_description_stable() {
        let tmp = TempDir::new().unwrap();
        let section = BrainSection::with_home(tmp.path().to_path_buf());
        assert_eq!(section.name(), "brain");
        assert!(!section.description().is_empty());
    }

    #[test]
    fn run_reports_failed_when_picker_missing() {
        let tmp = TempDir::new().unwrap();
        let mut section = BrainSection::with_home(tmp.path().to_path_buf());
        let stdin = std::io::Cursor::new(Vec::<u8>::new());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = section.run(&mut ui).unwrap();
        match outcome {
            SectionOutcome::Failed(msg) => {
                assert!(msg.contains("picker missing"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
