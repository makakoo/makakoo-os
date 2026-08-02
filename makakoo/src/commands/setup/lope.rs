//! The Lope section — optional install of the multi-CLI validator ensemble.
//!
//! Lope is not required for Makakoo to boot, but it is a high-leverage default:
//! one agent drafts, the rest validate. This section clones/updates `~/.lope`
//! and runs its own host-aware installer after explicit user consent.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::cli_agent::binary_on_path;
use super::harness::{Section, SectionOutcome, SectionStatus, Ui, YnSkip};

const LOPE_REPO_URL: &str = "https://github.com/traylinx/lope.git";
const LOPE_REPO_URL_NO_GIT: &str = "https://github.com/traylinx/lope";
const LOPE_REPO_SSH: &str = "git@github.com:traylinx/lope.git";

pub struct LopeSection {
    lope_home: PathBuf,
}

impl LopeSection {
    pub fn new() -> Self {
        Self {
            lope_home: lope_home(),
        }
    }

    #[cfg(test)]
    pub fn with_lope_home(lope_home: PathBuf) -> Self {
        Self { lope_home }
    }

    fn engine_path(&self) -> PathBuf {
        self.lope_home.join("lope").join("cli.py")
    }
}

impl Default for LopeSection {
    fn default() -> Self {
        Self::new()
    }
}

impl Section for LopeSection {
    fn name(&self) -> &'static str {
        "lope"
    }

    fn description(&self) -> &'static str {
        "Install Lope — your AI CLIs double-check each other's work"
    }

    fn status(&self) -> SectionStatus {
        if binary_on_path("lope") || self.engine_path().exists() {
            SectionStatus::AlreadySatisfied
        } else {
            SectionStatus::NotStarted
        }
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        if binary_on_path("lope") || self.engine_path().exists() {
            ui.line("lope: already installed. No action needed.")?;
            if let Some(version) = lope_version(&self.lope_home) {
                ui.line(format!("lope: {version}"))?;
            }
            return Ok(SectionOutcome::AlreadyPresent);
        }

        ui.line("lope: recommended.")?;
        ui.line("  Lope has your AI assistants review each other's work before it counts:")?;
        ui.line("  one drafts a plan or a change, the others independently check it and")?;
        ui.line("  vote. That catches mistakes a single model can't see in itself —")?;
        ui.line("  fewer wrong plans, fewer bad merges, better decisions.")?;

        let question = format!(
            "Install Lope now? This clones {LOPE_REPO_URL_NO_GIT} to {} and executes its installer.",
            self.lope_home.display()
        );
        let answer = ui.ask_ynskip(&question, YnSkip::No)?;
        match answer {
            YnSkip::No => Ok(SectionOutcome::Declined),
            YnSkip::Skip => Ok(SectionOutcome::Skipped),
            YnSkip::Yes => install_lope(ui, &self.lope_home),
        }
    }
}

fn lope_home() -> PathBuf {
    if let Ok(p) = std::env::var("LOPE_HOME") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lope")
}

fn install_lope(ui: &mut Ui, lope_home: &Path) -> anyhow::Result<SectionOutcome> {
    for bin in ["git", "python3", "bash"] {
        if !binary_on_path(bin) {
            ui.line(format!("lope: required binary `{bin}` not on PATH."))?;
            ui.line("  Install git + Python 3.9+ + bash, then re-run `makakoo setup lope`.")?;
            return Ok(SectionOutcome::Failed(format!("{bin} not found on PATH")));
        }
    }

    if lope_home.exists() && lope_home.join(".git").exists() {
        match git_origin(lope_home) {
            Some(origin) if is_expected_lope_origin(&origin) => {}
            Some(origin) => {
                return Ok(SectionOutcome::Failed(format!(
                    "{} is a git checkout, but origin is {origin:?}; expected {LOPE_REPO_URL_NO_GIT}. Refusing to execute its installer.",
                    lope_home.display()
                )));
            }
            None => {
                return Ok(SectionOutcome::Failed(format!(
                    "could not read git origin for {}; refusing to execute its installer",
                    lope_home.display()
                )));
            }
        }
        if !git_worktree_clean(lope_home) {
            return Ok(SectionOutcome::Failed(format!(
                "{} has local tracked modifications; refusing to execute Lope installer. Commit, reset, or move it aside first.",
                lope_home.display()
            )));
        }
        ui.line(format!(
            "lope: updating existing checkout at {} …",
            lope_home.display()
        ))?;
        if let Err(e) = run_inherited(
            Command::new("git")
                .arg("-C")
                .arg(lope_home)
                .arg("pull")
                .arg("--ff-only")
                .arg("origin")
                .arg("main"),
        ) {
            return Ok(SectionOutcome::Failed(format!(
                "could not fast-forward Lope checkout at {}: {e}",
                lope_home.display()
            )));
        }
        if !git_head_matches_origin_main(lope_home) {
            return Ok(SectionOutcome::Failed(format!(
                "{} is not exactly origin/main after update; refusing to execute Lope installer",
                lope_home.display()
            )));
        }
        if !git_worktree_clean(lope_home) {
            return Ok(SectionOutcome::Failed(format!(
                "{} has local tracked modifications after update; refusing to execute Lope installer",
                lope_home.display()
            )));
        }
    } else if lope_home.exists() {
        return Ok(SectionOutcome::Failed(format!(
            "{} exists but is not a git checkout; move it aside or set LOPE_HOME before running setup so Makakoo can clone {LOPE_REPO_URL_NO_GIT}",
            lope_home.display()
        )));
    } else {
        ui.line(format!("lope: cloning to {} …", lope_home.display()))?;
        if let Err(e) = run_inherited(
            Command::new("git")
                .arg("clone")
                .arg("--depth")
                .arg("1")
                .arg(LOPE_REPO_URL)
                .arg(lope_home),
        ) {
            return Ok(SectionOutcome::Failed(format!(
                "could not clone {LOPE_REPO_URL_NO_GIT} to {}: {e}",
                lope_home.display()
            )));
        }
    }

    let install_script = lope_home.join("install");
    if !install_script.exists() {
        return Ok(SectionOutcome::Failed(format!(
            "Lope install script missing at {}",
            install_script.display()
        )));
    }

    ui.line("lope: registering skills/commands into detected AI CLI hosts …")?;
    ui.stdout().flush()?;
    if let Err(e) = run_inherited(Command::new("bash").arg(&install_script)) {
        return Ok(SectionOutcome::Failed(format!(
            "Lope installer failed at {}: {e}",
            install_script.display()
        )));
    }

    if let Some(version) = lope_version(lope_home) {
        ui.line(format!("lope: installed. {version}"))?;
    } else {
        ui.line("lope: installed, but version smoke-test did not print a banner.")?;
    }
    ui.line("lope: restart your AI CLI sessions once so they pick up the new Lope commands.")?;
    ui.line("lope: if your shell lacks a `lope` command, add: alias lope='PYTHONPATH=~/.lope python3 -m lope'")?;
    Ok(SectionOutcome::Installed)
}

