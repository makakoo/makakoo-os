//! Per-agent scope enforcement: tool whitelist + path access.
//!
//! Phase 3 deliverable.  Layered evaluation order locked by Phase
//! 3 criteria:
//!
//!   1. `allowed_paths`  — must contain the candidate (prefix
//!      match against the canonicalised path)
//!   2. `forbidden_paths` — overrides; veto wins over allow
//!   3. Runtime grants, when present, are enforced by the caller's
//!      permission layer after this static slot-scope check.
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

use unicode_normalization::UnicodeNormalization;

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
    if slot
        .tools
        .iter()
        .any(|configured| tool_names_match(configured, tool))
    {
        return Ok(());
    }
    Err(ScopeError::ToolNotInScope {
        slot_id: slot.slot_id.clone(),
        candidate: tool.to_string(),
        allowed: slot.tools.clone(),
        least_privilege: false,
    })
}

/// AgentSpec historically accepted raw Makakoo names (`brain_search`) while
/// MCP clients expose the same tool as `mcp__harvey__brain_search`. Treat the
/// two spellings as one identity at the enforcement boundary.
fn tool_names_match(configured: &str, candidate: &str) -> bool {
    const PREFIX: &str = "mcp__harvey__";
    configured == candidate
        || configured.strip_prefix(PREFIX) == Some(candidate)
        || candidate.strip_prefix(PREFIX) == Some(configured)
}

/// Check whether the given path is permitted for the slot.  Used
/// for both read and write enforcement (callers identify the
/// access kind via the error message they render to the LLM).
///
/// Path matching uses prefix comparison after expanding `~/` to
/// the user's home directory.  Both `allowed_paths` and
/// `forbidden_paths` accept either absolute paths or `~/…` shorthand.
pub fn check_path(slot: &AgentSlot, candidate: &Path) -> Result<(), ScopeError> {
    let candidate_canon = canonicalise(candidate).unwrap_or_else(|| lexical_rooted(candidate));
    let allowed_canon: Vec<PathBuf> = slot
        .allowed_paths
        .iter()
        .filter_map(|s| canonicalise_scope_prefix(s))
        .collect();
    let forbidden_canon: Vec<PathBuf> = slot
        .forbidden_paths
        .iter()
        .filter_map(|s| canonicalise_scope_prefix(s))
        .collect();

    // A candidate whose nearest on-disk ancestor is a dangling symlink cannot
    // be proven to stay inside scope. Fail closed instead of lexically walking
    // through the symlink.
    if canonicalise(candidate).is_none() {
        return Err(ScopeError::PathNotInScope {
            slot_id: slot.slot_id.clone(),
            candidate: candidate_canon,
            allowed: allowed_canon,
            forbidden: forbidden_canon,
            least_privilege: slot.allowed_paths.is_empty(),
        });
    }

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
        .any(|prefix| path_starts_with(&candidate_canon, prefix));
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
        .any(|prefix| path_starts_with(&candidate_canon, prefix));
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

fn canonicalise_scope_prefix(configured: &str) -> Option<PathBuf> {
    let prefix = configured
        .strip_suffix("/**")
        .or_else(|| configured.strip_suffix("/*"))
        .unwrap_or(configured);
    canonicalise(Path::new(prefix))
}

/// Resolve a path the way scope enforcement does.
///
/// Exported so that anything authorising a write — `check_path` here, the
/// `write_file` handler in makakoo-mcp — resolves the candidate identically.
/// Two resolvers that disagree by one `..` or one symlink is precisely how a
/// sandbox gets bypassed, so there is exactly one.
pub fn resolve_scope_path(path: &Path) -> Option<PathBuf> {
    canonicalise(path)
}

/// Expand `~/`, resolve symlinks through the nearest existing ancestor, and
/// lexically collapse `.`/`..` for not-yet-created write targets.
/// Returns `None` when resolution encounters a dangling symlink.
fn canonicalise(p: &Path) -> Option<PathBuf> {
    let expanded = expand_and_root(p);
    if let Ok(path) = expanded.canonicalize() {
        return Some(path);
    }

    let mut cursor = expanded.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(cursor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() && cursor.canonicalize().is_err() {
                    // Existing symlink whose own target cannot be resolved is
                    // dangling. Resolvable ancestors such as macOS `/tmp` are
                    // safe and canonicalized below.
                    return None;
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name() else {
                    break;
                };
                missing.push(name.to_os_string());
                let Some(parent) = cursor.parent() else {
                    break;
                };
                cursor = parent;
            }
            Err(_) => return None,
        }
    }
    let mut resolved = cursor.canonicalize().ok()?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Some(lexical_normalize(&resolved))
}

fn lexical_rooted(p: &Path) -> PathBuf {
    lexical_normalize(&expand_and_root(p))
}

/// Component-aware path containment with filesystem identity normalization.
///
/// APFS stores decomposed and precomposed Unicode spellings as the same name,
/// and its default format is case-insensitive. `Path::starts_with` compares raw
/// bytes, so using it here would let a not-yet-created NFD/case alias evade a
/// configured prefix. Normalize Unicode on every platform and fold case on
/// platforms whose standard filesystems fold it. Invalid Unix path bytes stay
/// byte-distinct instead of being collapsed through lossy UTF-8 conversion.
fn path_starts_with(candidate: &Path, prefix: &Path) -> bool {
    let candidate = path_identity_components(candidate);
    let prefix = path_identity_components(prefix);
    candidate.starts_with(&prefix)
}

fn path_identity_components(path: &Path) -> Vec<Vec<u8>> {
    path.components()
        .map(|component| component_identity(component.as_os_str()))
        .collect()
}

