//! Per-slot LLM override — Phase 4 of v2-mega.
//!
//! Locked Q4:
//!
//! ```toml
//! [llm.inherit]   # docs-only marker, no field values
//!
//! [llm.override]  # explicit per-slot overrides
//! model            = "claude-opus-4-7"
//! max_tokens       = 8192
//! temperature      = 0.7
//! reasoning_effort = "medium"
//! ```
//!
//! Resolution: per-call args > slot.toml `[llm.override]` > makakoo
//! system defaults.
//!
//! Validation at create-time: `agent create` and `agent validate`
//! call SwitchAILocal `/v1/models` and reject unknown model ids;
//! network failure → warn, not error (offline workflow).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Reasoning-effort knob — three locked tiers per the v2 spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

/// Documentation-only marker. Loader accepts the section but ignores
/// any field values. Present so users editing slot.toml can self-
/// document which fields will inherit from system defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmInherit {
    /// Free-form notes; never read by the resolver.
    #[serde(default, flatten)]
    pub notes: HashMap<String, toml::Value>,
}

/// Explicit per-slot overrides. Any field set here wins over the
/// system default for that one slot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LlmOverride {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl LlmOverride {
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.max_tokens.is_none()
            && self.temperature.is_none()
            && self.reasoning_effort.is_none()
    }
}

/// System-level defaults (from `~/MAKAKOO/config/makakoo.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LlmDefaults {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub reasoning_effort: ReasoningEffort,
    pub top_p: f32,
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        ReasoningEffort::Medium
    }
}

impl LlmDefaults {
    /// The makakoo built-in fallback when no system config is set.
    /// Used in tests and when `~/MAKAKOO/config/makakoo.toml` is
    /// missing.
    pub fn builtin_fallback() -> Self {
        Self {
            model: "ail-compound".into(),
            max_tokens: 4096,
            temperature: 0.7,
            reasoning_effort: ReasoningEffort::Medium,
            top_p: 1.0,
        }
    }
}

/// Where each effective field came from. Powers `agent show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmSource {
    Override,
    SystemDefault,
}

/// Effective per-slot config + source attribution. Returned by
/// `resolve_effective`; rendered by `agent show`.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveLlm {
    pub model: (String, LlmSource),
    pub max_tokens: (u32, LlmSource),
    pub temperature: (f32, LlmSource),
    pub reasoning_effort: (ReasoningEffort, LlmSource),
    pub top_p: (f32, LlmSource),
}

impl EffectiveLlm {
    /// Render the locked `agent show` block with per-field source
    /// attribution.
    pub fn render_human(&self) -> String {
        fn label(s: LlmSource) -> &'static str {
            match s {
                LlmSource::Override => "[override]",
                LlmSource::SystemDefault => "[system default]",
            }
        }
        let (m, ms) = (&self.model.0, self.model.1);
        let (mt, mts) = (self.max_tokens.0, self.max_tokens.1);
        let (t, ts) = (self.temperature.0, self.temperature.1);
        let (re, res) = (self.reasoning_effort.0, self.reasoning_effort.1);
        let (tp, tps) = (self.top_p.0, self.top_p.1);
        format!(
            concat!(
                "llm:\n",
                "  model:            {m:<20} {ms}\n",
                "  max_tokens:       {mt:<20} {mts}\n",
                "  temperature:      {t:<20} {ts}\n",
                "  reasoning_effort: {re:<20} {res}\n",
                "  top_p:            {tp:<20} {tps}\n",
            ),
            m = m,
            ms = label(ms),
            mt = mt,
            mts = label(mts),
            t = format!("{:.2}", t),
            ts = label(ts),
            re = match re {
                ReasoningEffort::Low => "low",
                ReasoningEffort::Medium => "medium",
                ReasoningEffort::High => "high",
            },
            res = label(res),
            tp = format!("{:.2}", tp),
            tps = label(tps),
        )
    }
}

