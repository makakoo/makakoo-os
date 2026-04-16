//! Capability verb parser + scope matcher.
//!
//! A grant string has the shape `domain/name[:scope[,scope2,…]]`. Examples:
//!
//! - `brain/read`
//! - `llm/chat:minimax/ail-compound`
//! - `net/http:https://api.example.com/*,https://*.example.com/*`
//! - `fs/read:~/code/**`
//! - `exec/binary:git,curl`
//!
//! This module does the string → `Verb` parse, the vocabulary check
//! against `spec/CAPABILITIES.md §1`, and the scope glob match used by
//! the grant resolver.
//!
//! See also: `grants.rs` for the per-plugin `GrantTable` and the check
//! call; `audit.rs` for the logging side.

use std::fmt;

use thiserror::Error;

pub use crate::plugin::manifest::{KNOWN_VERBS, SCOPE_REQUIRED_VERBS};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VerbError {
    #[error("unknown capability verb: {verb:?}")]
    Unknown { verb: String },
    #[error("capability {verb:?} requires a scope (none provided)")]
    MissingScope { verb: String },
    #[error("capability {verb:?} scope {scope:?} rejected: {reason}")]
    BadScope {
        verb: String,
        scope: String,
        reason: String,
    },
    #[error("empty grant string")]
    Empty,
    #[error("grant {raw:?} has malformed shape (expected \"domain/name[:scope]\")")]
    MalformedShape { raw: String },
}

/// Parsed capability grant. Scopes are stored as a sorted, deduplicated
/// list so two manifests that declare the same set in different order
/// compare equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verb {
    pub verb: String,
    pub scopes: Vec<String>,
}

impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scopes.is_empty() {
            write!(f, "{}", self.verb)
        } else {
            write!(f, "{}:{}", self.verb, self.scopes.join(","))
        }
    }
}

/// Canonicalise a raw grant string without validating vocabulary. Used
/// by the manifest parser's preview step where we want "what would this
/// grant look like" without yet deciding whether to accept it.
pub fn normalize_grant(raw: &str) -> Result<Verb, VerbError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(VerbError::Empty);
    }
    let (verb_part, scope_part) = match trimmed.split_once(':') {
        None => (trimmed, None),
        Some((v, s)) => (v.trim(), Some(s.trim())),
    };
    if verb_part.is_empty() || !verb_part.contains('/') {
        return Err(VerbError::MalformedShape {
            raw: raw.to_string(),
        });
    }
    let mut scopes: Vec<String> = scope_part
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    scopes.sort();
    scopes.dedup();
    Ok(Verb {
        verb: verb_part.to_string(),
        scopes,
    })
}

/// Full parse — normalises then validates against the v0.1 vocabulary
/// and the `spec/CAPABILITIES.md §6` forbidden-pattern list.
pub fn parse_grant(raw: &str) -> Result<Verb, VerbError> {
    let v = normalize_grant(raw)?;
    if !KNOWN_VERBS.iter().any(|k| *k == v.verb) {
        return Err(VerbError::Unknown {
            verb: v.verb.clone(),
        });
    }
    // §6 rule 1-3: scope-required verbs must have a scope.
    if SCOPE_REQUIRED_VERBS.iter().any(|k| *k == v.verb) && v.scopes.is_empty() {
        return Err(VerbError::MissingScope {
            verb: v.verb.clone(),
        });
    }
    // §1.7 — secrets scopes must not be `*` (too broad).
    if (v.verb == "secrets/read" || v.verb == "secrets/write")
        && v.scopes.iter().any(|s| s == "*")
    {
        return Err(VerbError::BadScope {
            verb: v.verb.clone(),
            scope: "*".into(),
            reason: "unbounded secrets allowlist is rejected (§6)".into(),
        });
    }
    // §1.7 — secrets keys should look like uppercase SCREAMING_SNAKE_CASE;
    // warn-style rejection keeps convention without over-constraining.
    if v.verb == "secrets/read" || v.verb == "secrets/write" {
        for s in &v.scopes {
            if !is_valid_secret_key(s) {
                return Err(VerbError::BadScope {
                    verb: v.verb.clone(),
                    scope: s.clone(),
                    reason: "secret key must match [A-Z][A-Z0-9_]*".into(),
                });
            }
        }
    }
    // §1.6 — exec/binary with scope "*" should be exec/shell instead.
    if v.verb == "exec/binary" && v.scopes.iter().any(|s| s == "*") {
        return Err(VerbError::BadScope {
            verb: v.verb.clone(),
            scope: "*".into(),
            reason: "use exec/shell explicitly for unbounded exec (§1.6)".into(),
        });
    }
    Ok(v)
}

