//! The model-provider section — names which registered adapter is the
//! "primary" routing target. Writes
//! `~/.makakoo/primary_adapter.toml` via the primitive in
//! `makakoo_core::adapter::registry`.
//!
//! Intentionally narrow: this section does not prompt for API keys or
//! transport-specific settings. Per-adapter credential setup lives in
//! each adapter's own `install`/`doctor` flow.

use makakoo_core::adapter::registry::{
    load_primary_adapter, write_primary_adapter, AdapterRegistry,
};

use super::harness::{Section, SectionOutcome, SectionStatus, Ui};

pub struct ModelProviderSection;

impl ModelProviderSection {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ModelProviderSection {
    fn default() -> Self {
        Self::new()
    }
}

impl Section for ModelProviderSection {
    fn name(&self) -> &'static str {
        "model-provider"
    }

    fn description(&self) -> &'static str {
        "Pick the primary adapter (LLM gateway)"
    }

    fn status(&self) -> SectionStatus {
        let Ok(registry) = AdapterRegistry::load(AdapterRegistry::default_root()) else {
            return SectionStatus::NotStarted;
        };
        if load_primary_adapter(&registry).is_some() {
            SectionStatus::AlreadySatisfied
        } else {
            SectionStatus::NotStarted
        }
    }

    fn run(&mut self, ui: &mut Ui) -> anyhow::Result<SectionOutcome> {
        let registry = match AdapterRegistry::load(AdapterRegistry::default_root()) {
            Ok(r) => r,
            Err(e) => {
                ui.line(format!(
                    "model-provider: couldn't read adapter registry: {e}"
                ))?;
                return Ok(SectionOutcome::Failed(format!("registry load: {e}")));
            }
        };

        let names: Vec<String> = registry.names().map(String::from).collect();
        if names.is_empty() {
            ui.line("model-provider: no adapters registered in ~/.makakoo/adapters/registered/.")?;
            ui.line("  Install one first: makakoo adapter install <source>")?;
            return Ok(SectionOutcome::Failed(
                "no registered adapters to pick from".to_string(),
            ));
        }

        if let Some(current) = load_primary_adapter(&registry) {
            ui.line(format!("model-provider: current primary → {current}"))?;
        } else {
            ui.line("model-provider: no primary adapter set yet.")?;
        }

        ui.line("")?;
        ui.line("Registered adapters:")?;
        for (i, n) in names.iter().enumerate() {
            let summary = registry
                .get(n)
                .map(|a| a.manifest.adapter.description.as_str())
                .unwrap_or("");
            ui.line(format!("  {}. {} — {}", i + 1, n, summary))?;
        }
        ui.line(format!("  {}. (skip — don't change the primary)", names.len() + 1))?;
        ui.prompt_write(format!("\nPick 1-{}: ", names.len() + 1))?;

        let raw = ui.read_line()?;
        let Ok(n) = raw.parse::<usize>() else {
            ui.line("(not a number — leaving primary unchanged)")?;
            return Ok(SectionOutcome::Declined);
        };
        if n == names.len() + 1 {
            return Ok(SectionOutcome::Skipped);
        }
        if n < 1 || n > names.len() {
            ui.line("(out of range — leaving primary unchanged)")?;
            return Ok(SectionOutcome::Declined);
        }
        let chosen = &names[n - 1];

        match write_primary_adapter(chosen, &registry) {
            Ok(path) => {
                ui.line(format!(
                    "model-provider: primary → {chosen}. Written to {}",
                    path.display()
                ))?;
                Ok(SectionOutcome::Installed)
            }
            Err(e) => {
                ui.line(format!("model-provider: write failed — {e}"))?;
                Ok(SectionOutcome::Failed(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_description_stable() {
        let s = ModelProviderSection::new();
        assert_eq!(s.name(), "model-provider");
        assert!(!s.description().is_empty());
    }

    #[test]
    fn status_method_does_not_panic_with_no_registry() {
        // Status should return NotStarted (or AlreadySatisfied, depending on
        // the user's actual ~/.makakoo/) but never panic.
        let s = ModelProviderSection::new();
        let _ = s.status();
    }
}
