//! Restricted/headless distro linter.
//!
//! Resolves a distro file's full include chain (and excludes), loads every
//! referenced plugin's `plugin.toml` (failing closed on missing or invalid
//! manifests), and fails on any forbidden capability or tool class:
//!
//! 1. `exec/shell`, `exec/binary`, `language = "shell"`, or `[install]` scripts
//! 2. `secrets/read` (and `secrets/write`)
//! 3. MCP registration (`mcp/register`, `[mcp].tools`, `kind = "mcp-tool"`, `[abi].mcp-tool`)
//! 4. `brain/delete`
//! 5. Any `net/*` grant that is unscoped, wildcarded, or outside `allowed_net`
//! 6. Send tools (outbound/messaging/channel plugin family or send-like MCP tools)

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::capability::verb::{normalize_grant, scope_matches};
use crate::plugin::manifest::{Manifest, PluginKind, PluginLanguage};

use super::file::{DistroError, DistroFile};
use super::resolver::{resolve_distro, DistroResolverError};

#[cfg(test)]
#[path = "lint_tests.rs"]
mod tests;

#[derive(Debug, Error)]
pub enum DistroLintError {
    #[error("distro file error: {0}")]
    File(#[from] DistroError),
    #[error("distro resolver error: {0}")]
    Resolver(#[from] DistroResolverError),
}

/// Machine-readable distro lint rule identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistroLintRule {
    MissingManifest,
    InvalidManifest,
    ExecShell,
    ExecBinary,
    InstallScript,
    SecretRead,
    SecretWrite,
    McpRegistration,
    BrainDelete,
    UnlistedNetwork,
    SendTool,
}

impl DistroLintRule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingManifest => "missing_manifest",
            Self::InvalidManifest => "invalid_manifest",
            Self::ExecShell => "exec_shell",
            Self::ExecBinary => "exec_binary",
            Self::InstallScript => "install_script",
            Self::SecretRead => "secret_read",
            Self::SecretWrite => "secret_write",
            Self::McpRegistration => "mcp_registration",
            Self::BrainDelete => "brain_delete",
            Self::UnlistedNetwork => "unlisted_network",
            Self::SendTool => "send_tool",
        }
    }
}

/// One policy violation found while linting a distro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintViolation {
    pub plugin: String,
    pub rule: DistroLintRule,
    pub detail: String,
}

/// Options governing distro linting.
#[derive(Debug, Clone)]
pub struct LintOptions {
    /// Directory containing `<plugin-name>/plugin.toml` manifests.
    pub plugins_root: PathBuf,
    /// Explicitly allowlisted network scope globs (e.g. `http://127.0.0.1:18080/*`).
    /// Empty by default (every `net/*` grant fails).
    pub allowed_net: Vec<String>,
}

/// Complete report emitted by [`lint_distro_file`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistroLintReport {
    pub distro: String,
    pub ok: bool,
    pub checked_plugins: usize,
    pub violations: Vec<LintViolation>,
}

const SEND_PLUGIN_TOKENS: &[&str] = &[
    "outbound",
    "messaging",
    "telegram",
    "whatsapp",
    "slack",
    "discord",
    "email",
    "mail",
    "smtp",
    "sms",
    "send",
    "broadcast",
    "harveychat",
    "channel",
];

const SEND_MCP_TOOL_SUBSTRINGS: &[&str] = &[
    "send",
    "broadcast",
    "outbound",
    "email",
    "sms",
    "telegram",
    "slack",
    "whatsapp",
    "post_",
    "_post",
    "_dm",
];

/// Lint a distro TOML file and its resolved include chain against the strict
/// headless capability policy.
pub fn lint_distro_file(
    distro_path: &Path,
    opts: &LintOptions,
) -> Result<DistroLintReport, DistroLintError> {
    let root = DistroFile::load(distro_path)?;
    let resolved = resolve_distro(&root, distro_path)?;

    let mut queue: VecDeque<String> = resolved.plugins.iter().map(|p| p.name.clone()).collect();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut violations: Vec<LintViolation> = Vec::new();

    while let Some(plugin_name) = queue.pop_front() {
        if !visited.insert(plugin_name.clone()) {
            continue;
        }
        let manifest_path = opts.plugins_root.join(&plugin_name).join("plugin.toml");
        if !manifest_path.is_file() {
            violations.push(LintViolation {
                plugin: plugin_name,
                rule: DistroLintRule::MissingManifest,
                detail: format!("plugin manifest not found at {}", manifest_path.display()),
            });
            continue;
        }

        let manifest = match Manifest::load(&manifest_path) {
            Ok((m, _warnings)) => m,
            Err(e) => {
                violations.push(LintViolation {
                    plugin: plugin_name,
                    rule: DistroLintRule::InvalidManifest,
                    detail: format!("failed to parse manifest: {e}"),
                });
                continue;
            }
        };

        // Also lint transitive plugin dependencies so forbidden capabilities
        // cannot be smuggled through `[depends].plugins`.
        for dep in &manifest.depends.plugins {
            if let Some(dep_name) = dep.split_whitespace().next() {
                if !dep_name.is_empty() && !visited.contains(dep_name) {
                    queue.push_back(dep_name.to_string());
                }
            }
        }

        lint_manifest(&plugin_name, &manifest, &opts.allowed_net, &mut violations);
    }

    Ok(DistroLintReport {
        distro: resolved.root.distro.name,
        ok: violations.is_empty(),
        checked_plugins: visited.len(),
        violations,
    })
}

