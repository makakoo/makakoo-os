//! Per-agent scope enforcement: tool whitelist + path access.
//!
//! Phase 3 deliverable.  Layered evaluation order locked by Phase
//! 3 criteria:
//!
//!   1. `allowed_paths`  — must contain the candidate (prefix
//!                          match against the canonicalised path)
//!   2. `forbidden_paths` — overrides; veto wins over allow
//!   3. `bound_to_agent`  — runtime grant filtering happens at the
//!                          grant-store layer (see
//!                          `agents::grants` and the
//!                          `garagetytus-grants` crate)
//!
//! Tool dispatch is simpler: the candidate must appear in the
//! slot's `tools` whitelist.  Empty whitelist combined with
//! `inherit_baseline = false` denies all tools (least-privilege
//! per Q6).
//!
//! Both checks return structured error variants so the LLM
//! dispatcher can render a human-friendly response without
//! crashing the gateway loop.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::agents::slot::AgentSlot;

/// Structured scope-violation error.
///
/// Locked Phase-3 contract: each variant carries the slot id, the
/// candidate that was rejected, and the slot's allow/forbid
/// list(s) as raw `Vec<String>` / `Vec<PathBuf>` data.  Display
/// rendering happens at the formatter boundary so callers
/// (gateway → LLM, CLI → operator) can re-shape the message
/// without the lists being baked into a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    ToolNotInScope {
        slot_id: String,
        candidate: String,
        allowed: Vec<String>,
        /// `true` when `allowed` is empty AND the slot has
        /// `inherit_baseline = false` — distinguishes
        /// least-privilege deny from "tool not in this list".
        least_privilege: bool,
    },
    PathNotInScope {
        slot_id: String,
        candidate: PathBuf,
        allowed: Vec<PathBuf>,
        forbidden: Vec<PathBuf>,
        /// `true` when `allowed` is empty (no path is permitted
        /// regardless of the candidate).
        least_privilege: bool,
    },
}

impl std::error::Error for ScopeError {}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeError::ToolNotInScope {
                slot_id,
                candidate,
                allowed,
                least_privilege,
            } => {
                let allowed_repr = if *least_privilege {
                    "(none — least-privilege default)".to_string()
                } else {
                    allowed.join(", ")
                };
                write!(
                    f,
                    "tool '{candidate}' is not in scope for slot '{slot_id}'; allowed: {allowed_repr}"
                )
            }
            ScopeError::PathNotInScope {
                slot_id,
                candidate,
                allowed,
                forbidden,
                least_privilege,
            } => {
                let allowed_repr = if *least_privilege {
                    "(none — least-privilege default)".to_string()
                } else {
                    render_paths(allowed)
                };
                let forbidden_repr = render_paths(forbidden);
                write!(
                    f,
                    "path '{}' is not in scope for slot '{slot_id}'; allowed: {allowed_repr}; forbidden: {forbidden_repr}",
                    candidate.display()
                )
            }
        }
    }
}

/// Check whether `tool` is permitted for the given slot.  Locked
/// semantics:
///
///   - Empty `tools` whitelist + `inherit_baseline = false`
///     → deny everything (least-privilege).
///   - Empty `tools` whitelist + `inherit_baseline = true`
///     → permit any tool the caller passes (callers higher up
///     enforce baseline membership).
///   - Non-empty `tools` whitelist → tool must be a member.
pub fn check_tool(slot: &AgentSlot, tool: &str) -> Result<(), ScopeError> {
    if slot.tools.is_empty() {
        if slot.inherit_baseline {
            return Ok(());
        }
        return Err(ScopeError::ToolNotInScope {
            slot_id: slot.slot_id.clone(),
            candidate: tool.to_string(),
            allowed: Vec::new(),
            least_privilege: true,
        });
    }
    if slot.tools.iter().any(|t| t == tool) {
        return Ok(());
    }
    Err(ScopeError::ToolNotInScope {
        slot_id: slot.slot_id.clone(),
        candidate: tool.to_string(),
        allowed: slot.tools.clone(),
        least_privilege: false,
    })
}