#[cfg(unix)]
fn component_identity(component: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    match std::str::from_utf8(component.as_bytes()) {
        Ok(text) => normalized_text_identity(text),
        Err(_) => {
            let mut identity = b"raw:\0".to_vec();
            identity.extend_from_slice(component.as_bytes());
            identity
        }
    }
}

#[cfg(not(unix))]
fn component_identity(component: &std::ffi::OsStr) -> Vec<u8> {
    normalized_text_identity(&component.to_string_lossy())
}

fn normalized_text_identity(text: &str) -> Vec<u8> {
    let nfc: String = text.nfc().collect();
    let normalized = if cfg!(any(target_os = "macos", windows)) {
        // Unicode lowercase is deliberately conservative: collisions only
        // over-deny. It closes the ASCII and Unicode case aliases accepted by
        // the default APFS/NTFS path lookup without weakening Linux semantics.
        nfc.chars().flat_map(char::to_lowercase).collect()
    } else {
        nfc
    };
    let mut identity = b"utf8:\0".to_vec();
    identity.extend_from_slice(normalized.as_bytes());
    identity
}

fn expand_and_root(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(rest)
        } else {
            p.to_path_buf()
        }
    } else if p.is_absolute() {
        p.to_path_buf()
    } else {
        crate::platform::makakoo_home().join(p)
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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
            llm: None,
            runtime: None,
        }
    }

    // ── tool checks ────────────────────────────────────────────

    #[test]
    fn tool_in_whitelist_passes() {
        let s = slot_with(vec!["brain_search", "write_file"], vec![], vec![], false);
        check_tool(&s, "brain_search").unwrap();
    }

    #[test]
    fn raw_and_mcp_qualified_tool_names_are_equivalent() {
        let raw = slot_with(vec!["brain_search"], vec![], vec![], false);
        check_tool(&raw, "mcp__harvey__brain_search").unwrap();

        let qualified = slot_with(vec!["mcp__harvey__brain_search"], vec![], vec![], false);
        check_tool(&qualified, "brain_search").unwrap();
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
                assert_eq!(candidate, canonicalise(Path::new("/etc/passwd")).unwrap());
                assert_eq!(
                    allowed,
                    vec![canonicalise(Path::new("/tmp/secretary/")).unwrap()]
                );
                assert!(!least_privilege);
            }
            _ => panic!("wrong variant"),
        }
        // Verify the Display rendering still mentions the
        // allowed-list contents. Separators are normalized because
        // Windows renders canonicalized paths with backslashes.
        let err = check_path(&s, Path::new("/etc/passwd")).unwrap_err();
        let msg = format!("{err}").replace('\\', "/");
        assert!(msg.contains("tmp/secretary"));
        assert!(msg.contains("etc/passwd"));
    }

    #[test]
    fn parent_traversal_cannot_escape_allowed_prefix() {
        let s = slot_with(vec![], vec!["/tmp/secretary"], vec![], false);
        assert!(check_path(&s, Path::new("/tmp/secretary/../outside.txt")).is_err());
    }

    #[test]
    fn documented_recursive_glob_is_treated_as_scope_prefix() {
        let s = slot_with(vec![], vec!["/tmp/secretary/**"], vec![], false);
        check_path(&s, Path::new("/tmp/secretary/nested/notes.md")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cannot_escape_allowed_prefix() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), "x").unwrap();
        symlink(&outside, allowed.join("escape")).unwrap();
        let allowed_text = allowed.to_string_lossy().into_owned();
        let s = slot_with(vec![], vec![&allowed_text], vec![], false);
        assert!(check_path(&s, &allowed.join("escape/secret")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_candidate_fails_closed() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        symlink(tmp.path().join("missing-target"), allowed.join("dangling")).unwrap();
        let allowed_text = allowed.to_string_lossy().into_owned();
        let s = slot_with(vec![], vec![&allowed_text], vec![], false);
        assert!(check_path(&s, &allowed.join("dangling/new-file")).is_err());
    }

    #[test]
    fn relative_paths_are_rooted_at_makakoo_home_not_process_cwd() {
        let home = crate::platform::makakoo_home();
        let allowed = home.join("data/agents/test");
        let s = slot_with(vec![], vec!["data/agents/test"], vec![], false);
        check_path(&s, &allowed.join("notes.md")).unwrap();
        assert!(check_path(&s, Path::new("../outside.md")).is_err());
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
                assert_eq!(
                    forbidden,
                    vec![canonicalise(Path::new("/tmp/shared/secrets/")).unwrap()]
                );
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
    fn unicode_normalization_alias_cannot_bypass_forbidden_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().to_string_lossy().into_owned();
        let forbidden = tmp.path().join("s\u{e9}crets");
        let forbidden_text = forbidden.to_string_lossy().into_owned();
        let candidate = tmp.path().join("se\u{301}crets/key.txt");
        let s = slot_with(vec![], vec![&allowed], vec![&forbidden_text], false);

        assert!(!forbidden.exists(), "test requires a missing scope prefix");
        assert!(check_path(&s, &candidate).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn case_alias_cannot_bypass_missing_forbidden_prefix_on_apfs() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().to_string_lossy().into_owned();
        let forbidden = tmp.path().join("Secrets");
        let forbidden_text = forbidden.to_string_lossy().into_owned();
        let candidate = tmp.path().join("sECRETS/key.txt");
        let s = slot_with(vec![], vec![&allowed], vec![&forbidden_text], false);

        assert!(!forbidden.exists(), "test requires a missing scope prefix");
        assert!(check_path(&s, &candidate).is_err());
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
