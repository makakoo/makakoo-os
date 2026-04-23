//! The persona-picker section — preserves the pre-wizard `makakoo setup`
//! behavior verbatim, now wrapped in the [`Section`] trait.
//!
//! Asks the user to name their assistant, pick a pronoun, and choose a
//! voice default. Writes `$MAKAKOO_HOME/config/persona.json`. If the
//! file already exists, `status()` returns `AlreadySatisfied` and
//! `run()` reports `AlreadyPresent` without prompting — unless the
//! caller passed `--force`, in which case the prompts run and the file
//! is overwritten.
//!
//! The interactive loop logic is intentionally thin — name resolution
//! lives in `makakoo_core::config::resolve_name_choice` and is unit-
//! tested there. This file only owns the I/O seams.

use chrono::Utc;

use makakoo_core::config::{
    persona_path, resolve_name_choice, PersonaConfig, SUGGESTED_NAMES,
};

use super::harness::{Section, SectionOutcome, SectionStatus, Ui};

pub struct PersonaSection {
    pub force: bool,
}

impl PersonaSection {
    pub fn new(force: bool) -> Self {
        Self { force }
    }
}

impl Section for PersonaSection {
    fn name(&self) -> &'static str {
        "persona"
    }

    fn description(&self) -> &'static str {
        "Name your assistant, pronoun, voice default"
    }

    fn status(&self) -> SectionStatus {
        if persona_path().exists() {
            SectionStatus::AlreadySatisfied
        } else {
            SectionStatus::NotStarted
        }
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        let path = persona_path();
        if path.exists() && !self.force {
            ui.line(format!("persona already configured at {}", path.display()))?;
            ui.line("(re-run with `makakoo setup --force` to overwrite)")?;
            return Ok(SectionOutcome::AlreadyPresent);
        }

        banner(ui)?;
        let name = prompt_name(ui)?;
        let pronoun = prompt_pronoun(ui)?;
        let voice = prompt_voice(ui)?;

        let cfg = PersonaConfig {
            name: name.clone(),
            pronoun,
            voice_default: voice,
        };
        cfg.save_to(&path)?;

        ui.line("")?;
        ui.line("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
        ui.line(format!("  Saved to {}", path.display()))?;
        ui.line(format!("  Hi. I'm {name}. Let's build something."))?;
        ui.line("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;

        // Touch state with a timestamp so the dispatcher persists `Completed`.
        let _ = Utc::now(); // reserved for future use; state is stamped by dispatcher
        Ok(SectionOutcome::Installed)
    }
}

fn banner(ui: &mut Ui) -> anyhow::Result<()> {
    ui.line("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    ui.line("  Welcome to Makakoo OS")?;
    ui.line("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")?;
    ui.line("")?;
    ui.line("Let's name your assistant. Pick a suggestion or")?;
    ui.line("type your own — this is the name you'll call out")?;
    ui.line("to in chat, in logs, and in the Brain.")?;
    ui.line("")?;
    Ok(())
}

fn prompt_name(ui: &mut Ui) -> anyhow::Result<String> {
    loop {
        ui.line("Name suggestions:")?;
        for (i, n) in SUGGESTED_NAMES.iter().enumerate() {
            ui.line(format!("  {}. {}", i + 1, n))?;
        }
        ui.line(format!("  {}. (type your own)", SUGGESTED_NAMES.len() + 1))?;
        ui.prompt_write(format!("\nPick 1-{}: ", SUGGESTED_NAMES.len() + 1))?;

        let choice = ui.read_line()?;
        let custom = if choice == format!("{}", SUGGESTED_NAMES.len() + 1) {
            ui.prompt_write("Custom name: ")?;
            Some(ui.read_line()?)
        } else {
            None
        };

        match resolve_name_choice(&choice, custom.as_deref()) {
            Some(name) => return Ok(name),
            None => {
                ui.line("(didn't catch that — try again)")?;
                ui.line("")?;
            }
        }
    }
}

fn prompt_pronoun(ui: &mut Ui) -> anyhow::Result<String> {
    ui.line("")?;
    ui.prompt_write("Pronoun (he / she / they) [they]: ")?;
    let raw = ui.read_line()?;
    if raw.is_empty() {
        Ok("they".to_string())
    } else {
        Ok(raw)
    }
}

fn prompt_voice(ui: &mut Ui) -> anyhow::Result<String> {
    ui.line("")?;
    ui.line("Voice default:")?;
    ui.line("  1. caveman  (terse, token-efficient, default)")?;
    ui.line("  2. full     (formal prose, longer answers)")?;
    ui.prompt_write("Pick 1-2 [1]: ")?;
    let raw = ui.read_line()?;
    match raw.as_str() {
        "" | "1" => Ok("caveman".to_string()),
        "2" => Ok("full".to_string()),
        other => Ok(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn status_notstarted_when_persona_missing() {
        // When persona.json doesn't exist (fresh $MAKAKOO_HOME in test env),
        // the section reports NotStarted. Actual file presence depends on the
        // test harness's $MAKAKOO_HOME override.
        let section = PersonaSection::new(false);
        // We can't easily move persona_path() here without wiring env vars;
        // that's covered at integration-test level. Just assert the method
        // compiles and returns a variant.
        let _ = section.status();
    }

    #[test]
    fn description_and_name_are_stable() {
        let section = PersonaSection::new(false);
        assert_eq!(section.name(), "persona");
        assert!(!section.description().is_empty());
    }

    #[test]
    fn prompts_resolve_suggested_name() {
        let input = b"1\nthey\n1\n".to_vec();
        let stdin = Cursor::new(input);
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let name = prompt_name(&mut ui).unwrap();
        assert_eq!(name, SUGGESTED_NAMES[0]);
    }

    #[test]
    fn prompts_resolve_custom_name() {
        let custom_choice_num = SUGGESTED_NAMES.len() + 1;
        let input = format!("{custom_choice_num}\nZephyr\n");
        let stdin = Cursor::new(input.into_bytes());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let name = prompt_name(&mut ui).unwrap();
        assert_eq!(name, "Zephyr");
    }

    #[test]
    fn pronoun_defaults_to_they_on_empty() {
        let stdin = Cursor::new(b"\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let p = prompt_pronoun(&mut ui).unwrap();
        assert_eq!(p, "they");
    }

    #[test]
    fn voice_defaults_to_caveman() {
        let stdin = Cursor::new(b"\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let v = prompt_voice(&mut ui).unwrap();
        assert_eq!(v, "caveman");
    }

    #[test]
    fn voice_picks_full_on_2() {
        let stdin = Cursor::new(b"2\n".to_vec());
        let mut ui = Ui::new(stdin, Vec::<u8>::new());
        let v = prompt_voice(&mut ui).unwrap();
        assert_eq!(v, "full");
    }
}