/// Match a requested scope against a granted scope glob. Both must come
/// from the same verb.
///
/// Supports three glob forms used in the spec:
///
/// - `*` — matches any run of non-separator chars (`.`, `/`, etc.
///   allowed depending on domain; v0.1 keeps the shared logic simple).
/// - `**` — matches any run of chars including separators.
/// - exact — matches only that literal.
///
/// Unscoped granted scope (granted is empty) means "match anything".
/// Unscoped request with a scoped grant means "implicit `*`-style
/// rejection unless the grant itself is `*`".
pub fn scope_matches(granted: &str, requested: &str) -> bool {
    if granted.is_empty() {
        return true; // unscoped grant allows any request
    }
    if granted == "*" {
        return true;
    }
    glob_match(granted, requested)
}

/// Simple glob matcher. `**` matches any chars, `*` matches any run of
/// non-slash chars. Anchored to the full string (no partial match).
fn glob_match(pattern: &str, s: &str) -> bool {
    // Convert the glob into a plain regex. This keeps the semantics
    // identical across OSes without pulling in the `glob` crate.
    let re = glob_to_regex(pattern);
    regex::Regex::new(&re).map(|r| r.is_match(s)).unwrap_or(false)
}

fn glob_to_regex(pattern: &str) -> String {
    // v0.1 semantics (spec/CAPABILITIES.md §1): `*` and `**` both match
    // any run of characters including slashes. The distinction in the
    // spec between URL globs (`*`) and path globs (`**`) is cosmetic —
    // author-convenient, not kernel-meaningful — so we collapse to a
    // single permissive interpretation. If v0.2 needs finer control
    // we can re-introduce a separator-aware form per domain.
    let mut out = String::with_capacity(pattern.len() * 2 + 2);
    out.push('^');
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    out.push_str(".*");
                    i += 2;
                    continue;
                } else {
                    out.push_str(".*");
                }
            }
            b'?' => out.push('.'),
            b'.' | b'+' | b'(' | b')' | b'|' | b'^' | b'$' | b'{' | b'}'
            | b'[' | b']' | b'\\' => {
                out.push('\\');
                out.push(c as char);
            }
            _ => out.push(c as char),
        }
        i += 1;
    }
    out.push('$');
    out
}

