//! `makakoo setup` — interactive section dispatcher.
//!
//! Extends the previous one-shot persona picker into a re-runnable
//! wizard. Each step is a discrete [`Section`] that owns its own prompt
//! flow; this module stitches them together:
//!
//! 1. Apply filters (`--only`, `--skip`, platform gate).
//! 2. TTY gate — non-interactive stdin or `--non-interactive` prints
//!    state and exits cleanly.
//! 3. Load `completed.json` from `$MAKAKOO_HOME/state/makakoo-setup/`.
//! 4. For each selected section:
//!    - Call `status()` — if already completed/skipped and the user
//!      didn't force a re-run, print one-line status and continue.
//!    - Call `run()` — the section owns its prompts.
//!    - Map the [`SectionOutcome`] to a [`SectionStatus`] and persist.
//! 5. Print a final summary table.
//!
//! Phase 1 registers only the persona section. Brain, CLI-agent,
//! terminal, model-provider, and infect sections land in subsequent
//! sprint phases.

use std::collections::HashSet;
use std::io::{stdin, stdout};

use chrono::Utc;
use makakoo_core::platform::makakoo_home;

pub mod brain;
pub mod cli_agent;
pub mod harness;
pub mod infect_section;
pub mod model_provider;
pub mod persona;
pub mod state;
pub mod terminal;

#[cfg(test)]
pub(crate) mod test_support;

pub use harness::{
    is_interactive_stdin, Section, SectionOutcome, SectionStatus, Ui,
};

/// Parsed CLI args for `makakoo setup`. One value type so downstream
/// changes to `cli.rs` are localized.
#[derive(Debug, Clone, Default)]
pub struct SetupArgs {
    /// Positional section name — if set, only this section runs.
    pub section: Option<String>,
    /// Run only these sections (by name).
    pub only: Vec<String>,
    /// Run every section except these.
    pub skip: Vec<String>,
    /// Skip interactive prompts — just print state and exit.
    pub non_interactive: bool,
    /// Wipe `completed.json` before running.
    pub reset: bool,
    /// Re-run the persona section even if persona.json exists.
    pub force: bool,
}

/// Entry point called by the top-level command dispatcher.
pub fn run(args: SetupArgs) -> anyhow::Result<i32> {
    let home = makakoo_home();

    if args.reset {
        state::reset(&home)?;
        println!("Reset: wiped {}", state::state_path_for(&home).display());
    }

    // Build the section registry. Later phases append more sections
    // in the canonical order: persona → brain → cli-agent → terminal
    // → model-provider → infect.
    let mut sections: Vec<Box<dyn Section>> = vec![
        Box::new(persona::PersonaSection::new(args.force)),
        Box::new(brain::BrainSection::new()),
        Box::new(cli_agent::CliAgentSection::new()),
        Box::new(terminal::TerminalSection::new()),
        Box::new(model_provider::ModelProviderSection::new()),
        Box::new(infect_section::InfectSection::new()),
    ];

    // Filter sections per --only / --skip / positional `section` / platform gate.
    let wanted = select_sections(&sections, &args)?;
    if wanted.is_empty() {
        eprintln!("setup: no sections matched the given filters. Nothing to do.");
        return Ok(1);
    }

    // Non-interactive or non-TTY → print state table and exit cleanly.
    let state_snapshot = state::load(&home);
    if args.non_interactive || !is_interactive_stdin() {
        if !args.non_interactive && !is_interactive_stdin() {
            println!("setup: not running on a live terminal. Use `makakoo setup --non-interactive` for a state report, or run from a TTY to start the wizard.");
        }
        print_summary_table(&sections, &state_snapshot, &wanted);
        return Ok(0);
    }

    // Interactive path — run each selected section in order.
    let mut state = state_snapshot;
    let stdin = stdin();
    let stdin = stdin.lock();
    let stdout = stdout();
    let stdout = stdout.lock();
    let mut ui = Ui::new(stdin, stdout);

    for idx in &wanted {
        let section = &mut sections[*idx];
        if !section.is_applicable() {
            // Platform-gated section; silently skip.
            continue;
        }
        let current = state.get(section.name());
        let is_persona_with_force = section.name() == "persona" && args.force;
        if current.is_terminal() && !is_persona_with_force {
            ui.line(format!(
                "{} — {} (skip; re-run with --reset to re-ask)",
                section.name(),
                current.label()
            ))?;
            continue;
        }

        ui.line("")?;
        ui.line(format!(
            "── {} — {} ──",
            section.name(),
            section.description()
        ))?;

        let outcome = section.run(&mut ui)?;
        let new_status = match outcome {
            SectionOutcome::Installed => Some(SectionStatus::Completed { at: Utc::now() }),
            SectionOutcome::AlreadyPresent => None, // don't persist; detect each run
            SectionOutcome::Declined => None,       // don't persist; re-ask next run
            SectionOutcome::Skipped => Some(SectionStatus::Skipped { at: Utc::now() }),
            SectionOutcome::Failed(reason) => Some(SectionStatus::Failed {
                reason,
                at: Utc::now(),
            }),
        };
        if let Some(status) = new_status {
            state.set(section.name(), status);
            state::save(&home, &state)?;
        }
    }

    ui.line("")?;
    ui.line("── summary ──")?;
    print_summary_table_ui(&mut ui, &sections, &state, &wanted)?;

    Ok(0)
}

