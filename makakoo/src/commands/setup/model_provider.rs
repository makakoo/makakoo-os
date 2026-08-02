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
use makakoo_core::adapter::{install_from_path, InstallOptions, InstallRoot};

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
        let mut registry = match AdapterRegistry::load(AdapterRegistry::default_root()) {
            Ok(r) => r,
            Err(e) => {
                ui.line(format!(
                    "model-provider: couldn't read adapter registry: {e}"
                ))?;
                return Ok(SectionOutcome::Failed(format!("registry load: {e}")));
            }
        };

        let mut names: Vec<String> = registry.names().map(String::from).collect();
        if names.is_empty() {
            ui.line("model-provider: an adapter is what connects Makakoo to an AI model service.")?;
        ui.line("  None is set up yet.")?;
            ui.line("model-provider: installing bundled switchailocal adapter as the default local gateway …")?;

            match install_bundled_switchailocal() {
                Ok(registered_path) => {
                    ui.line(format!(
                        "model-provider: registered switchailocal at {}",
                        registered_path.display()
                    ))?;
                    registry = AdapterRegistry::load(AdapterRegistry::default_root())?;
                    names = registry.names().map(String::from).collect();
                }
                Err(e) => {
                    ui.line(format!(
                        "model-provider: couldn't install bundled switchailocal adapter: {e}"
                    ))?;
                    ui.line("  Manual fallback: makakoo adapter install switchailocal --bundled --skip-health-check")?;
                    return Ok(SectionOutcome::Failed(format!(
                        "no registered adapters and bootstrap failed: {e}"
                    )));
                }
            }
        }

        if names.is_empty() {
            ui.line("model-provider: no adapters registered in ~/.makakoo/adapters/registered/.")?;
            ui.line("  Install one first: makakoo adapter install <source>")?;
            return Ok(SectionOutcome::Failed(
                "no registered adapters to pick from".to_string(),
            ));
        }

        if names.len() == 1 && load_primary_adapter(&registry).is_none() {
            let chosen = &names[0];
            match write_primary_adapter(chosen, &registry) {
                Ok(path) => {
                    ui.line(format!(
                        "model-provider: one adapter available, primary → {chosen}. Written to {}",
                        path.display()
                    ))?;
                    ui.line("")?;
                    ui.line("Next: run  makakoo secret set AIL_API_KEY  if switchAILocal needs an upstream key.")?;
                    ui.line("Smoke-test: run  makakoo query 'hello'  in a new terminal.")?;
                    return Ok(SectionOutcome::Installed);
                }
                Err(e) => {
                    ui.line(format!("model-provider: write failed — {e}"))?;
                    return Ok(SectionOutcome::Failed(e.to_string()));
                }
            }
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
        ui.line(format!(
            "  {}. (skip — don't change the primary)",
            names.len() + 1
        ))?;
        ui.prompt_write(format!(
            "\nPick 1-{} [{} = skip]: ",
            names.len() + 1,
            names.len() + 1
        ))?;

        let raw = ui.read_line()?;
        let chosen = match resolve_adapter_pick(&raw, names.len()) {
            AdapterPick::Skip => return Ok(SectionOutcome::Skipped),
            AdapterPick::Index(idx) => &names[idx],
            AdapterPick::InvalidNumber => {
                ui.line("(that wasn't a number — keeping your current choice)")?;
                return Ok(SectionOutcome::Declined);
            }
            AdapterPick::OutOfRange => {
                ui.line("(no option with that number — keeping your current choice)")?;
                return Ok(SectionOutcome::Declined);
            }
        };

        match write_primary_adapter(chosen, &registry) {
            Ok(path) => {
                ui.line(format!(
                    "model-provider: primary → {chosen}. Written to {}",
                    path.display()
                ))?;
                // Smoke-test hint per DOGFOOD-FINDINGS F-004. Picking an
                // adapter does not verify that the adapter actually serves
                // the default `makakoo query` model alias (ail-compound).
                // A previous grandma-install shipped with an adapter that
                // succeeded setup but then failed every `makakoo query`
                // call with `unknown provider for model ail-compound`.
                // Surface the smoke-test step inline so users catch it
                // before their first real query.
                ui.line("")?;
                ui.line("Smoke-test: run  makakoo query 'hello'  in a new terminal.")?;
                ui.line("  If it prints an answer, you're done.")?;
                ui.line("  If it errors  \"unknown provider for model <alias>\"  —")?;
                ui.line("  the adapter works but the default model alias isn't registered.")?;
                ui.line("  Fix: run  makakoo query --model <alias-your-adapter-uses> 'hello'")?;
                ui.line("  or see docs/troubleshooting/tree.md#error-llm-error.")?;
                Ok(SectionOutcome::Installed)
            }
            Err(e) => {
                ui.line(format!("model-provider: write failed — {e}"))?;
                Ok(SectionOutcome::Failed(e.to_string()))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterPick {
    Index(usize),
    Skip,
    InvalidNumber,
    OutOfRange,
}

fn resolve_adapter_pick(raw: &str, adapter_count: usize) -> AdapterPick {
    let trimmed = raw.trim();
    let skip_number = adapter_count + 1;
    if trimmed.is_empty() {
        return AdapterPick::Skip;
    }
    let Ok(n) = trimmed.parse::<usize>() else {
        return AdapterPick::InvalidNumber;
    };
    if n == skip_number {
        AdapterPick::Skip
    } else if n < 1 || n > adapter_count {
        AdapterPick::OutOfRange
    } else {
        AdapterPick::Index(n - 1)
    }
}

fn install_bundled_switchailocal() -> anyhow::Result<std::path::PathBuf> {
    let source_dir = crate::commands::adapter::bundled_adapter_dir("switchailocal")
        .ok_or_else(|| anyhow::anyhow!("bundled adapter `switchailocal` not found"))?;
    let root = InstallRoot::default_from_env();
    let report = install_from_path(
        &source_dir,
        &root,
        InstallOptions {
            allow_unsigned: true,
            accept_re_trust: true,
            skip_health_check: true,
        },
    )?;
    Ok(report.registered_path)
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

    #[test]
    fn adapter_pick_empty_defaults_to_skip() {
        assert_eq!(resolve_adapter_pick("", 2), AdapterPick::Skip);
        assert_eq!(resolve_adapter_pick("  ", 2), AdapterPick::Skip);
    }

    #[test]
    fn adapter_pick_maps_one_based_index_to_zero_based() {
        assert_eq!(resolve_adapter_pick("1", 2), AdapterPick::Index(0));
        assert_eq!(resolve_adapter_pick("2", 2), AdapterPick::Index(1));
    }

    #[test]
    fn adapter_pick_skip_and_invalid_paths() {
        assert_eq!(resolve_adapter_pick("3", 2), AdapterPick::Skip);
        assert_eq!(resolve_adapter_pick("4", 2), AdapterPick::OutOfRange);
        assert_eq!(resolve_adapter_pick("nope", 2), AdapterPick::InvalidNumber);
    }
}
