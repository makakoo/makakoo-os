//! Pure prompt composition.
//!
//! Composes `strategy ⊕ mascot ⊕ pattern` into a single OpenAI-style
//! `system` message, plus interpolates user variables into the
//! pattern's `system.md` body to produce the `user` message body.
//!
//! Decoupled from CLI parsing and MCP wiring so SPRINT-PATTERN-SUBSTRATE-V1
//! Phase 5 can reuse it as-is.

use std::collections::BTreeMap;

use thiserror::Error;

use super::variables::{resolve_values, substitute, SubstitutionError};
use crate::plugin::manifest::PatternTable;

/// Inputs needed to compose a single dispatch. All strings are already
/// resolved — strategy/mascot lookups happen in the calling layer.
#[derive(Debug, Clone)]
pub struct ComposeRequest<'a> {
    /// The pattern's [pattern] table — drives variables, defaults, tags.
    pub pattern: &'a PatternTable,
    /// The contents of the pattern's sibling `system.md`.
    pub system_md: &'a str,
    /// Resolved strategy text (already loaded). `None` skips the axis.
    pub strategy_text: Option<&'a str>,
    /// Resolved mascot persona text (already loaded). `None` skips the axis.
    pub mascot_text: Option<&'a str>,
    /// User-provided variable values keyed by variable name.
    pub provided_vars: &'a BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedPrompt {
    /// Final system message — concatenation of strategy + mascot +
    /// pattern's system.md, separated by blank lines. Axes that were
    /// `None` in the request contribute nothing.
    pub system: String,
    /// Final user message — pattern's system.md with `{{name}}` tokens
    /// substituted from resolved variables. Wait — this is wrong, see
    /// note below: `system_md` lives in the system role, not the user
    /// role. The `user` body is just the canonical `input` variable
    /// (or empty) for callers that want to construct a chat. For now
    /// we expose both: callers can decide whether to pass the
    /// substituted body as the system prompt + a separate user turn,
    /// or fold input into the system prompt and send an empty user
    /// turn. Pattern authors choose by where they place `{{input}}`.
    pub user: String,
}

#[derive(Debug, Error)]
pub enum ComposeError {
    #[error(transparent)]
    Substitution(#[from] SubstitutionError),
}

/// Compose a [`ComposedPrompt`] from a request.
///
/// The system message is built as `strategy + mascot + pattern.system_md`,
/// joined by blank lines, with all `{{var}}` tokens substituted using
/// resolved variable values. Empty axes are skipped cleanly — no extra
/// blank lines.
///
/// The user message is the value of the canonical `input` variable
/// when it exists, otherwise empty. Patterns whose body already
/// references `{{input}}` get the value substituted into the system
/// prompt; the user turn stays empty (saves a roundtrip on stateless
/// dispatch). Patterns that omit `{{input}}` from their body get the
/// raw input as a fresh user message — handy for Q&A-style patterns.
pub fn compose(req: &ComposeRequest) -> Result<ComposedPrompt, ComposeError> {
    let values = resolve_values(req.pattern, req.provided_vars)?;
    let body = substitute(req.system_md, &values)?;

    let mut parts: Vec<&str> = Vec::with_capacity(3);
    if let Some(s) = req.strategy_text {
        let s = s.trim();
        if !s.is_empty() {
            parts.push(s);
        }
    }
    if let Some(m) = req.mascot_text {
        let m = m.trim();
        if !m.is_empty() {
            parts.push(m);
        }
    }
    let body_trimmed = body.trim();
    if !body_trimmed.is_empty() {
        parts.push(body_trimmed);
    }

    let system = parts.join("\n\n");

    // If the body includes `{{input}}`, the user already has the input
    // folded in via substitution — leave the user turn empty. If not,
    // forward whatever is in `input` as the user turn.
    let user = if req.system_md.contains("{{input}}") || req.system_md.contains("{{ input }}") {
        String::new()
    } else {
        values.get("input").cloned().unwrap_or_default()
    };

    Ok(ComposedPrompt { system, user })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifest::{PatternTable, VariableDecl, VariableKind};

    fn pat_with_input() -> PatternTable {
        PatternTable {
            variables: vec![VariableDecl {
                name: "input".into(),
                description: None,
                kind: VariableKind::String,
                required: true,
                default: None,
            }],
            ..Default::default()
        }
    }

    fn provided(k: &str, v: &str) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(k.into(), v.into());
        m
    }

    #[test]
    fn composes_pattern_only() {
        let pat = pat_with_input();
        let provided = provided("input", "hello world");
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "Summarize: {{input}}",
            strategy_text: None,
            mascot_text: None,
            provided_vars: &provided,
        };
        let out = compose(&req).unwrap();
        assert_eq!(out.system, "Summarize: hello world");
        // Body folded `{{input}}` in, so user turn is empty.
        assert_eq!(out.user, "");
    }

    #[test]
    fn composes_strategy_plus_pattern() {
        let pat = pat_with_input();
        let provided = provided("input", "topic");
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "About: {{input}}",
            strategy_text: Some("THINK STEP BY STEP."),
            mascot_text: None,
            provided_vars: &provided,
        };
        let out = compose(&req).unwrap();
        assert_eq!(out.system, "THINK STEP BY STEP.\n\nAbout: topic");
    }

    #[test]
    fn composes_full_stack() {
        let pat = pat_with_input();
        let provided = provided("input", "X");
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "Pattern body: {{input}}",
            strategy_text: Some("Strategy."),
            mascot_text: Some("Mascot."),
            provided_vars: &provided,
        };
        let out = compose(&req).unwrap();
        assert_eq!(out.system, "Strategy.\n\nMascot.\n\nPattern body: X");
    }

    #[test]
    fn empty_axes_omitted_cleanly() {
        let pat = pat_with_input();
        let provided = provided("input", "X");
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "Body {{input}}",
            strategy_text: Some("   \n  \n"), // whitespace-only
            mascot_text: Some(""),
            provided_vars: &provided,
        };
        let out = compose(&req).unwrap();
        // Whitespace-only axes don't add blank lines.
        assert_eq!(out.system, "Body X");
    }

    #[test]
    fn body_without_input_token_routes_input_to_user_turn() {
        let pat = pat_with_input();
        let provided = provided("input", "the question");
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "You are a helpful Q&A assistant.",
            strategy_text: None,
            mascot_text: None,
            provided_vars: &provided,
        };
        let out = compose(&req).unwrap();
        assert_eq!(out.system, "You are a helpful Q&A assistant.");
        assert_eq!(out.user, "the question");
    }

    #[test]
    fn missing_required_var_propagates() {
        let pat = pat_with_input();
        let provided = BTreeMap::new();
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "x {{input}}",
            strategy_text: None,
            mascot_text: None,
            provided_vars: &provided,
        };
        let err = compose(&req).unwrap_err();
        assert!(matches!(
            err,
            ComposeError::Substitution(SubstitutionError::MissingRequired { ref name }) if name == "input"
        ));
    }

    #[test]
    fn undeclared_token_propagates() {
        // Pattern declares 'input' but body references {{ghost}} too.
        let pat = pat_with_input();
        let provided = provided("input", "ok");
        let req = ComposeRequest {
            pattern: &pat,
            system_md: "{{input}} {{ghost}}",
            strategy_text: None,
            mascot_text: None,
            provided_vars: &provided,
        };
        let err = compose(&req).unwrap_err();
        assert!(matches!(
            err,
            ComposeError::Substitution(SubstitutionError::UndeclaredToken { ref name }) if name == "ghost"
        ));
    }
}
