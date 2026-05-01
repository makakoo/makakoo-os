//! Per-pattern model + vendor route resolution.
//!
//! SPRINT-PATTERN-SUBSTRATE-V1 Phase 3 (Locked Decision 5).
//!
//! Resolution order is total:
//!   1. `pattern.toml` declared field
//!   2. CLI flag / MCP arg
//!   3. `FABRIC_MODEL_<NAME>` env var (model only — vendor not envable)
//!   4. Kernel default
//!
//! Rationale: pattern authors capture intent ("this audit pattern needs
//! a 1M-context model"); call sites override per-invocation; env lets
//! Sebastian rebind without editing TOML; kernel default catches the
//! rest.

use crate::plugin::manifest::PatternTable;

/// The kernel's default dispatch model. Matches existing usage at
/// `swarm/gateway.rs::DEFAULT_DISPATCH_MODEL` so per-call dispatch
/// stays consistent with swarm dispatch.
pub const DEFAULT_MODEL: &str = "ail-compound";

/// The kernel's default vendor — switchAILocal per Operating Rule 7.
pub const DEFAULT_VENDOR: &str = "switchailocal";

/// Result of route resolution: model + vendor that the LLM client
/// should target for this dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub model: String,
    pub vendor: String,
}

/// Resolve the dispatch route for a pattern.
///
/// Inputs:
/// - `pattern_name` — used to derive `FABRIC_MODEL_<NAME>` env var key
/// - `pattern` — the pattern table (carries `model`, `vendor` defaults)
/// - `flag_model` / `flag_vendor` — per-call overrides from CLI/MCP
/// - `env_lookup` — function from var name to value (injectable for tests)
pub fn resolve_route<F>(
    pattern_name: &str,
    pattern: &PatternTable,
    flag_model: Option<&str>,
    flag_vendor: Option<&str>,
    env_lookup: F,
) -> ResolvedRoute
where
    F: Fn(&str) -> Option<String>,
{
    let model = flag_model
        .map(str::to_string)
        .or_else(|| pattern.model.clone())
        .or_else(|| env_lookup(&fabric_model_env_var(pattern_name)))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let vendor = flag_vendor
        .map(str::to_string)
        .or_else(|| pattern.vendor.clone())
        .unwrap_or_else(|| DEFAULT_VENDOR.to_string());

    ResolvedRoute { model, vendor }
}

/// Compute the env var key for a pattern. `pattern-summarize` →
/// `FABRIC_MODEL_PATTERN_SUMMARIZE`. Hyphens become underscores;
/// the whole thing is uppercased. Stolen from Fabric for muscle-memory
/// continuity.
pub fn fabric_model_env_var(pattern_name: &str) -> String {
    format!("FABRIC_MODEL_{}", pattern_name.replace('-', "_").to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(model: Option<&str>, vendor: Option<&str>) -> PatternTable {
        PatternTable {
            model: model.map(str::to_string),
            vendor: vendor.map(str::to_string),
            ..Default::default()
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn pattern_toml_wins_over_env_and_default() {
        let r = resolve_route(
            "summarize",
            &pat(Some("from-toml"), None),
            None,
            None,
            |k| {
                if k == "FABRIC_MODEL_SUMMARIZE" {
                    Some("from-env".into())
                } else {
                    None
                }
            },
        );
        assert_eq!(r.model, "from-toml");
    }

    #[test]
    fn flag_wins_over_pattern_toml() {
        let r = resolve_route(
            "summarize",
            &pat(Some("from-toml"), None),
            Some("from-flag"),
            None,
            no_env,
        );
        assert_eq!(r.model, "from-flag");
    }

    #[test]
    fn env_wins_over_kernel_default_when_no_toml_and_no_flag() {
        let r = resolve_route("summarize", &pat(None, None), None, None, |k| {
            if k == "FABRIC_MODEL_SUMMARIZE" {
                Some("from-env".into())
            } else {
                None
            }
        });
        assert_eq!(r.model, "from-env");
    }

    #[test]
    fn falls_back_to_kernel_default() {
        let r = resolve_route("summarize", &pat(None, None), None, None, no_env);
        assert_eq!(r.model, DEFAULT_MODEL);
        assert_eq!(r.vendor, DEFAULT_VENDOR);
    }

    #[test]
    fn vendor_resolution_uses_same_precedence_minus_env() {
        let r = resolve_route(
            "summarize",
            &pat(None, Some("from-toml")),
            None,
            Some("from-flag"),
            no_env,
        );
        assert_eq!(r.vendor, "from-flag");

        let r = resolve_route("summarize", &pat(None, Some("from-toml")), None, None, no_env);
        assert_eq!(r.vendor, "from-toml");
    }

    #[test]
    fn fabric_model_env_var_normalizes_hyphens() {
        assert_eq!(
            fabric_model_env_var("extract-wisdom"),
            "FABRIC_MODEL_EXTRACT_WISDOM"
        );
    }

    #[test]
    fn fabric_model_env_var_uppercases_simple_name() {
        assert_eq!(fabric_model_env_var("foo"), "FABRIC_MODEL_FOO");
    }

    #[test]
    fn fabric_model_env_var_preserves_underscores() {
        // Convention: dirs use hyphens, but be defensive about underscores too.
        assert_eq!(fabric_model_env_var("a_b"), "FABRIC_MODEL_A_B");
    }

    #[test]
    fn full_precedence_chain() {
        // pattern.model = X, no flag, no env → X
        let r = resolve_route("p", &pat(Some("X"), None), None, None, no_env);
        assert_eq!(r.model, "X");

        // pattern.model = X, flag = Y, no env → Y
        let r = resolve_route("p", &pat(Some("X"), None), Some("Y"), None, no_env);
        assert_eq!(r.model, "Y");

        // pattern.model = None, no flag, env Z → Z
        let r = resolve_route("p", &pat(None, None), None, None, |k| {
            if k == "FABRIC_MODEL_P" {
                Some("Z".into())
            } else {
                None
            }
        });
        assert_eq!(r.model, "Z");

        // None, None, None → DEFAULT
        let r = resolve_route("p", &pat(None, None), None, None, no_env);
        assert_eq!(r.model, DEFAULT_MODEL);
    }
}