fn run_inherited(cmd: &mut Command) -> anyhow::Result<()> {
    let status = cmd
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        anyhow::bail!("command failed with exit code {:?}", status.code());
    }
    Ok(())
}

fn git_origin(lope_home: &Path) -> Option<String> {
    let out = git_output(lope_home, ["config", "--get", "remote.origin.url"])?;
    if !out.status.success() {
        return None;
    }
    let origin = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if origin.is_empty() {
        None
    } else {
        Some(origin)
    }
}

fn git_worktree_clean(lope_home: &Path) -> bool {
    let Some(out) = git_output(lope_home, ["status", "--porcelain"]) else {
        return false;
    };
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

fn git_head_matches_origin_main(lope_home: &Path) -> bool {
    let Some(head) = git_rev_parse(lope_home, "HEAD") else {
        return false;
    };
    let Some(origin_main) = git_rev_parse(lope_home, "refs/remotes/origin/main") else {
        return false;
    };
    head == origin_main
}

fn git_rev_parse(lope_home: &Path, rev: &str) -> Option<String> {
    let out = git_output(lope_home, ["rev-parse", rev])?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_output<const N: usize>(lope_home: &Path, args: [&str; N]) -> Option<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(lope_home)
        .args(args)
        .output()
        .ok()
}

fn is_expected_lope_origin(origin: &str) -> bool {
    matches!(
        origin.trim_end_matches('/'),
        LOPE_REPO_URL | LOPE_REPO_URL_NO_GIT | LOPE_REPO_SSH
    )
}

fn lope_version(lope_home: &Path) -> Option<String> {
    let out = Command::new("python3")
        .arg("-m")
        .arg("lope")
        .arg("version")
        .env("PYTHONPATH", lope_home)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s.lines().next().unwrap_or(&s).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn status_already_satisfied_when_engine_exists() {
        let _override = super::super::cli_agent::override_binary_on_path("lope", false);
        let tmp = TempDir::new().unwrap();
        let engine = tmp.path().join("lope");
        std::fs::create_dir_all(&engine).unwrap();
        std::fs::write(engine.join("cli.py"), "# test").unwrap();
        let section = LopeSection::with_lope_home(tmp.path().to_path_buf());
        assert_eq!(section.status(), SectionStatus::AlreadySatisfied);
    }

    #[test]
    fn status_not_started_when_missing() {
        let _override = super::super::cli_agent::override_binary_on_path("lope", false);
        let tmp = TempDir::new().unwrap();
        let section = LopeSection::with_lope_home(tmp.path().join("missing"));
        assert_eq!(section.status(), SectionStatus::NotStarted);
    }

    #[test]
    fn name_and_description_stable() {
        let section = LopeSection::with_lope_home(PathBuf::from("/tmp/lope-test"));
        assert_eq!(section.name(), "lope");
        assert!(!section.description().is_empty());
    }

    #[test]
    fn expected_origin_accepts_https_and_ssh_forms() {
        assert!(is_expected_lope_origin("https://github.com/traylinx/lope"));
        assert!(is_expected_lope_origin(
            "https://github.com/traylinx/lope.git"
        ));
        assert!(is_expected_lope_origin("git@github.com:traylinx/lope.git"));
        assert!(!is_expected_lope_origin(
            "https://github.com/example/lope.git"
        ));
    }

    #[test]
    fn install_refuses_existing_non_git_directory() {
        let _git = super::super::cli_agent::override_binary_on_path("git", true);
        let _python = super::super::cli_agent::override_binary_on_path("python3", true);
        let _bash = super::super::cli_agent::override_binary_on_path("bash", true);
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        let mut ui = Ui::new(std::io::Cursor::new(Vec::<u8>::new()), Vec::<u8>::new());
        let outcome = install_lope(&mut ui, tmp.path()).unwrap();
        match outcome {
            SectionOutcome::Failed(reason) => assert!(reason.contains("not a git checkout")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn run_defaults_to_decline_for_remote_installer() {
        let _override = super::super::cli_agent::override_binary_on_path("lope", false);
        let tmp = TempDir::new().unwrap();
        let mut section = LopeSection::with_lope_home(tmp.path().join("missing"));
        let stdin = std::io::Cursor::new(b"\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let outcome = section.run(&mut ui).unwrap();
        assert_eq!(outcome, SectionOutcome::Declined);
    }
}
