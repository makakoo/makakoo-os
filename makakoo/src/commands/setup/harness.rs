//! Shared types and helpers for the `makakoo setup` section dispatcher.
//!
//! Every wizard section implements [`Section`]. The dispatcher in [`super`]
//! calls `status()` to print the current state, then `run()` to execute
//! whatever interactive flow the section owns. Status and outcome types
//! are deliberately separate:
//!
//! - [`SectionStatus`] is read from `completed.json` or recomputed at
//!   runtime; it describes *where the section stands right now*.
//! - [`SectionOutcome`] is what `run()` returns; it describes *what just
//!   happened*. The dispatcher maps one to the other.

use std::io::{BufRead, IsTerminal, Write};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Persisted per-section state. Serialized into `completed.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum SectionStatus {
    /// No setup attempt yet.
    NotStarted,
    /// The section detected the desired state already exists (e.g. pi
    /// already on PATH). We don't persist this — it's recomputed each run.
    AlreadySatisfied,
    /// Section ran successfully and made changes.
    Completed {
        at: DateTime<Utc>,
    },
    /// User explicitly chose "skip" — don't re-ask unless `--reset`.
    Skipped {
        at: DateTime<Utc>,
    },
    /// Section attempted and errored. Re-asks next run.
    Failed {
        reason: String,
        at: DateTime<Utc>,
    },
}

impl SectionStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SectionStatus::Completed { .. } | SectionStatus::Skipped { .. }
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            SectionStatus::NotStarted => "not-started",
            SectionStatus::AlreadySatisfied => "already-satisfied",
            SectionStatus::Completed { .. } => "completed",
            SectionStatus::Skipped { .. } => "skipped",
            SectionStatus::Failed { .. } => "failed",
        }
    }
}

/// What a section's `run()` just produced. The dispatcher decides whether
/// to persist each outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum SectionOutcome {
    /// Section made a change (installed, wrote config, etc.).
    Installed,
    /// Nothing to do — desired state already satisfied.
    AlreadyPresent,
    /// User said "n" — don't persist. Re-ask next run.
    Declined,
    /// User said "s" — persist `Skipped` so we don't re-ask.
    Skipped,
    /// Dependency missing or operation failed. Persist with the reason.
    Failed(String),
}

/// A discrete step in the setup wizard. Each section is independently
/// runnable (`makakoo setup <section>`) and idempotent (`run()` can be
/// called repeatedly).
pub trait Section {
    /// Machine-readable section name (appears in `completed.json` and on
    /// the CLI as `makakoo setup <name>`).
    fn name(&self) -> &'static str;

    /// One-line description shown in `--help` and the summary table.
    fn description(&self) -> &'static str;

    /// Platform gate. Default applies on every platform.
    fn is_applicable(&self) -> bool {
        true
    }

    /// Probe current state. Called before every `run()` to decide whether
    /// the section has work to do, and used as a quick health check when
    /// `--non-interactive` is set.
    fn status(&self) -> SectionStatus;

    /// Execute the section's interactive flow. Only called on a live TTY
    /// (the dispatcher enforces the gate).
    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome>;
}

/// User-choice from a `[Y/n/s]` prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YnSkip {
    /// User said yes (or accepted the yes default).
    Yes,
    /// User said no — re-ask next run.
    No,
    /// User said skip — record as persisted `Skipped`.
    Skip,
}

/// Interactive I/O seam. Sections never touch stdin/stdout directly —
/// they go through `Ui` so tests can drive them with in-memory buffers.
pub struct Ui<'a> {
    stdin: Box<dyn BufRead + 'a>,
    stdout: Box<dyn Write + 'a>,
}

impl<'a> Ui<'a> {
    pub fn new(stdin: impl BufRead + 'a, stdout: impl Write + 'a) -> Self {
        Self {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
        }
    }

    pub fn stdin(&mut self) -> &mut dyn BufRead {
        &mut *self.stdin
    }

    pub fn stdout(&mut self) -> &mut dyn Write {
        &mut *self.stdout
    }

    /// Print a line to the wizard's output stream.
    pub fn line(&mut self, s: impl AsRef<str>) -> anyhow::Result<()> {
        writeln!(self.stdout, "{}", s.as_ref())?;
        Ok(())
    }

    /// Print without trailing newline, flushed.
    pub fn prompt_write(&mut self, s: impl AsRef<str>) -> anyhow::Result<()> {
        write!(self.stdout, "{}", s.as_ref())?;
        self.stdout.flush()?;
        Ok(())
    }

    /// Read one trimmed line from stdin.
    pub fn read_line(&mut self) -> anyhow::Result<String> {
        let mut buf = String::new();
        let bytes = self.stdin.read_line(&mut buf)?;
        if bytes == 0 {
            anyhow::bail!("setup: unexpected end of input");
        }
        Ok(buf.trim().to_string())
    }

    /// Ask a [Y/n/s] question. Re-prompts on invalid input. Empty line
    /// accepts the default.
    pub fn ask_ynskip(
        &mut self,
        question: &str,
        default: YnSkip,
    ) -> anyhow::Result<YnSkip> {
        let hint = match default {
            YnSkip::Yes => "[Y/n/s]",
            YnSkip::No => "[y/N/s]",
            YnSkip::Skip => "[y/n/S]",
        };
        loop {
            self.prompt_write(format!("{question} {hint}: "))?;
            let raw = self.read_line()?;
            match raw.to_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(YnSkip::Yes),
                "n" | "no" => return Ok(YnSkip::No),
                "s" | "skip" => return Ok(YnSkip::Skip),
                _ => {
                    self.line("(didn't catch that — y / n / s, or Enter for the default)")?;
                }
            }
        }
    }
}

/// True when stdin is a live terminal. Tests and piped inputs return false.
/// Overridable via `MAKAKOO_FORCE_TTY=0` (force not-a-tty) or
/// `MAKAKOO_FORCE_TTY=1` (force tty) — used by integration tests.
pub fn is_interactive_stdin() -> bool {
    if let Ok(override_val) = std::env::var("MAKAKOO_FORCE_TTY") {
        return override_val == "1";
    }
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn ynskip_accepts_y_variants() {
        for input in ["y\n", "Y\n", "yes\n", "YES\n"] {
            let stdin = Cursor::new(input.as_bytes().to_vec());
            let stdout: Vec<u8> = Vec::new();
            let mut ui = Ui::new(stdin, stdout);
            assert_eq!(ui.ask_ynskip("?", YnSkip::No).unwrap(), YnSkip::Yes);
        }
    }

    #[test]
    fn ynskip_accepts_n_variants() {
        for input in ["n\n", "N\n", "no\n"] {
            let stdin = Cursor::new(input.as_bytes().to_vec());
            let mut ui = Ui::new(stdin, Vec::<u8>::new());
            assert_eq!(ui.ask_ynskip("?", YnSkip::Yes).unwrap(), YnSkip::No);
        }
    }

    #[test]
    fn ynskip_accepts_s_variants() {
        for input in ["s\n", "S\n", "skip\n"] {
            let stdin = Cursor::new(input.as_bytes().to_vec());
            let mut ui = Ui::new(stdin, Vec::<u8>::new());
            assert_eq!(ui.ask_ynskip("?", YnSkip::Yes).unwrap(), YnSkip::Skip);
        }
    }

    #[test]
    fn ynskip_empty_line_picks_default() {
        let stdin = Cursor::new(b"\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        assert_eq!(ui.ask_ynskip("?", YnSkip::Yes).unwrap(), YnSkip::Yes);

        let stdin = Cursor::new(b"\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        assert_eq!(ui.ask_ynskip("?", YnSkip::Skip).unwrap(), YnSkip::Skip);
    }

    #[test]
    fn ynskip_reprompts_on_junk() {
        let stdin = Cursor::new(b"xyz\nmaybe\ny\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        assert_eq!(ui.ask_ynskip("?", YnSkip::No).unwrap(), YnSkip::Yes);
    }

    #[test]
    fn status_is_terminal() {
        let now = Utc::now();
        assert!(!SectionStatus::NotStarted.is_terminal());
        assert!(!SectionStatus::AlreadySatisfied.is_terminal());
        assert!(SectionStatus::Completed { at: now }.is_terminal());
        assert!(SectionStatus::Skipped { at: now }.is_terminal());
        assert!(!SectionStatus::Failed {
            reason: "x".into(),
            at: now
        }
        .is_terminal());
    }

    #[test]
    fn status_serde_roundtrip() {
        let now = Utc::now();
        for s in [
            SectionStatus::NotStarted,
            SectionStatus::AlreadySatisfied,
            SectionStatus::Completed { at: now },
            SectionStatus::Skipped { at: now },
            SectionStatus::Failed {
                reason: "npm missing".into(),
                at: now,
            },
        ] {
            let json = serde_json::to_string(&s).unwrap();
            let back: SectionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }
}