/// Apply the locked precedence order: `[llm.override]` field wins,
/// otherwise inherit from `defaults`.
pub fn resolve_effective(
    over: Option<&LlmOverride>,
    defaults: &LlmDefaults,
) -> EffectiveLlm {
    let over = over.cloned().unwrap_or_default();
    let model = match over.model {
        Some(m) => (m, LlmSource::Override),
        None => (defaults.model.clone(), LlmSource::SystemDefault),
    };
    let max_tokens = match over.max_tokens {
        Some(v) => (v, LlmSource::Override),
        None => (defaults.max_tokens, LlmSource::SystemDefault),
    };
    let temperature = match over.temperature {
        Some(v) => (v, LlmSource::Override),
        None => (defaults.temperature, LlmSource::SystemDefault),
    };
    let reasoning_effort = match over.reasoning_effort {
        Some(v) => (v, LlmSource::Override),
        None => (defaults.reasoning_effort, LlmSource::SystemDefault),
    };
    let top_p = (defaults.top_p, LlmSource::SystemDefault);
    EffectiveLlm {
        model,
        max_tokens,
        temperature,
        reasoning_effort,
        top_p,
    }
}

/// Convert an effective config into the env-var bag the supervisor
/// passes to the Python gateway. Lock the `MAKAKOO_LLM_*` namespace
/// so the gateway's reader stays stable across versions.
pub fn effective_to_env(eff: &EffectiveLlm) -> Vec<(String, String)> {
    vec![
        ("MAKAKOO_LLM_MODEL".into(), eff.model.0.clone()),
        ("MAKAKOO_LLM_MAX_TOKENS".into(), eff.max_tokens.0.to_string()),
        ("MAKAKOO_LLM_TEMPERATURE".into(), format!("{}", eff.temperature.0)),
        (
            "MAKAKOO_LLM_REASONING_EFFORT".into(),
            match eff.reasoning_effort.0 {
                ReasoningEffort::Low => "low".into(),
                ReasoningEffort::Medium => "medium".into(),
                ReasoningEffort::High => "high".into(),
            },
        ),
        ("MAKAKOO_LLM_TOP_P".into(), format!("{}", eff.top_p.0)),
    ]
}

// ── Validation ────────────────────────────────────────────────────

/// Validation result from a create-time / validate-time check.
#[derive(Debug)]
pub enum ModelValidation {
    /// Catalog reachable; model id known.
    Known { available: Vec<String> },
    /// Catalog reachable; model id unknown. Caller MUST reject.
    Unknown { available: Vec<String> },
    /// Catalog NOT reachable (network down). Caller treats as warning.
    OfflineWarn { detail: String },
}

/// Trait extracted so tests can substitute a fake list_models
/// without invoking real HTTP.
pub trait ModelCatalog {
    fn list_models(&self) -> Result<Vec<String>, String>;
}

