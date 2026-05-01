//! Variable substitution for pattern user-message bodies.
//!
//! Replaces `{{name}}` tokens with declared variable values. Literal
//! token replacement only — no expressions, no namespaces, no shell
//! escapes. Brain-aware templating namespaces (`brain`, `garage`, etc.)
//! are explicitly out of v1 scope per SPRINT-PATTERN-SUBSTRATE-V1 §3.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::plugin::manifest::{PatternTable, VariableDecl};

/// Substitution error. Distinct variants for required-missing vs
/// undeclared-token so callers can produce actionable messages.
#[derive(Debug, Error)]
pub enum SubstitutionError {
    #[error("required variable {name:?} was not provided")]
    MissingRequired { name: String },

    /// A `{{token}}` appears in the body that doesn't match any
    /// variable declared in `[pattern].variables`. Catching this at
    /// substitution time prevents silently shipping a leaky placeholder
    /// to the model.
    #[error("template references undeclared variable {name:?}")]
    UndeclaredToken { name: String },
}

/// Resolve every declared variable into a final concrete string. Caller
/// passes user-supplied values via `provided`; `pattern.variables`
/// drives required + default semantics.
pub fn resolve_values(
    pattern: &PatternTable,
    provided: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, SubstitutionError> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for VariableDecl {
        name,
        required,
        default,
        ..
    } in &pattern.variables
    {
        if let Some(v) = provided.get(name) {
            out.insert(name.clone(), v.clone());
        } else if let Some(d) = default {
            out.insert(name.clone(), d.clone());
        } else if *required {
            return Err(SubstitutionError::MissingRequired {
                name: name.clone(),
            });
        } else {
            // Optional variable with no default and no provided value:
            // substitute empty string. Pattern authors who care can
            // mark it required or supply a default.
            out.insert(name.clone(), String::new());
        }
    }
    Ok(out)
}

/// Replace every `{{name}}` token in `body` with the corresponding
/// value from `values`. Tokens referencing names not in `values` raise
/// [`SubstitutionError::UndeclaredToken`].
///
/// Whitespace inside the braces is permitted: `{{ name }}` resolves the
/// same as `{{name}}`.
pub fn substitute(
    body: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, SubstitutionError> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("}}") else {
            // Unterminated `{{` — pass through literally and stop
            // substituting. Don't error — pattern authors may quote
            // braces in their prompts.
            out.push_str("{{");
            out.push_str(after_open);
            return Ok(out);
        };
        let raw = &after_open[..end];
        let name = raw.trim();
        // Names are sanity-checked by the manifest loader against
        // VARIABLE_NAME_RE — but tokens in the body could be typos.
        if !is_simple_name(name) {
            // Not a substitution target (might be `{{ some markdown }}`
            // unrelated). Pass through verbatim.
            out.push_str("{{");
            out.push_str(raw);
            out.push_str("}}");
        } else {
            let Some(value) = values.get(name) else {
                return Err(SubstitutionError::UndeclaredToken {
                    name: name.to_string(),
                });
            };
            out.push_str(value);
        }
        rest = &after_open[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn is_simple_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::manifest::{VariableDecl, VariableKind};

    fn pat(vars: Vec<VariableDecl>) -> PatternTable {
        PatternTable {
            variables: vars,
            ..Default::default()
        }
    }

    fn var(name: &str, required: bool, default: Option<&str>) -> VariableDecl {
        VariableDecl {
            name: name.to_string(),
            description: None,
            kind: VariableKind::String,
            required,
            default: default.map(str::to_string),
        }
    }

    #[test]
    fn substitutes_simple_token() {
        let mut v = BTreeMap::new();
        v.insert("input".to_string(), "world".to_string());
        let out = substitute("Hello {{input}}!", &v).unwrap();
        assert_eq!(out, "Hello world!");
    }

    #[test]
    fn substitutes_multiple_tokens() {
        let mut v = BTreeMap::new();
        v.insert("a".to_string(), "X".to_string());
        v.insert("b".to_string(), "Y".to_string());
        let out = substitute("{{a}}-{{b}}-{{a}}", &v).unwrap();
        assert_eq!(out, "X-Y-X");
    }

    #[test]
    fn whitespace_inside_braces_resolves() {
        let mut v = BTreeMap::new();
        v.insert("input".to_string(), "world".to_string());
        let out = substitute("Hello {{  input  }}!", &v).unwrap();
        assert_eq!(out, "Hello world!");
    }

    #[test]
    fn undeclared_token_errors() {
        let v = BTreeMap::new();
        let err = substitute("Hi {{missing}}", &v).unwrap_err();
        assert!(matches!(
            err,
            SubstitutionError::UndeclaredToken { ref name } if name == "missing"
        ));
    }

    #[test]
    fn unterminated_brace_passes_through() {
        let v = BTreeMap::new();
        let out = substitute("oops {{ no close", &v).unwrap();
        assert_eq!(out, "oops {{ no close");
    }

    #[test]
    fn non_simple_token_passes_through() {
        // `{{ markdown reference }}` is multi-word, not a variable name.
        // Should be left alone.
        let v = BTreeMap::new();
        let out = substitute("see {{ Section 3 }} for details", &v).unwrap();
        assert_eq!(out, "see {{ Section 3 }} for details");
    }

    #[test]
    fn empty_body_returns_empty() {
        let v = BTreeMap::new();
        assert_eq!(substitute("", &v).unwrap(), "");
    }

    #[test]
    fn no_tokens_passes_through() {
        let v = BTreeMap::new();
        assert_eq!(substitute("plain text", &v).unwrap(), "plain text");
    }

    #[test]
    fn resolve_values_uses_provided() {
        let mut p = BTreeMap::new();
        p.insert("input".to_string(), "hello".to_string());
        let pat = pat(vec![var("input", true, None)]);
        let out = resolve_values(&pat, &p).unwrap();
        assert_eq!(out.get("input").unwrap(), "hello");
    }

    #[test]
    fn resolve_values_falls_back_to_default() {
        let p = BTreeMap::new();
        let pat = pat(vec![var("input", false, Some("fallback"))]);
        let out = resolve_values(&pat, &p).unwrap();
        assert_eq!(out.get("input").unwrap(), "fallback");
    }

    #[test]
    fn resolve_values_required_missing_errors() {
        let p = BTreeMap::new();
        let pat = pat(vec![var("input", true, None)]);
        let err = resolve_values(&pat, &p).unwrap_err();
        assert!(matches!(
            err,
            SubstitutionError::MissingRequired { ref name } if name == "input"
        ));
    }

    #[test]
    fn resolve_values_optional_missing_is_empty() {
        let p = BTreeMap::new();
        let pat = pat(vec![var("input", false, None)]);
        let out = resolve_values(&pat, &p).unwrap();
        assert_eq!(out.get("input").unwrap(), "");
    }

    #[test]
    fn is_simple_name_accepts_valid() {
        assert!(is_simple_name("input"));
        assert!(is_simple_name("user_name"));
        assert!(is_simple_name("_private"));
        assert!(is_simple_name("v1"));
    }

    #[test]
    fn is_simple_name_rejects_invalid() {
        assert!(!is_simple_name(""));
        assert!(!is_simple_name("Bad"));
        assert!(!is_simple_name("9start"));
        assert!(!is_simple_name("with-dash"));
        assert!(!is_simple_name("with space"));
    }
}