/// Resolve which section indices to run based on positional/flag filters.
/// Returns indices into the full registry so platform-gated sections
/// still show in the summary table as "not applicable".
fn select_sections(
    sections: &[Box<dyn Section>],
    args: &SetupArgs,
) -> anyhow::Result<Vec<usize>> {
    let valid_names: HashSet<&'static str> =
        sections.iter().map(|s| s.name()).collect();

    // Positional `section` wins if set.
    if let Some(name) = &args.section {
        if !valid_names.contains(name.as_str()) {
            anyhow::bail!(
                "unknown section: {name:?}. Valid: {:?}",
                sections.iter().map(|s| s.name()).collect::<Vec<_>>()
            );
        }
        return Ok(sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name() == name)
            .map(|(i, _)| i)
            .collect());
    }

    // --only wins over --skip.
    if !args.only.is_empty() {
        for name in &args.only {
            if !valid_names.contains(name.as_str()) {
                anyhow::bail!(
                    "unknown section in --only: {name:?}. Valid: {:?}",
                    sections.iter().map(|s| s.name()).collect::<Vec<_>>()
                );
            }
        }
        let only_set: HashSet<&str> = args.only.iter().map(String::as_str).collect();
        return Ok(sections
            .iter()
            .enumerate()
            .filter(|(_, s)| only_set.contains(s.name()))
            .map(|(i, _)| i)
            .collect());
    }

    // --skip filters from the full set.
    for name in &args.skip {
        if !valid_names.contains(name.as_str()) {
            anyhow::bail!(
                "unknown section in --skip: {name:?}. Valid: {:?}",
                sections.iter().map(|s| s.name()).collect::<Vec<_>>()
            );
        }
    }
    let skip_set: HashSet<&str> = args.skip.iter().map(String::as_str).collect();
    Ok(sections
        .iter()
        .enumerate()
        .filter(|(_, s)| !skip_set.contains(s.name()))
        .map(|(i, _)| i)
        .collect())
}

/// Resolve the label to show for a section. Precedence:
///
/// 1. If the state file has a terminal entry (Completed / Skipped /
///    Failed) for the section, surface that — it's the authoritative
///    record of what the wizard did.
/// 2. Otherwise ask the section for its live `status()` (detects
///    `AlreadySatisfied` from reality, e.g. "persona.json exists" or
///    "brain_sources.json has multi-source").
fn resolve_display_status(
    section: &dyn Section,
    state: &state::StateFile,
) -> SectionStatus {
    let from_state = state.get(section.name());
    if from_state.is_terminal() || matches!(from_state, SectionStatus::Failed { .. }) {
        from_state
    } else {
        section.status()
    }
}

