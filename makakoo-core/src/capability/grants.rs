//! Grant resolver — turn a parsed `Manifest` into a typed `GrantTable`.
//!
//! Spec: `spec/CAPABILITIES.md §2` "grant resolution algorithm."
//!
//! The resolver:
//!   1. Parses each entry in `[capabilities].grants` as a `Verb`.
//!   2. Auto-adds `state/plugin:$MAKAKOO_HOME/state/<name>/` when the
//!      manifest declares a `[state]` table.
//!   3. Auto-adds `infect/contribute` when `[infect.fragments]` is set.
//!   4. Auto-adds `mcp/register:<tool>` for every `[mcp.tools]` entry.
//!   5. Auto-adds `sancho/register:<task>` for every `[sancho.tasks]`.
//!   6. Rejects §6 forbidden patterns that only surface after auto-grants
//!      merge (e.g. the `exec/shell + state.retention="keep"` combo).
//!
//! Runtime `check(verb, requested_scope)` returns `GrantCheck::Allow`
//! with the matched granted scope, or `GrantCheck::Deny` with a reason
//! suitable for the audit log.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use thiserror::Error;

use super::verb::{parse_grant, scope_matches, Verb, VerbError};
use crate::plugin::manifest::{Manifest, StateRetention};

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("grant #{index} ({raw:?}) rejected: {source}")]
    BadGrant {
        index: usize,
        raw: String,
        #[source]
        source: VerbError,
    },
    #[error(
        "plugin {plugin:?} declares both exec/shell and [state].retention = keep; disallowed in v0.1 (§6 rule 4)"
    )]
    ShellWithKeep { plugin: String },
}

/// Resolution of one plugin's capability surface.
#[derive(Debug, Clone, Default)]
pub struct GrantTable {
    pub plugin: String,
    pub plugin_version: String,
    /// verb → list of granted scopes. Empty scope list = unscoped grant.
    verbs: BTreeMap<String, BTreeSet<String>>,
}

impl GrantTable {
    pub fn new(plugin: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            plugin_version: version.into(),
            verbs: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, v: Verb) {
        let entry = self.verbs.entry(v.verb).or_default();
        if v.scopes.is_empty() {
            // Unscoped insert: wipe existing scopes, since unscoped
            // dominates (§6 rule 6 — the narrow ones become redundant).
            entry.clear();
            entry.insert(String::new());
        } else if !entry.contains("") {
            for s in v.scopes {
                entry.insert(s);
            }
        }
    }

    pub fn has_verb(&self, verb: &str) -> bool {
        self.verbs.contains_key(verb)
    }

    pub fn scopes_for(&self, verb: &str) -> Option<Vec<&str>> {
        self.verbs
            .get(verb)
            .map(|set| set.iter().map(|s| s.as_str()).collect())
    }

    /// Machine-readable view suitable for `makakoo plugin info --json`.
    pub fn rows(&self) -> Vec<(String, Vec<String>)> {
        self.verbs
            .iter()
            .map(|(k, v)| {
                let scopes: Vec<String> = v.iter().cloned().collect();
                (k.clone(), scopes)
            })
            .collect()
    }

    /// Check a runtime request. `requested_scope` is the concrete thing
    /// the plugin is asking for (a URL, file path, secret key, etc.).
    pub fn check(&self, verb: &str, requested_scope: &str) -> GrantCheck {
        let Some(set) = self.verbs.get(verb) else {
            return GrantCheck::Deny {
                reason: format!("no grant for verb {verb:?}"),
            };
        };
        // Unscoped grant (empty string in the set) matches any request.
        if set.contains("") {
            return GrantCheck::Allow {
                matched_scope: None,
            };
        }
        for g in set {
            if scope_matches(g, requested_scope) {
                return GrantCheck::Allow {
                    matched_scope: Some(g.clone()),
                };
            }
        }
        GrantCheck::Deny {
            reason: format!(
                "no granted scope matches request {requested_scope:?} for verb {verb:?}"
            ),
        }
    }
}

/// Outcome of a runtime capability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantCheck {
    Allow { matched_scope: Option<String> },
    Deny { reason: String },
}

impl GrantCheck {
    pub fn is_allow(&self) -> bool {
        matches!(self, GrantCheck::Allow { .. })
    }
    pub fn matched_scope(&self) -> Option<&str> {
        match self {
            GrantCheck::Allow { matched_scope } => matched_scope.as_deref(),
            GrantCheck::Deny { .. } => None,
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            GrantCheck::Deny { reason } => Some(reason.as_str()),
            GrantCheck::Allow { .. } => None,
        }
    }
}