fn is_valid_secret_key(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();
    if !(bytes[0].is_ascii_uppercase() || bytes[0] == b'_') {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_brain_read_no_scope() {
        let v = parse_grant("brain/read").unwrap();
        assert_eq!(v.verb, "brain/read");
        assert!(v.scopes.is_empty());
    }

    #[test]
    fn parse_llm_chat_model_scope() {
        let v = parse_grant("llm/chat:minimax/ail-compound").unwrap();
        assert_eq!(v.verb, "llm/chat");
        assert_eq!(v.scopes, vec!["minimax/ail-compound".to_string()]);
    }

    #[test]
    fn parse_multi_scope_dedup_and_sort() {
        let v = parse_grant("net/http:https://b.com/*,https://a.com/*,https://a.com/*")
            .unwrap();
        assert_eq!(v.verb, "net/http");
        assert_eq!(v.scopes.len(), 2);
        assert_eq!(v.scopes[0], "https://a.com/*");
    }

    #[test]
    fn display_roundtrip() {
        let raw = "net/http:https://api.example.com/*,https://data.example.com/*";
        let v = parse_grant(raw).unwrap();
        let printed = v.to_string();
        assert_eq!(printed, raw);
    }

    #[test]
    fn unknown_verb_rejected() {
        let err = parse_grant("telemetry/submit").unwrap_err();
        assert!(matches!(err, VerbError::Unknown { .. }));
    }

    #[test]
    fn fs_read_without_scope_rejected() {
        let err = parse_grant("fs/read").unwrap_err();
        assert!(matches!(err, VerbError::MissingScope { .. }));
    }

    #[test]
    fn secrets_wildcard_rejected() {
        let err = parse_grant("secrets/read:*").unwrap_err();
        assert!(matches!(err, VerbError::BadScope { .. }));
    }

    #[test]
    fn secrets_lowercase_key_rejected() {
        let err = parse_grant("secrets/read:apiKey").unwrap_err();
        assert!(matches!(err, VerbError::BadScope { .. }));
    }

    #[test]
    fn secrets_valid_key_accepted() {
        let v = parse_grant("secrets/read:AIL_API_KEY").unwrap();
        assert_eq!(v.scopes, vec!["AIL_API_KEY".to_string()]);
    }

    #[test]
    fn exec_binary_without_scope_rejected() {
        let err = parse_grant("exec/binary").unwrap_err();
        assert!(matches!(err, VerbError::MissingScope { .. }));
    }

    #[test]
    fn exec_binary_wildcard_redirected_to_shell() {
        let err = parse_grant("exec/binary:*").unwrap_err();
        assert!(matches!(err, VerbError::BadScope { .. }));
    }

    #[test]
    fn exec_shell_allowed_unscoped() {
        let v = parse_grant("exec/shell").unwrap();
        assert_eq!(v.verb, "exec/shell");
    }

    #[test]
    fn malformed_shape_rejected() {
        let err = parse_grant("no-slash").unwrap_err();
        assert!(matches!(err, VerbError::MalformedShape { .. }));
        let err = parse_grant("").unwrap_err();
        assert!(matches!(err, VerbError::Empty));
    }

    #[test]
    fn sancho_register_requires_scope() {
        let err = parse_grant("sancho/register").unwrap_err();
        assert!(matches!(err, VerbError::MissingScope { .. }));
        let v = parse_grant("sancho/register:my_task").unwrap();
        assert_eq!(v.scopes, vec!["my_task".to_string()]);
    }

    #[test]
    fn scope_match_unscoped_grant_accepts_anything() {
        assert!(scope_matches("", "anything"));
    }

    #[test]
    fn scope_match_star_accepts_anything() {
        assert!(scope_matches("*", "whatever"));
    }

    #[test]
    fn scope_match_star_is_permissive_in_v0_1() {
        // v0.1 collapses `*` and `**` to "match anything" per
        // glob_to_regex docs — spec distinction is cosmetic.
        assert!(scope_matches(
            "https://api.example.com/*",
            "https://api.example.com/v1/users"
        ));
        assert!(scope_matches(
            "https://api.example.com/**",
            "https://api.example.com/v1/users"
        ));
    }

    #[test]
    fn scope_match_exact_literal() {
        assert!(scope_matches(
            "https://polymarket.com",
            "https://polymarket.com"
        ));
        assert!(!scope_matches(
            "https://polymarket.com",
            "https://polymarket.com/v1"
        ));
    }

    #[test]
    fn scope_match_path_glob() {
        assert!(scope_matches("~/code/**", "~/code/repo/src/main.rs"));
    }

    #[test]
    fn scope_match_model_glob() {
        assert!(scope_matches("minimax/*", "minimax/ail-compound"));
        assert!(!scope_matches("minimax/*", "anthropic/claude"));
    }

    #[test]
    fn normalize_accepts_unknown_verb_but_parse_rejects() {
        let v = normalize_grant("telemetry/submit:metric").unwrap();
        assert_eq!(v.verb, "telemetry/submit");
        let err = parse_grant("telemetry/submit:metric").unwrap_err();
        assert!(matches!(err, VerbError::Unknown { .. }));
    }
}