/// Look up `model_id` in the catalog. Network failure → `OfflineWarn`.
pub fn validate_model(catalog: &dyn ModelCatalog, model_id: &str) -> ModelValidation {
    match catalog.list_models() {
        Ok(models) => {
            if models.iter().any(|m| m == model_id) {
                ModelValidation::Known { available: models }
            } else {
                ModelValidation::Unknown { available: models }
            }
        }
        Err(detail) => ModelValidation::OfflineWarn { detail },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> LlmDefaults {
        LlmDefaults {
            model: "ail-compound".into(),
            max_tokens: 4096,
            temperature: 0.5,
            reasoning_effort: ReasoningEffort::Medium,
            top_p: 1.0,
        }
    }

    #[test]
    fn override_empty_returns_all_defaults() {
        let eff = resolve_effective(None, &defaults());
        assert_eq!(eff.model.0, "ail-compound");
        assert_eq!(eff.model.1, LlmSource::SystemDefault);
        assert_eq!(eff.max_tokens, (4096, LlmSource::SystemDefault));
        assert_eq!(eff.temperature, (0.5, LlmSource::SystemDefault));
        assert_eq!(
            eff.reasoning_effort,
            (ReasoningEffort::Medium, LlmSource::SystemDefault)
        );
    }

    #[test]
    fn override_model_only_keeps_other_defaults() {
        let over = LlmOverride {
            model: Some("claude-haiku-4-5".into()),
            ..Default::default()
        };
        let eff = resolve_effective(Some(&over), &defaults());
        assert_eq!(eff.model.0, "claude-haiku-4-5");
        assert_eq!(eff.model.1, LlmSource::Override);
        assert_eq!(eff.max_tokens, (4096, LlmSource::SystemDefault));
        assert_eq!(eff.temperature.1, LlmSource::SystemDefault);
    }

    #[test]
    fn override_all_fields_marks_each_as_override() {
        let over = LlmOverride {
            model: Some("claude-opus-4-7".into()),
            max_tokens: Some(8192),
            temperature: Some(0.3),
            reasoning_effort: Some(ReasoningEffort::High),
        };
        let eff = resolve_effective(Some(&over), &defaults());
        assert_eq!(eff.model.1, LlmSource::Override);
        assert_eq!(eff.max_tokens, (8192, LlmSource::Override));
        assert_eq!(eff.temperature, (0.3, LlmSource::Override));
        assert_eq!(
            eff.reasoning_effort,
            (ReasoningEffort::High, LlmSource::Override)
        );
        // top_p has no override slot — always SystemDefault.
        assert_eq!(eff.top_p.1, LlmSource::SystemDefault);
    }

    #[test]
    fn render_human_attributes_each_field() {
        let over = LlmOverride {
            model: Some("claude-opus-4-7".into()),
            max_tokens: Some(8192),
            ..Default::default()
        };
        let eff = resolve_effective(Some(&over), &defaults());
        let out = eff.render_human();
        assert!(out.contains("model:"));
        assert!(out.contains("claude-opus-4-7"));
        assert!(out.contains("[override]"));
        assert!(out.contains("[system default]"));
        // Both override-marked fields should be tagged.
        let override_count = out.matches("[override]").count();
        let default_count = out.matches("[system default]").count();
        assert_eq!(override_count, 2, "model + max_tokens overridden");
        assert_eq!(default_count, 3, "temperature + reasoning + top_p inherited");
    }

    #[test]
    fn effective_to_env_yields_locked_keys() {
        let over = LlmOverride {
            model: Some("claude-opus-4-7".into()),
            max_tokens: Some(8192),
            temperature: Some(0.3),
            reasoning_effort: Some(ReasoningEffort::High),
        };
        let eff = resolve_effective(Some(&over), &defaults());
        let env = effective_to_env(&eff);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map.get("MAKAKOO_LLM_MODEL").unwrap(), "claude-opus-4-7");
        assert_eq!(map.get("MAKAKOO_LLM_MAX_TOKENS").unwrap(), "8192");
        assert_eq!(map.get("MAKAKOO_LLM_TEMPERATURE").unwrap(), "0.3");
        assert_eq!(map.get("MAKAKOO_LLM_REASONING_EFFORT").unwrap(), "high");
        assert_eq!(map.get("MAKAKOO_LLM_TOP_P").unwrap(), "1");
    }

    #[test]
    fn override_is_empty_check() {
        assert!(LlmOverride::default().is_empty());
        let over = LlmOverride {
            temperature: Some(0.1),
            ..Default::default()
        };
        assert!(!over.is_empty());
    }

    // ── Validation tests ──────────────────────────────────────────

    struct FakeCatalog {
        result: Result<Vec<String>, String>,
    }

    impl ModelCatalog for FakeCatalog {
        fn list_models(&self) -> Result<Vec<String>, String> {
            self.result.clone()
        }
    }

    #[test]
    fn validate_known_model_returns_known() {
        let catalog = FakeCatalog {
            result: Ok(vec!["ail-compound".into(), "claude-opus-4-7".into()]),
        };
        match validate_model(&catalog, "claude-opus-4-7") {
            ModelValidation::Known { available } => {
                assert!(available.contains(&"claude-opus-4-7".to_string()));
            }
            other => panic!("expected Known, got {other:?}"),
        }
    }

    #[test]
    fn validate_unknown_model_returns_unknown_with_available_list() {
        let catalog = FakeCatalog {
            result: Ok(vec!["ail-compound".into()]),
        };
        match validate_model(&catalog, "nonexistent-model") {
            ModelValidation::Unknown { available } => {
                assert_eq!(available, vec!["ail-compound".to_string()]);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn validate_offline_returns_warn_not_error() {
        let catalog = FakeCatalog {
            result: Err("connection refused".into()),
        };
        match validate_model(&catalog, "any-model") {
            ModelValidation::OfflineWarn { detail } => {
                assert!(detail.contains("connection refused"));
            }
            other => panic!("expected OfflineWarn, got {other:?}"),
        }
    }
}