/// Check whether the given path is permitted for the slot.  Used
/// for both read and write enforcement (callers identify the
/// access kind via the error message they render to the LLM).
///
/// Path matching uses prefix comparison after expanding `~/` to
/// the user's home directory.  Both `allowed_paths` and
/// `forbidden_paths` accept either absolute paths or `~/…` shorthand.
pub fn check_path(slot: &AgentSlot, candidate: &Path) -> Result<(), ScopeError> {
    let candidate_canon = canonicalise(candidate);
    let allowed_canon: Vec<PathBuf> =
        slot.allowed_paths.iter().map(|s| canonicalise(Path::new(s))).collect();
    let forbidden_canon: Vec<PathBuf> = slot
        .forbidden_paths
        .iter()
        .map(|s| canonicalise(Path::new(s)))
        .collect();

    // Allow-first: no allowed paths declared = no read/write at all.
    if allowed_canon.is_empty() {
        return Err(ScopeError::PathNotInScope {
            slot_id: slot.slot_id.clone(),
            candidate: candidate_canon.clone(),
            allowed: allowed_canon,
            forbidden: forbidden_canon,
            least_privilege: true,
        });
    }
    let allowed = allowed_canon
        .iter()
        .any(|prefix| candidate_canon.starts_with(prefix));
    if !allowed {
        return Err(ScopeError::PathNotInScope {
            slot_id: slot.slot_id.clone(),
            candidate: candidate_canon.clone(),
            allowed: allowed_canon,
            forbidden: forbidden_canon,
            least_privilege: false,
        });
    }
    // Forbidden override wins over allow.
    let forbidden = forbidden_canon
        .iter()
        .any(|prefix| candidate_canon.starts_with(prefix));
    if forbidden {
        return Err(ScopeError::PathNotInScope {
            slot_id: slot.slot_id.clone(),
            candidate: candidate_canon.clone(),
            allowed: allowed_canon,
            forbidden: forbidden_canon,
            least_privilege: false,
        });
    }
    Ok(())
}

/// Expand `~/` to the user's home dir; otherwise pass through.
fn canonicalise(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
}