/// Lint one loaded plugin manifest against all strict headless lint rules.
pub fn lint_manifest(
    plugin_name: &str,
    manifest: &Manifest,
    allowed_net: &[String],
    violations: &mut Vec<LintViolation>,
) {
    let mut push = |rule: DistroLintRule, detail: String| {
        violations.push(LintViolation {
            plugin: plugin_name.to_string(),
            rule,
            detail,
        });
    };

    if manifest.plugin.language == PluginLanguage::Shell {
        push(
            DistroLintRule::ExecShell,
            "plugin declares language = \"shell\"".into(),
        );
    }
    if let Some(ref script) = manifest.install.unix {
        push(
            DistroLintRule::InstallScript,
            format!("declares [install].unix = {script:?}"),
        );
    }
    if let Some(ref script) = manifest.install.windows {
        push(
            DistroLintRule::InstallScript,
            format!("declares [install].windows = {script:?}"),
        );
    }

    if manifest.plugin.kind == PluginKind::McpTool {
        push(
            DistroLintRule::McpRegistration,
            "plugin declares kind = \"mcp-tool\"".into(),
        );
    }
    if manifest.abi.mcp_tool.is_some() {
        push(
            DistroLintRule::McpRegistration,
            "plugin declares [abi].mcp-tool".into(),
        );
    }
    for tool in &manifest.mcp.tools {
        push(
            DistroLintRule::McpRegistration,
            format!("plugin registers MCP tool {:?}", tool.name),
        );
        if is_send_mcp_tool_name(&tool.name) {
            push(
                DistroLintRule::SendTool,
                format!("MCP tool {:?} matches forbidden send-tool class", tool.name),
            );
        }
    }

    if let Some(reason) = send_plugin_reason(plugin_name, &manifest.plugin.keywords) {
        push(DistroLintRule::SendTool, reason);
    }

    for raw_grant in &manifest.capabilities.grants {
        let Ok(parsed) = normalize_grant(raw_grant) else {
            continue;
        };
        match parsed.verb.as_str() {
            "exec/shell" => push(
                DistroLintRule::ExecShell,
                format!("declares forbidden capability {raw_grant:?}"),
            ),
            "exec/binary" => push(
                DistroLintRule::ExecBinary,
                format!("declares forbidden capability {raw_grant:?}"),
            ),
            "secrets/read" => push(
                DistroLintRule::SecretRead,
                format!("declares forbidden capability {raw_grant:?}"),
            ),
            "secrets/write" => push(
                DistroLintRule::SecretWrite,
                format!("declares forbidden capability {raw_grant:?}"),
            ),
            "mcp/register" => push(
                DistroLintRule::McpRegistration,
                format!("declares forbidden capability {raw_grant:?}"),
            ),
            "brain/delete" => push(
                DistroLintRule::BrainDelete,
                format!("declares forbidden capability {raw_grant:?}"),
            ),
            verb if verb.starts_with("net/") => {
                if parsed.scopes.is_empty() {
                    push(
                        DistroLintRule::UnlistedNetwork,
                        format!("unscoped network grant {raw_grant:?} is forbidden"),
                    );
                } else {
                    for scope in &parsed.scopes {
                        if !is_network_scope_allowlisted(scope, allowed_net) {
                            push(
                                DistroLintRule::UnlistedNetwork,
                                format!(
                                    "network grant {verb}:{scope} is not in the allowed network list"
                                ),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_network_scope_allowlisted(scope: &str, allowed_net: &[String]) -> bool {
    let s = scope.trim();
    if s.is_empty() || s == "*" || s == "**" || s.starts_with("*://") || s.contains("..") {
        return false;
    }
    allowed_net.iter().any(|allowed| {
        let a = allowed.trim();
        if a.is_empty() || a == "*" || a == "**" {
            return false;
        }
        a == s || scope_matches(a, s)
    })
}

fn send_plugin_reason(plugin_name: &str, keywords: &[String]) -> Option<String> {
    let lower_name = plugin_name.to_ascii_lowercase();
    for tok in lower_name.split(['-', '_']) {
        if SEND_PLUGIN_TOKENS.contains(&tok) {
            return Some(format!(
                "plugin name {plugin_name:?} belongs to forbidden outbound/messaging family ({tok:?})"
            ));
        }
    }
    for kw in keywords {
        let lower_kw = kw.to_ascii_lowercase();
        if SEND_PLUGIN_TOKENS.contains(&lower_kw.as_str()) {
            return Some(format!(
                "plugin keyword {kw:?} belongs to forbidden outbound/messaging family"
            ));
        }
    }
    None
}

fn is_send_mcp_tool_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "dm"
        || lower == "post"
        || SEND_MCP_TOOL_SUBSTRINGS
            .iter()
            .any(|needle| lower.contains(needle))
}