/// Parse every entry in `[capabilities].grants`, merge in the §2
/// auto-default grants, and enforce the merge-time forbidden patterns.
///
/// `makakoo_home` is only consumed when the manifest declares a
/// `[state]` table so the default `state/plugin:$MAKAKOO_HOME/state/<name>`
/// grant has a concrete path the audit log can reference.
pub fn resolve_grants(
    manifest: &Manifest,
    makakoo_home: &Path,
) -> Result<GrantTable, ResolveError> {
    let mut table = GrantTable::new(
        manifest.plugin.name.clone(),
        manifest.plugin.version.to_string(),
    );

    // 1) Explicit grants from [capabilities].grants.
    for (i, raw) in manifest.capabilities.grants.iter().enumerate() {
        let v = parse_grant(raw).map_err(|source| ResolveError::BadGrant {
            index: i,
            raw: raw.clone(),
            source,
        })?;
        table.insert(v);
    }

    // 2) Auto-default: state/plugin when [state] is present.
    if let Some(state) = manifest.state.as_ref() {
        let scope = resolve_state_dir_token(&state.dir, makakoo_home, &manifest.plugin.name);
        table.insert(Verb {
            verb: "state/plugin".to_string(),
            scopes: vec![scope],
        });
    }

    // 3) Auto-default: infect/contribute when [infect.fragments] is set.
    if !manifest.infect.fragments.is_empty() {
        table.insert(Verb {
            verb: "infect/contribute".to_string(),
            scopes: vec![],
        });
    }

    // 4) Auto-default: mcp/register per tool.
    for tool in &manifest.mcp.tools {
        table.insert(Verb {
            verb: "mcp/register".to_string(),
            scopes: vec![tool.name.clone()],
        });
    }

    // 5) Auto-default: sancho/register per task.
    for task in &manifest.sancho.tasks {
        table.insert(Verb {
            verb: "sancho/register".to_string(),
            scopes: vec![task.name.clone()],
        });
    }

    // 6) §6 rule 4: exec/shell + [state].retention = keep is disallowed.
    if table.has_verb("exec/shell") {
        let keep = manifest
            .state
            .as_ref()
            .map(|s| matches!(s.retention, StateRetention::Keep))
            .unwrap_or(false);
        if keep {
            return Err(ResolveError::ShellWithKeep {
                plugin: manifest.plugin.name.clone(),
            });
        }
    }

    Ok(table)
}

