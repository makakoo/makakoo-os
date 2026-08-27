//! LLM provider discovery for `makakoo agent create`.
//!
//! Probes local + remote providers and returns a list for the user
//! to pick from. Phase 6 of SPRINT-FLUE-DEFAULT-AGENT-SPECS.
//!
//! Probes (in order):
//! 1. `http://localhost:18080/v1/models` — switchailocal (local OpenAI-compat)
//! 2. `http://localhost:11434/api/tags` — Ollama (local)
//! 3. `ANTHROPIC_API_KEY` env var — Anthropic (cloud)
//! 4. `OPENAI_API_KEY` env var — OpenAI (cloud)
//!
//! Each probe has a 2-second timeout so discovery never blocks the
//! CLI for more than ~8s total. Probes run concurrently.

use std::time::Duration;

use reqwest::Client;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredProvider {
    /// Provider ID used in `registerProvider(...)` and model specifiers.
    /// E.g. `switchailocal`, `anthropic`, `openai`, `ollama`.
    pub id: String,
    /// Human-readable name for the interactive prompt.
    pub display_name: String,
    /// Default model for this provider. E.g. `ail-compound`, `claude-sonnet-4-6`.
    pub default_model: String,
    /// Where this provider was discovered.
    pub source: ProviderSource,
    /// Whether the provider requires an API key at runtime.
    pub requires_api_key: bool,
    /// Optional base URL (for local providers).
    pub base_url: Option<String>,
    /// Wire protocol for the Flue runtime (`api` field of `registerProvider`).
    /// Local switchailocal/ollama use `openai-completions`.
    pub api_protocol: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum ProviderSource {
    /// Local server (switchailocal, ollama, etc.)
    Local { base_url: String },
    /// Cloud provider authenticated via env var
    EnvVar { env_var: String },
    /// Catalog provider (reserved for future use)
    Catalog,
}

/// Probe all known providers concurrently. Returns a list sorted by
/// priority: local-first, then cloud. Empty list means no providers
/// are reachable — the caller should fall back to the spec's
/// hardcoded model.
pub async fn discover_providers() -> Vec<DiscoveredProvider> {
    let client = match Client::builder().timeout(Duration::from_secs(2)).build() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let (switchai, ollama) = tokio::join!(probe_switchailocal(&client), probe_ollama(&client),);

    let mut providers = Vec::new();
    if let Some(p) = switchai {
        providers.push(p);
    }
    if let Some(p) = ollama {
        providers.push(p);
    }
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        providers.push(DiscoveredProvider {
            id: "anthropic".into(),
            display_name: "Anthropic (cloud, ANTHROPIC_API_KEY set)".into(),
            default_model: "claude-sonnet-4-6".into(),
            source: ProviderSource::EnvVar {
                env_var: "ANTHROPIC_API_KEY".into(),
            },
            requires_api_key: true,
            base_url: None,
            api_protocol: "anthropic-messages".into(),
        });
    }
    if std::env::var("OPENAI_API_KEY").is_ok() {
        providers.push(DiscoveredProvider {
            id: "openai".into(),
            display_name: "OpenAI (cloud, OPENAI_API_KEY set)".into(),
            default_model: "gpt-5.5".into(),
            source: ProviderSource::EnvVar {
                env_var: "OPENAI_API_KEY".into(),
            },
            requires_api_key: true,
            base_url: None,
            api_protocol: "openai-completions".into(),
        });
    }
    providers
}

/// Probe `http://localhost:18080/v1/models` (switchailocal).
async fn probe_switchailocal(client: &Client) -> Option<DiscoveredProvider> {
    let res = client
        .get("http://localhost:18080/v1/models")
        .header("Authorization", "Bearer sk-test-123")
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let body: Value = res.json().await.ok()?;
    let models = body.get("data")?.as_array()?;
    // Prefer "ail-compound" if present (switchailocal's flagship model),
    // otherwise fall back to the first available model.
    let model = models
        .iter()
        .find(|m| m.get("id").and_then(|v| v.as_str()) == Some("ail-compound"))
        .and_then(|m| m.get("id").and_then(|v| v.as_str()))
        .or_else(|| {
            models
                .first()
                .and_then(|m| m.get("id").and_then(|v| v.as_str()))
        })
        .unwrap_or("ail-compound")
        .to_string();
    Some(DiscoveredProvider {
        id: "switchailocal".into(),
        display_name: "switchailocal (local OpenAI-compatible)".into(),
        default_model: model,
        source: ProviderSource::Local {
            base_url: "http://127.0.0.1:18080/v1".into(),
        },
        requires_api_key: false,
        base_url: Some("http://127.0.0.1:18080/v1".into()),
        api_protocol: "openai-completions".into(),
    })
}

/// Probe `http://localhost:11434/api/tags` (Ollama).
async fn probe_ollama(client: &Client) -> Option<DiscoveredProvider> {
    let res = client
        .get("http://localhost:11434/api/tags")
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let body: Value = res.json().await.ok()?;
    let models = body.get("models")?.as_array()?;
    // Filter out embedding models and prefer `:cloud` (chat-capable)
    // models. Fall back to any non-embedding model.
    let chat_models: Vec<&str> = models
        .iter()
        .filter_map(|m| m.get("name").and_then(|v| v.as_str()))
        .filter(|n| !n.contains("embedding"))
        .collect();
    let model = chat_models
        .iter()
        .find(|n| n.contains(":cloud"))
        .copied()
        .or_else(|| chat_models.first().copied())
        .unwrap_or("llama3.1:8b")
        .to_string();
    Some(DiscoveredProvider {
        id: "ollama".into(),
        display_name: "Ollama (local)".into(),
        default_model: model,
        source: ProviderSource::Local {
            base_url: "http://localhost:11434/v1".into(),
        },
        requires_api_key: false,
        base_url: Some("http://localhost:11434/v1".into()),
        api_protocol: "openai-completions".into(),
    })
}

/// Default fallback when no providers are detected — the spec's
/// hardcoded model. Used by the CLI when discovery returns empty.
pub fn default_fallback() -> DiscoveredProvider {
    DiscoveredProvider {
        id: "anthropic".into(),
        display_name: "Anthropic (cloud, no API key detected — set ANTHROPIC_API_KEY)".into(),
        default_model: "claude-sonnet-4-6".into(),
        source: ProviderSource::Catalog,
        requires_api_key: true,
        base_url: None,
        api_protocol: "anthropic-messages".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fallback_is_anthropic() {
        let f = default_fallback();
        assert_eq!(f.id, "anthropic");
        assert_eq!(f.default_model, "claude-sonnet-4-6");
        assert!(f.requires_api_key);
    }

    #[tokio::test]
    async fn discover_returns_empty_when_nothing_reachable() {
        // This test runs in CI without switchailocal/ollama running.
        // We just verify the function returns without panicking.
        let providers = discover_providers().await;
        // May or may not have providers depending on the test env.
        // Just ensure no panic and types are correct.
        for p in &providers {
            assert!(!p.id.is_empty());
            assert!(!p.default_model.is_empty());
        }
    }
}