fn print_summary_table(
    sections: &[Box<dyn Section>],
    state: &state::StateFile,
    selected: &[usize],
) {
    let name_w = sections.iter().map(|s| s.name().len()).max().unwrap_or(8);
    for idx in selected {
        let s = &sections[*idx];
        let status = resolve_display_status(s.as_ref(), state);
        let applicable = if s.is_applicable() { "" } else { " (n/a)" };
        println!(
            "  {:<width$}  {}{}",
            s.name(),
            status.label(),
            applicable,
            width = name_w
        );
    }
}

fn print_summary_table_ui(
    ui: &mut Ui,
    sections: &[Box<dyn Section>],
    state: &state::StateFile,
    selected: &[usize],
) -> anyhow::Result<()> {
    let name_w = sections.iter().map(|s| s.name().len()).max().unwrap_or(8);
    for idx in selected {
        let s = &sections[*idx];
        let status = resolve_display_status(s.as_ref(), state);
        let applicable = if s.is_applicable() { "" } else { " (n/a)" };
        ui.line(format!(
            "  {:<width$}  {}{}",
            s.name(),
            status.label(),
            applicable,
            width = name_w
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<Box<dyn Section>> {
        vec![
            Box::new(persona::PersonaSection::new(false)),
            Box::new(brain::BrainSection::new()),
            Box::new(cli_agent::CliAgentSection::new()),
            Box::new(terminal::TerminalSection::new()),
            Box::new(model_provider::ModelProviderSection::new()),
            Box::new(infect_section::InfectSection::new()),
        ]
    }

    #[test]
    fn select_default_returns_all_sections() {
        let sections = registry();
        let args = SetupArgs::default();
        let sel = select_sections(&sections, &args).unwrap();
        assert_eq!(sel, (0..sections.len()).collect::<Vec<_>>());
    }

    #[test]
    fn select_positional_filters_to_one() {
        let sections = registry();
        let args = SetupArgs {
            section: Some("persona".into()),
            ..Default::default()
        };
        let sel = select_sections(&sections, &args).unwrap();
        assert_eq!(sel, vec![0]);

        let args = SetupArgs {
            section: Some("brain".into()),
            ..Default::default()
        };
        let sel = select_sections(&sections, &args).unwrap();
        assert_eq!(sel, vec![1]);
    }

    #[test]
    fn select_unknown_positional_errors() {
        let sections = registry();
        let args = SetupArgs {
            section: Some("nonexistent".into()),
            ..Default::default()
        };
        let err = select_sections(&sections, &args).unwrap_err();
        assert!(err.to_string().contains("unknown section"));
    }

    #[test]
    fn select_only_filters() {
        let sections = registry();
        let args = SetupArgs {
            only: vec!["persona".into()],
            ..Default::default()
        };
        let sel = select_sections(&sections, &args).unwrap();
        assert_eq!(sel, vec![0]);
    }

    #[test]
    fn select_skip_removes_section() {
        let sections = registry();
        let args = SetupArgs {
            skip: vec!["persona".into()],
            ..Default::default()
        };
        let sel = select_sections(&sections, &args).unwrap();
        // every section except persona (index 0)
        assert_eq!(sel, (1..sections.len()).collect::<Vec<_>>());
    }

    #[test]
    fn select_unknown_only_errors() {
        let sections = registry();
        let args = SetupArgs {
            only: vec!["ghost".into()],
            ..Default::default()
        };
        assert!(select_sections(&sections, &args).is_err());
    }

    #[test]
    fn select_unknown_skip_errors() {
        let sections = registry();
        let args = SetupArgs {
            skip: vec!["ghost".into()],
            ..Default::default()
        };
        assert!(select_sections(&sections, &args).is_err());
    }
}