/// Expand `$MAKAKOO_HOME` placeholders in a state-dir token.
fn resolve_state_dir_token(raw: &str, home: &Path, plugin_name: &str) -> String {
    let home_s = home.to_string_lossy();
    let expanded = raw
        .replace("$MAKAKOO_HOME", &home_s)
        .replace("${MAKAKOO_HOME}", &home_s);
    if expanded.is_empty() {
        // Fallback so the audit log still has something useful.
        format!("{home_s}/state/{plugin_name}")
    } else {
        expanded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;

    fn manifest(body: &str) -> Manifest {
        Manifest::parse(body, &PathBuf::from("t.toml")).unwrap().0
    }

    #[test]
    fn empty_grants_resolves_empty() {
        let m = manifest(
            r#"
[plugin]
name = "plain-plugin"
version = "1.0.0"
kind = "skill"
language = "python"
[source]
path = "."
[abi]
skill = "^1.0"
[entrypoint]
run = "true"
"#,
        );
        let t = resolve_grants(&m, Path::new("/var/makakoo")).unwrap();
        assert!(t.rows().is_empty());
    }

    #[test]
    fn explicit_grants_resolved() {
        let m = manifest(
            r#"
[plugin]
name = "net-plugin"
version = "1.0.0"
kind = "skill"
language = "python"
[source]
path = "."
[abi]
skill = "^1.0"
[entrypoint]
run = "true"
[capabilities]
grants = [
  "brain/read",
  "net/http:https://api.example.com/*",
  "secrets/read:AIL_API_KEY",
]
"#,
        );
        let t = resolve_grants(&m, Path::new("/var/m")).unwrap();
        assert!(t.has_verb("brain/read"));
        assert!(t.has_verb("net/http"));
        assert!(t.has_verb("secrets/read"));

        let check = t.check("net/http", "https://api.example.com/v1/users");
        assert!(check.is_allow());
        assert_eq!(
            check.matched_scope(),
            Some("https://api.example.com/*")
        );

        let denied = t.check("net/http", "https://evil.com/x");
        assert!(!denied.is_allow());
    }

    #[test]
    fn unscoped_grant_matches_any_request() {
        let m = manifest(
            r#"
[plugin]
name = "liberal-plugin"
version = "1.0.0"
kind = "skill"
language = "python"
[source]
path = "."
[abi]
skill = "^1.0"
[entrypoint]
run = "true"
[capabilities]
grants = ["net/http"]
"#,
        );
        let t = resolve_grants(&m, Path::new("/var/m")).unwrap();
        assert!(t.check("net/http", "https://anywhere.example.com").is_allow());
    }

    #[test]
    fn state_table_auto_adds_state_plugin_grant() {
        let m = manifest(
            r#"
[plugin]
name = "stateful-plugin"
version = "1.0.0"
kind = "skill"
language = "python"
[source]
path = "."
[abi]
skill = "^1.0"
[entrypoint]
run = "true"
[state]
dir = "$MAKAKOO_HOME/state/stateful-plugin"
retention = "purge_on_uninstall"
"#,
        );
        let t = resolve_grants(&m, Path::new("/var/m")).unwrap();
        assert!(t.has_verb("state/plugin"));
        let scopes = t.scopes_for("state/plugin").unwrap();
        assert!(scopes
            .iter()
            .any(|s| s.starts_with("/var/m/state/stateful-plugin")));
    }

    #[test]
    fn sancho_tasks_auto_register() {
        let m = manifest(
            r#"
[plugin]
name = "sancho-plugin"
version = "1.0.0"
kind = "sancho-task"
language = "python"
[source]
path = "."
[abi]
sancho-task = "^1.0"
[entrypoint]
run = "true"
[sancho]
tasks = [
  { name = "morning_brief", interval = "1h" },
  { name = "evening_recap", interval = "1h" },
]
"#,
        );
        let t = resolve_grants(&m, Path::new("/var/m")).unwrap();
        let scopes = t.scopes_for("sancho/register").unwrap();
        assert!(scopes.iter().any(|s| *s == "morning_brief"));
        assert!(scopes.iter().any(|s| *s == "evening_recap"));
    }

    #[test]
    fn mcp_tools_auto_register() {
        let m = manifest(
            r#"
[plugin]
name = "mcp-plugin"
version = "1.0.0"
kind = "mcp-tool"
language = "python"
[source]
path = "."
[abi]
mcp-tool = "^1.0"
[entrypoint]
run = "true"
[mcp]
tools = [{ name = "foo_bar", handler = "handlers:foo_bar" }]
"#,
        );
        let t = resolve_grants(&m, Path::new("/var/m")).unwrap();
        let scopes = t.scopes_for("mcp/register").unwrap();
        assert_eq!(scopes, vec!["foo_bar"]);
    }

    #[test]
    fn exec_shell_with_retention_keep_rejected() {
        let m = manifest(
            r#"
[plugin]
name = "shelly-plugin"
version = "1.0.0"
kind = "skill"
language = "python"
[source]
path = "."
[abi]
skill = "^1.0"
[entrypoint]
run = "true"
[capabilities]
grants = ["exec/shell"]
[state]
dir = "$MAKAKOO_HOME/state/shelly-plugin"
retention = "keep"
"#,
        );
        let err = resolve_grants(&m, Path::new("/var/m")).unwrap_err();
        assert!(matches!(err, ResolveError::ShellWithKeep { .. }));
    }

    #[test]
    fn bad_grant_bubbles_up_with_index() {
        // The manifest parser already rejects unknown verbs, so we
        // construct the Manifest struct directly to prove the resolver
        // is a second line of defense when someone bypasses the TOML
        // parser (future: programmatic grants injected by the infect
        // flow).
        let mut m = manifest(
            r#"
[plugin]
name = "bad-plugin"
version = "1.0.0"
kind = "skill"
language = "python"
[source]
path = "."
[abi]
skill = "^1.0"
[entrypoint]
run = "true"
[capabilities]
grants = ["brain/read"]
"#,
        );
        m.capabilities.grants.push("telemetry/weird".into());
        let err = resolve_grants(&m, Path::new("/var/m")).unwrap_err();
        match err {
            ResolveError::BadGrant { index, raw, .. } => {
                assert_eq!(index, 1);
                assert_eq!(raw, "telemetry/weird");
            }
            _ => panic!("expected BadGrant"),
        }
    }

    /// Every shipped `plugins-core/*` manifest must resolve without error —
    /// if someone edits a core plugin and introduces a bad grant, this
    /// test catches it immediately.
    #[test]
    fn shipped_core_plugins_resolve_cleanly() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let core_dir = Path::new(manifest_dir)
            .parent()
            .unwrap()
            .join("plugins-core");
        if !core_dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(core_dir).unwrap() {
            let entry = entry.unwrap();
            let toml = entry.path().join("plugin.toml");
            if !toml.exists() {
                continue;
            }
            let (m, _) = Manifest::load(&toml).unwrap();
            resolve_grants(&m, Path::new("/var/m")).unwrap_or_else(|e| {
                panic!("failed to resolve grants for {}: {e}", toml.display())
            });
        }
    }
}
