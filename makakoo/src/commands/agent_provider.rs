//! Legacy Flue provider selection. DSH always routes through switchAILocal.

use std::io::IsTerminal;

use makakoo_core::agents::llm_provider::{DiscoveredProvider, ProviderSource};
use makakoo_core::agents::spec::AgentSpec;

use crate::output;

pub fn choose(
    spec: &AgentSpec,
    providers: &[DiscoveredProvider],
) -> (AgentSpec, Option<DiscoveredProvider>) {
    let mut sorted = providers.to_vec();
    sorted.sort_by_key(|provider| match &provider.source {
        ProviderSource::Local { .. } => 0,
        ProviderSource::EnvVar { .. } => 1,
        ProviderSource::Catalog => 2,
    });
    let requested = spec.model.split('/').next().unwrap_or("");
    let preferred = sorted
        .iter()
        .find(|provider| provider.id == requested)
        .cloned();
    let chosen = preferred.or_else(|| choose_fallback(&sorted));
    let mut resolved = spec.clone();
    if let Some(provider) = chosen.as_ref() {
        let has_specific_model = resolved
            .model
            .split_once('/')
            .map(|(_, model)| !model.is_empty())
            .unwrap_or(false);
        if !has_specific_model {
            resolved.model = format!("{}/{}", provider.id, provider.default_model);
        }
    }
    (resolved, chosen)
}

fn choose_fallback(sorted: &[DiscoveredProvider]) -> Option<DiscoveredProvider> {
    match sorted.len() {
        0 => {
            output::print_warn(
                "No Flue LLM providers detected. Set a provider key or start switchAILocal.",
            );
            None
        }
        1 => sorted.first().cloned(),
        _ if !std::io::stdin().is_terminal() => {
            let selected = sorted[0].clone();
            output::print_warn(format!(
                "Multiple Flue providers detected; auto-selecting '{}' (local-first).",
                selected.id
            ));
            Some(selected)
        }
        _ => choose_interactively(sorted),
    }
}

fn choose_interactively(sorted: &[DiscoveredProvider]) -> Option<DiscoveredProvider> {
    use std::io::Write;
    println!("Multiple Flue LLM providers detected. Which one to use?");
    for (index, provider) in sorted.iter().enumerate() {
        println!(
            "  {}. {} — model: {}",
            index + 1,
            provider.display_name,
            provider.default_model
        );
    }
    print!("Enter choice [1]: ");
    std::io::stdout().flush().ok();
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();
    let choice: usize = input.trim().parse().unwrap_or(1);
    selected_choice(sorted, choice)
}

fn selected_choice(sorted: &[DiscoveredProvider], choice: usize) -> Option<DiscoveredProvider> {
    if !(1..=sorted.len()).contains(&choice) {
        output::print_warn(format!(
            "Flue provider choice {} is out of range 1..={}; keeping the spec model.",
            choice,
            sorted.len()
        ));
        return None;
    }
    sorted.get(choice - 1).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> DiscoveredProvider {
        DiscoveredProvider {
            id: id.into(),
            display_name: id.into(),
            default_model: "model".into(),
            source: ProviderSource::Catalog,
            requires_api_key: false,
            base_url: None,
            api_protocol: "openai-completions".into(),
        }
    }

    #[test]
    fn interactive_choice_zero_is_rejected() {
        let providers = vec![provider("first"), provider("second")];
        assert!(selected_choice(&providers, 0).is_none());
        assert_eq!(selected_choice(&providers, 1).unwrap().id, "first");
        assert!(selected_choice(&providers, 3).is_none());
    }
}