fn render_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "(none)".into()
    } else {
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::config::{TelegramConfig, TransportConfig, TransportEntry};

    fn slot_with(
        tools: Vec<&str>,
        allowed_paths: Vec<&str>,
        forbidden_paths: Vec<&str>,
        inherit_baseline: bool,
    ) -> AgentSlot {
        AgentSlot {
            slot_id: "test".into(),
            name: "Test".into(),
            persona: None,
            inherit_baseline,
            allowed_paths: allowed_paths.into_iter().map(String::from).collect(),
            forbidden_paths: forbidden_paths.into_iter().map(String::from).collect(),
            tools: tools.into_iter().map(String::from).collect(),
            process_mode: "supervised_pair".into(),
            transports: vec![TransportEntry {
                id: "t".into(),
                kind: "telegram".into(),
                enabled: true,
                account_id: None,
                secret_ref: None,
                secret_env: None,
                inline_secret_dev: Some("123:abc".into()),
                app_token_ref: None,
                app_token_env: None,
                inline_app_token_dev: None,
                allowed_users: vec!["1".into()],
                config: TransportConfig::Telegram(TelegramConfig::default()),
            }],
        }
    }

    // ── tool checks ────────────────────────────────────────────

    #[test]
    fn tool_in_whitelist_passes() {
        let s = slot_with(vec!["brain_search", "write_file"], vec![], vec![], false);
        check_tool(&s, "brain_search").unwrap();
    }

    #[test]
    fn tool_not_in_whitelist_returns_structured_error() {
        let s = slot_with(vec!["brain_search"], vec![], vec![], false);
        let err = check_tool(&s, "run_command").unwrap_err();
        match err {
            ScopeError::ToolNotInScope {
                slot_id,
                candidate,
                allowed,
                least_privilege,
            } => {
                assert_eq!(slot_id, "test");
                assert_eq!(candidate, "run_command");
                assert_eq!(allowed, vec!["brain_search".to_string()]);
                assert!(!least_privilege);
            }
            _ => panic!("wrong variant"),
        }
        // Display rendering happens at the formatter boundary,
        // not at construction time — verify the message is still
        // human-readable.
        let err = check_tool(&s, "run_command").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("brain_search"));
        assert!(msg.contains("run_command"));
    }

    #[test]
    fn empty_whitelist_with_inherit_baseline_permits_anything() {
        let s = slot_with(vec![], vec![], vec![], true);
        check_tool(&s, "run_command").unwrap();
    }

    #[test]
    fn empty_whitelist_without_inherit_baseline_denies_all() {
        let s = slot_with(vec![], vec![], vec![], false);
        let err = check_tool(&s, "run_command").unwrap_err();
        assert!(format!("{err}").contains("least-privilege"));
    }

    // ── path checks ────────────────────────────────────────────

    #[test]
    fn allowed_path_prefix_match_passes() {
        let s = slot_with(vec![], vec!["/tmp/secretary/"], vec![], false);
        check_path(&s, Path::new("/tmp/secretary/notes.md")).unwrap();
    }

    #[test]
    fn path_outside_allowed_denied() {
        let s = slot_with(vec![], vec!["/tmp/secretary/"], vec![], false);
        let err = check_path(&s, Path::new("/etc/passwd")).unwrap_err();
        match err {
            ScopeError::PathNotInScope {
                slot_id,
                candidate,
                allowed,
                least_privilege,
                ..
            } => {
                assert_eq!(slot_id, "test");
                assert_eq!(candidate, PathBuf::from("/etc/passwd"));
                assert_eq!(allowed, vec![PathBuf::from("/tmp/secretary/")]);
                assert!(!least_privilege);
            }
            _ => panic!("wrong variant"),
        }
        // Verify the Display rendering still mentions the
        // allowed-list contents.
        let err = check_path(&s, Path::new("/etc/passwd")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("/tmp/secretary"));
        assert!(msg.contains("/etc/passwd"));
    }

    #[test]
    fn forbidden_overrides_allowed_on_write() {
        let s = slot_with(
            vec![],
            vec!["/tmp/shared/"],
            vec!["/tmp/shared/secrets/"],
            false,
        );
        // Allowed by /tmp/shared/ but forbidden by /tmp/shared/secrets/
        let err = check_path(&s, Path::new("/tmp/shared/secrets/keys.txt")).unwrap_err();
        match err {
            ScopeError::PathNotInScope { forbidden, .. } => {
                assert_eq!(forbidden, vec![PathBuf::from("/tmp/shared/secrets/")]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn forbidden_overrides_allowed_on_read() {
        // Read uses the same check_path() as write — the spec
        // notes "same layering applied before returning file
        // contents".
        let s = slot_with(
            vec![],
            vec!["/tmp/shared/"],
            vec!["/tmp/shared/private/"],
            false,
        );
        let err = check_path(&s, Path::new("/tmp/shared/private/diary.md")).unwrap_err();
        assert!(matches!(err, ScopeError::PathNotInScope { .. }));
    }

    #[test]
    fn empty_allowed_paths_denies_everything() {
        let s = slot_with(vec![], vec![], vec![], false);
        let err = check_path(&s, Path::new("/anywhere")).unwrap_err();
        assert!(format!("{err}").contains("least-privilege"));
    }

    #[test]
    fn tilde_expansion_works() {
        let s = slot_with(vec![], vec!["~/MAKAKOO/data/secretary/"], vec![], false);
        let home = dirs::home_dir().unwrap();
        let candidate = home.join("MAKAKOO/data/secretary/notes.md");
        check_path(&s, &candidate).unwrap();
    }

    #[test]
    fn tilde_in_forbidden_works_too() {
        let s = slot_with(
            vec![],
            vec!["~/MAKAKOO/"],
            vec!["~/MAKAKOO/secrets/"],
            false,
        );
        let home = dirs::home_dir().unwrap();
        let denied = home.join("MAKAKOO/secrets/x.key");
        assert!(check_path(&s, &denied).is_err());
        let allowed = home.join("MAKAKOO/data/y.md");
        check_path(&s, &allowed).unwrap();
    }
}
