//! `plugin.toml` parser + validator.
//!
//! Source of truth: `spec/PLUGIN_MANIFEST.md` v0.1. Every validation rule
//! in §17 of that spec that can be enforced at manifest parse time is
//! enforced here. Cross-plugin rules (14-16) live in `registry.rs`,
//! dependency + ABI rules (7-10) live in `resolver.rs`, and install-time
//! rules (5, 6, 11) live in `staging.rs`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use thiserror::Error;

static PLUGIN_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z][a-z0-9-]{1,62}$").expect("valid regex"));

/// Every capability verb in the v0.1 vocabulary (spec/CAPABILITIES.md §1).
/// Kept as a sorted slice so we can binary-search on validation.
pub const KNOWN_VERBS: &[&str] = &[
    "brain/delete",
    "brain/query",
    "brain/read",
    "brain/write",
    "exec/binary",
    "exec/shell",
    "fs/read",
    "fs/write",
    "infect/contribute",
    "llm/chat",
    "llm/embed",
    "llm/omni",
    "mcp/register",
    "net/http",
    "net/tcp",
    "net/udp",
    "net/ws",
    "perms/grant",
    "perms/revoke",
    "sancho/register",
    "secrets/read",
    "secrets/write",
    "state/global",
    "state/plugin",
];

/// Verbs that require a non-empty scope (spec/CAPABILITIES.md §1.5–1.8).
pub const SCOPE_REQUIRED_VERBS: &[&str] = &[
    "exec/binary",
    "fs/read",
    "fs/write",
    "mcp/register",
    "perms/grant",
    "sancho/register",
    "secrets/read",
    "secrets/write",
];

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse TOML in {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid manifest {path}: {msg}")]
    Invalid { path: PathBuf, msg: String },
}

impl ManifestError {
    fn invalid(path: &Path, msg: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.to_path_buf(),
            msg: msg.into(),
        }
    }
}

/// Primary role of a plugin. Exactly one per manifest.
///
/// Note: `kind = "openai-compatible" | "subprocess" | "mcp-stdio"` are
/// **adapter** kinds (see `adapter::manifest::AdapterKind`), not plugin
/// kinds. They live in a parallel manifest schema for the universal
/// bridge — adapters wrap external CLIs, plugins are first-class
/// Makakoo citizens. Don't fold them in here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginKind {
    Skill,
    Agent,
    SanchoTask,
    McpTool,
    Mascot,
    BootstrapFragment,
    Library,
    /// Long-lived OS-level daemon supervised via `[service]` table.
    /// Lifecycle hooks (`start`/`stop`/`health`) come from `[entrypoint]`;
    /// `[service]` adds restart policy + health interval + optional
    /// HTTP health endpoint. Driven by `makakoo plugin start|stop|status|restart`.
    Service,
}

impl PluginKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginKind::Skill => "skill",
            PluginKind::Agent => "agent",
            PluginKind::SanchoTask => "sancho-task",
            PluginKind::McpTool => "mcp-tool",
            PluginKind::Mascot => "mascot",
            PluginKind::BootstrapFragment => "bootstrap-fragment",
            PluginKind::Library => "library",
            PluginKind::Service => "service",
        }
    }
}

/// Implementation language — drives install + runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginLanguage {
    Python,
    Rust,
    Node,
    Shell,
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRetention {
    Keep,
    PurgeOnUninstall,
}

impl Default for StateRetention {
    fn default() -> Self {
        StateRetention::Keep
    }
}

/// `[plugin]` identity table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginTable {
    pub name: String,
    pub version: Version,
    pub kind: PluginKind,
    pub language: PluginLanguage,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// `[source]` table — exactly one of git/tar/path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTable {
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub tar: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub blake3: Option<String>,
}

/// `[abi]` table — which ABIs this plugin targets. All fields optional
/// because plugins only declare the ABIs they actually implement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbiTable {
    #[serde(default)]
    pub skill: Option<VersionReq>,
    #[serde(default, rename = "agent")]
    pub agent: Option<VersionReq>,
    #[serde(default, rename = "sancho-task")]
    pub sancho_task: Option<VersionReq>,
    #[serde(default, rename = "mcp-tool")]
    pub mcp_tool: Option<VersionReq>,
    #[serde(default)]
    pub mascot: Option<VersionReq>,
    #[serde(default, rename = "bootstrap-fragment")]
    pub bootstrap_fragment: Option<VersionReq>,
}

impl AbiTable {
    pub fn is_empty(&self) -> bool {
        self.skill.is_none()
            && self.agent.is_none()
            && self.sancho_task.is_none()
            && self.mcp_tool.is_none()
            && self.mascot.is_none()
            && self.bootstrap_fragment.is_none()
    }
}

/// `[depends]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependsTable {
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub python: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub rust: Option<String>,
    #[serde(default)]
    pub binaries: Vec<String>,
    #[serde(default)]
    pub system: Vec<String>,
    #[serde(default)]
    pub packages: DependsPackagesTable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependsPackagesTable {
    #[serde(default)]
    pub python: Vec<String>,
    #[serde(default)]
    pub node: Vec<String>,
    #[serde(default)]
    pub rust: Vec<String>,
}

/// `[install]` dual-shell scripts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTable {
    #[serde(default)]
    pub unix: Option<String>,
    #[serde(default)]
    pub windows: Option<String>,
}

/// `[entrypoint]` table. Which subkeys are required depends on the plugin
/// kind — see `Manifest::validate`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrypointTable {
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub stop: Option<String>,
    #[serde(default)]
    pub health: Option<String>,
}

/// `[capabilities]` grants list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesTable {
    #[serde(default)]
    pub grants: Vec<String>,
}

/// Entry in `[sancho].tasks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SanchoTaskSpec {
    pub name: String,
    pub interval: String,
    #[serde(default)]
    pub active_hours: Option<[u8; 2]>,
    #[serde(default)]
    pub weekdays: Vec<String>,
    #[serde(default)]
    pub gates: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SanchoTable {
    #[serde(default)]
    pub tasks: Vec<SanchoTaskSpec>,
}

/// Entry in `[mcp].tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpToolSpec {
    pub name: String,
    pub handler: String,
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpTable {
    #[serde(default)]
    pub tools: Vec<McpToolSpec>,
}

/// `[infect.fragments]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfectTable {
    #[serde(default)]
    pub fragments: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MascotTable {
    pub species: String,
    #[serde(default)]
    pub stats: std::collections::BTreeMap<String, i64>,
    #[serde(default)]
    pub patrol: Option<String>,
    #[serde(default)]
    pub flavor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateTable {
    pub dir: String,
    #[serde(default)]
    pub retention: StateRetention,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestTable {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub timeout: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingTable {
    pub model: String,
    pub dim: u32,
    #[serde(default)]
    pub provider: Option<String>,
}

/// Restart behavior for `kind = "service"` plugins. Driven by the
/// service runner when a started service exits before `stop` is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Always restart, regardless of exit code.
    Always,
    /// Restart only on non-zero exit. Default for service plugins.
    OnFailure,
    /// Never restart automatically — supervisor is external (e.g. launchd).
    Never,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy::OnFailure
    }
}

fn default_health_interval_sec() -> u32 {
    60
}

/// `[service]` table — only meaningful when `plugin.kind = "service"`.
/// Lifecycle commands come from `[entrypoint]` (start/stop/health).
/// `[service]` overlays daemon-specific behavior on top.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceTable {
    /// Optional override for `[entrypoint].start`. When absent, the
    /// runner falls back to the entrypoint command.
    #[serde(default)]
    pub start_cmd: Option<String>,
    /// Optional override for `[entrypoint].stop`.
    #[serde(default)]
    pub stop_cmd: Option<String>,
    /// HTTP URL or shell command. Strings starting with `http://` /
    /// `https://` are probed via GET (200/204 = healthy); everything
    /// else is shelled out via `/bin/sh -c` (exit 0 = healthy).
    /// When absent, the runner falls back to `[entrypoint].health`.
    #[serde(default)]
    pub health_endpoint: Option<String>,
    /// Seconds between health probes when the runner is supervising.
    /// Manual `makakoo plugin status` ignores this — it always probes once.
    #[serde(default = "default_health_interval_sec")]
    pub health_interval_sec: u32,
    /// What the runner does when a started service exits before `stop`.
    #[serde(default)]
    pub restart_policy: RestartPolicy,
}

impl Default for ServiceTable {
    fn default() -> Self {
        Self {
            start_cmd: None,
            stop_cmd: None,
            health_endpoint: None,
            health_interval_sec: default_health_interval_sec(),
            restart_policy: RestartPolicy::default(),
        }
    }
}

/// The parsed + validated `plugin.toml`.
///
/// Note: no `deny_unknown_fields` at this level — unknown *top-level*
/// tables are a forward-compat warning (spec §0 principle 4), not an
/// error. `deny_unknown_fields` is applied on every *inner* table so
/// typos inside known tables do fail loudly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub plugin: PluginTable,
    pub source: SourceTable,
    #[serde(default)]
    pub abi: AbiTable,
    #[serde(default)]
    pub depends: DependsTable,
    #[serde(default)]
    pub install: InstallTable,
    #[serde(default)]
    pub entrypoint: EntrypointTable,
    #[serde(default)]
    pub capabilities: CapabilitiesTable,
    #[serde(default)]
    pub sancho: SanchoTable,
    #[serde(default)]
    pub mcp: McpTable,
    #[serde(default)]
    pub infect: InfectTable,
    #[serde(default)]
    pub mascot: Option<MascotTable>,
    #[serde(default)]
    pub state: Option<StateTable>,
    #[serde(default)]
    pub test: TestTable,
    #[serde(default)]
    pub embedding: Option<EmbeddingTable>,
    #[serde(default)]
    pub service: Option<ServiceTable>,
}

/// Known top-level tables. Anything else triggers a forward-compat warning.
const KNOWN_TOP_LEVEL: &[&str] = &[
    "plugin",
    "source",
    "abi",
    "depends",
    "install",
    "entrypoint",
    "capabilities",
    "sancho",
    "mcp",
    "infect",
    "mascot",
    "state",
    "test",
    "embedding",
    "service",
];

/// Warnings collected during a single parse. Non-fatal — the caller decides
/// how to surface them (CLI prints them, daemon logs them).
#[derive(Debug, Default, Clone)]
pub struct ParseWarnings(pub Vec<String>);

impl ParseWarnings {
    pub fn push(&mut self, msg: impl Into<String>) {
        self.0.push(msg.into());
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Manifest {
    /// Load + parse + validate a `plugin.toml` file. Returns the manifest
    /// and any non-fatal warnings.
    pub fn load(path: &Path) -> Result<(Self, ParseWarnings), ManifestError> {
        let bytes = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&bytes, path)
    }

    /// Parse + validate an in-memory `plugin.toml` body. `origin` is used
    /// for error messages only — no I/O happens here.
    pub fn parse(body: &str, origin: &Path) -> Result<(Self, ParseWarnings), ManifestError> {
        let mut warnings = ParseWarnings::default();

        // Step 1: parse untyped so we can detect unknown top-level tables
        // (warning, rule 17 + forward-compat note).
        let raw: toml::Value = toml::from_str(body).map_err(|source| ManifestError::Toml {
            path: origin.to_path_buf(),
            source,
        })?;
        let table = raw.as_table().ok_or_else(|| {
            ManifestError::invalid(origin, "top-level must be a TOML table")
        })?;
        for key in table.keys() {
            if !KNOWN_TOP_LEVEL.contains(&key.as_str()) {
                warnings.push(format!(
                    "unknown top-level table [{key}] — ignored (forward-compat)"
                ));
            }
        }

        // Step 2: typed parse (deny_unknown_fields catches rule 17 inside
        // known tables).
        let manifest: Manifest =
            toml::from_str(body).map_err(|source| ManifestError::Toml {
                path: origin.to_path_buf(),
                source,
            })?;

        // Step 3: cross-field validation.
        manifest.validate(origin, &mut warnings)?;

        Ok((manifest, warnings))
    }

    /// Run every manifest-level validation rule from PLUGIN_MANIFEST.md §17
    /// that does not require cross-plugin or runtime context.
    fn validate(&self, path: &Path, warnings: &mut ParseWarnings) -> Result<(), ManifestError> {
        // Rule 3: name regex.
        if !PLUGIN_NAME_RE.is_match(&self.plugin.name) {
            return Err(ManifestError::invalid(
                path,
                format!(
                    "plugin.name {:?} fails regex ^[a-z][a-z0-9-]{{1,62}}$",
                    self.plugin.name
                ),
            ));
        }

        // Rule 4: [source] has exactly one of git/tar/path.
        let source_variants = [
            self.source.git.is_some(),
            self.source.tar.is_some(),
            self.source.path.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if source_variants != 1 {
            return Err(ManifestError::invalid(
                path,
                format!(
                    "[source] must have exactly one of git/tar/path (found {source_variants})"
                ),
            ));
        }

        // Rule 7 (partial): [abi] must not be empty — except for library
        // plugins (importable code, no callable ABI surface) and service
        // plugins (just a long-lived daemon, no callable API).
        if self.abi.is_empty()
            && self.plugin.kind != PluginKind::Library
            && self.plugin.kind != PluginKind::BootstrapFragment
            && self.plugin.kind != PluginKind::Service
        {
            return Err(ManifestError::invalid(
                path,
                "[abi] must declare at least one ABI (e.g. skill = \"^0.1\")",
            ));
        }

        // Rule 12: entrypoint required subkeys per kind.
        self.validate_entrypoint(path)?;

        // Rule 13: capability verbs must be known and correctly scoped.
        for grant in &self.capabilities.grants {
            validate_grant(grant, path)?;
        }

        // Kind-specific extras.
        match self.plugin.kind {
            PluginKind::Mascot => {
                if self.mascot.is_none() {
                    return Err(ManifestError::invalid(
                        path,
                        "[mascot] table required when plugin.kind = mascot",
                    ));
                }
            }
            PluginKind::BootstrapFragment => {
                if self.infect.fragments.is_empty() {
                    return Err(ManifestError::invalid(
                        path,
                        "kind = bootstrap-fragment requires [infect.fragments]",
                    ));
                }
            }
            _ => {}
        }

        // [service] table only meaningful for service-kind plugins.
        if self.service.is_some() && self.plugin.kind != PluginKind::Service {
            warnings.push(format!(
                "[service] table declared but plugin.kind = {} — table will be ignored",
                self.plugin.kind.as_str()
            ));
        }

        // Reserved-prefix warning (rule 17 warnings section).
        if self.plugin.name.starts_with("core-") || self.plugin.name.starts_with("official-") {
            warnings.push(format!(
                "plugin name {:?} uses a reserved prefix — only the makakoo-os monorepo should use this",
                self.plugin.name
            ));
        }

        // Moving-ref warning for git branches.
        if self.source.git.is_some() {
            if let Some(rev) = &self.source.rev {
                if !looks_like_tag_or_sha(rev) {
                    warnings.push(format!(
                        "[source].rev = {rev:?} looks like a branch — tracking a moving ref"
                    ));
                }
            }
        }

        // Duplicate sancho task names within THIS manifest. Cross-plugin
        // uniqueness is checked in registry.rs.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for t in &self.sancho.tasks {
            if !seen.insert(t.name.as_str()) {
                return Err(ManifestError::invalid(
                    path,
                    format!("duplicate [sancho].tasks.name {:?}", t.name),
                ));
            }
        }
        let mut seen_mcp: BTreeSet<&str> = BTreeSet::new();
        for t in &self.mcp.tools {
            if !seen_mcp.insert(t.name.as_str()) {
                return Err(ManifestError::invalid(
                    path,
                    format!("duplicate [mcp].tools.name {:?}", t.name),
                ));
            }
        }

        Ok(())
    }

    fn validate_entrypoint(&self, path: &Path) -> Result<(), ManifestError> {
        match self.plugin.kind {
            PluginKind::Skill | PluginKind::SanchoTask => {
                if self.entrypoint.run.is_none() {
                    return Err(ManifestError::invalid(
                        path,
                        format!(
                            "[entrypoint].run required for kind = {}",
                            self.plugin.kind.as_str()
                        ),
                    ));
                }
            }
            PluginKind::Agent => {
                for (field, val) in [
                    ("start", &self.entrypoint.start),
                    ("stop", &self.entrypoint.stop),
                    ("health", &self.entrypoint.health),
                ] {
                    if val.is_none() {
                        return Err(ManifestError::invalid(
                            path,
                            format!("[entrypoint].{field} required for kind = agent"),
                        ));
                    }
                }
            }
            PluginKind::Service => {
                // Service plugins draw start/stop/health from either
                // [entrypoint] or [service]. Missing in both is an error.
                let svc = self.service.as_ref();
                let has_start = self.entrypoint.start.is_some()
                    || svc.and_then(|s| s.start_cmd.as_ref()).is_some();
                let has_stop = self.entrypoint.stop.is_some()
                    || svc.and_then(|s| s.stop_cmd.as_ref()).is_some();
                let has_health = self.entrypoint.health.is_some()
                    || svc.and_then(|s| s.health_endpoint.as_ref()).is_some();
                if !has_start {
                    return Err(ManifestError::invalid(
                        path,
                        "kind = service requires [entrypoint].start or [service].start_cmd",
                    ));
                }
                if !has_stop {
                    return Err(ManifestError::invalid(
                        path,
                        "kind = service requires [entrypoint].stop or [service].stop_cmd",
                    ));
                }
                if !has_health {
                    return Err(ManifestError::invalid(
                        path,
                        "kind = service requires [entrypoint].health or [service].health_endpoint",
                    ));
                }
            }
            PluginKind::Mascot => {
                // Patrol is declared in [mascot].patrol, no [entrypoint].
            }
            PluginKind::McpTool | PluginKind::BootstrapFragment => {
                // These kinds declare entrypoints in their kind-specific tables.
            }
            PluginKind::Library => {
                // Library plugins provide importable code, not runnable entrypoints.
            }
        }
        Ok(())
    }
}

/// Split a grant string into `(verb, Option<scope>)`.
fn split_grant(grant: &str) -> (&str, Option<&str>) {
    match grant.find(':') {
        Some(idx) => (&grant[..idx], Some(&grant[idx + 1..])),
        None => (grant, None),
    }
}

fn validate_grant(grant: &str, path: &Path) -> Result<(), ManifestError> {
    let (verb, scope) = split_grant(grant);
    if !KNOWN_VERBS.contains(&verb) {
        return Err(ManifestError::invalid(
            path,
            format!("unknown capability verb {verb:?} in grant {grant:?}"),
        ));
    }
    // Scope-required verbs must carry a non-empty scope.
    if SCOPE_REQUIRED_VERBS.contains(&verb) {
        match scope {
            None => {
                return Err(ManifestError::invalid(
                    path,
                    format!("verb {verb:?} requires a scope (e.g. {verb}:something)"),
                ));
            }
            Some(s) if s.trim().is_empty() => {
                return Err(ManifestError::invalid(
                    path,
                    format!("verb {verb:?} has empty scope"),
                ));
            }
            _ => {}
        }
    }
    // Explicit rejection: secrets/read:* is too broad (spec §1.7).
    if verb == "secrets/read" || verb == "secrets/write" {
        if scope == Some("*") {
            return Err(ManifestError::invalid(
                path,
                format!("verb {verb:?} with scope '*' rejected: too broad"),
            ));
        }
    }
    Ok(())
}

/// Heuristic — anything that starts with `v` followed by a digit, or is a
/// 7-40 char hex string, is considered a pinned ref. Everything else looks
/// like a branch. Deliberately loose — this only drives a warning.
fn looks_like_tag_or_sha(rev: &str) -> bool {
    if rev.starts_with('v') && rev.len() > 1 && rev.as_bytes()[1].is_ascii_digit() {
        return true;
    }
    let looks_hex = rev.len() >= 7
        && rev.len() <= 40
        && rev.chars().all(|c| c.is_ascii_hexdigit());
    looks_hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p() -> PathBuf {
        PathBuf::from("plugin.toml")
    }

    fn minimal() -> String {
        r#"
[plugin]
name = "example"
version = "0.1.0"
kind = "skill"
language = "python"

[source]
path = "local/example"

[abi]
skill = "^1.0"

[entrypoint]
run = ".venv/bin/python -m example"
"#
        .to_string()
    }

    #[test]
    fn parses_minimal_skill() {
        let (m, w) = Manifest::parse(&minimal(), &p()).unwrap();
        assert_eq!(m.plugin.name, "example");
        assert_eq!(m.plugin.kind, PluginKind::Skill);
        assert_eq!(m.plugin.version, Version::new(0, 1, 0));
        assert!(w.is_empty());
    }

    #[test]
    fn rejects_bad_name() {
        let body = minimal().replace("\"example\"", "\"Bad_Name\"");
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("regex"));
    }

    #[test]
    fn rejects_multiple_source_variants() {
        let body = minimal().replace(
            "[source]\npath = \"local/example\"",
            "[source]\npath = \"local/example\"\ngit = \"https://example.com/x\"",
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("exactly one of git/tar/path"));
    }

    #[test]
    fn rejects_zero_source_variants() {
        let body = minimal().replace(
            "[source]\npath = \"local/example\"",
            "[source]",
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("exactly one of git/tar/path"));
    }

    #[test]
    fn rejects_empty_abi() {
        let body = minimal().replace("[abi]\nskill = \"^1.0\"", "[abi]");
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("[abi]"));
    }

    #[test]
    fn agent_requires_start_stop_health() {
        // Drop the skill entrypoint, bump kind to agent, leave entrypoint empty.
        let body = r#"
[plugin]
name = "agent-x"
version = "0.1.0"
kind = "agent"
language = "python"

[source]
path = "local/x"

[abi]
agent = "^1.0"

[entrypoint]
start = ".venv/bin/python -m x --start"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        assert!(format!("{err}").contains("stop"));
    }

    #[test]
    fn skill_requires_entrypoint_run() {
        let body = minimal().replace(
            "[entrypoint]\nrun = \".venv/bin/python -m example\"",
            "[entrypoint]",
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("run"));
    }

    #[test]
    fn rejects_unknown_verb() {
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"brain/supreme\"]\n",
            minimal()
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("unknown capability verb"));
    }

    #[test]
    fn accepts_unscoped_optional_verb() {
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"brain/read\", \"llm/chat\"]\n",
            minimal()
        );
        let (_m, _w) = Manifest::parse(&body, &p()).unwrap();
    }

    #[test]
    fn rejects_unscoped_scope_required_verb() {
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"secrets/read\"]\n",
            minimal()
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("requires a scope"));
    }

    #[test]
    fn known_verbs_sorted() {
        // Binary-search helpers rely on KNOWN_VERBS being lexicographically
        // sorted. If you add a verb, keep the slice sorted.
        for pair in KNOWN_VERBS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "KNOWN_VERBS not sorted: {:?} >= {:?}",
                pair[0],
                pair[1]
            );
        }
        for pair in SCOPE_REQUIRED_VERBS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "SCOPE_REQUIRED_VERBS not sorted: {:?} >= {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn scope_required_verbs_subset_of_known() {
        for v in SCOPE_REQUIRED_VERBS {
            assert!(
                KNOWN_VERBS.contains(v),
                "SCOPE_REQUIRED_VERBS has {v:?} but KNOWN_VERBS does not"
            );
        }
    }

    #[test]
    fn accepts_perms_grant_with_scope() {
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"perms/grant:~/code/**\"]\n",
            minimal()
        );
        let (_m, _w) = Manifest::parse(&body, &p()).unwrap();
    }

    #[test]
    fn rejects_perms_grant_without_scope() {
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"perms/grant\"]\n",
            minimal()
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("requires a scope"));
    }

    #[test]
    fn accepts_perms_revoke_unscoped() {
        // perms/revoke is revoke-by-id — no scope required.
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"perms/revoke\"]\n",
            minimal()
        );
        let (_m, _w) = Manifest::parse(&body, &p()).unwrap();
    }

    #[test]
    fn rejects_secrets_wildcard() {
        let body = format!(
            "{}\n[capabilities]\ngrants = [\"secrets/read:*\"]\n",
            minimal()
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("too broad"));
    }

    #[test]
    fn warns_on_unknown_top_level_table() {
        let body = format!("{}\n[telemetry]\nkey = \"value\"\n", minimal());
        let (_m, w) = Manifest::parse(&body, &p()).unwrap();
        assert!(w.0.iter().any(|w| w.contains("[telemetry]")));
    }

    #[test]
    fn rejects_unknown_field_in_known_table() {
        let body = minimal().replace(
            "[plugin]",
            "[plugin]\nmysterious = \"value\"",
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("mysterious") || s.contains("unknown field"));
    }

    #[test]
    fn warns_on_reserved_prefix() {
        let body = minimal().replace("\"example\"", "\"core-example\"");
        let (_m, w) = Manifest::parse(&body, &p()).unwrap();
        assert!(w.0.iter().any(|w| w.contains("reserved prefix")));
    }

    #[test]
    fn warns_on_branch_rev() {
        let body = r#"
[plugin]
name = "example"
version = "0.1.0"
kind = "skill"
language = "python"

[source]
git = "https://example.com/x"
rev = "main"

[abi]
skill = "^1.0"

[entrypoint]
run = ".venv/bin/python -m example"
"#;
        let (_m, w) = Manifest::parse(body, &p()).unwrap();
        assert!(w.0.iter().any(|w| w.contains("moving ref")));
    }

    #[test]
    fn duplicate_sancho_task_within_manifest_rejected() {
        let body = format!(
            "{}\n[sancho]\ntasks = [{{ name = \"x\", interval = \"5m\" }}, {{ name = \"x\", interval = \"10m\" }}]\n",
            minimal()
        );
        let err = Manifest::parse(&body, &p()).unwrap_err();
        assert!(format!("{err}").contains("duplicate"));
    }

    #[test]
    fn mascot_kind_requires_mascot_table() {
        let body = r#"
[plugin]
name = "mascot-a"
version = "0.1.0"
kind = "mascot"
language = "python"

[source]
path = "local/mascot"

[abi]
mascot = "^1.0"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        assert!(format!("{err}").contains("[mascot]"));
    }

    #[test]
    fn parses_minimal_service_kind() {
        // Service kind with only [entrypoint] — no [service] table.
        // Validates the "service can borrow start/stop/health from
        // [entrypoint]" path.
        let body = r#"
[plugin]
name = "garage-store"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/garage"

[entrypoint]
start = "/usr/local/opt/garage/bin/garage server"
stop = "/usr/bin/pkill -f 'garage server'"
health = "/usr/local/opt/garage/bin/garage status"
"#;
        let (m, w) = Manifest::parse(body, &p()).unwrap();
        assert_eq!(m.plugin.kind, PluginKind::Service);
        assert!(m.service.is_none());
        assert!(w.is_empty());
    }

    #[test]
    fn parses_service_with_table() {
        let body = r#"
[plugin]
name = "garage-store"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/garage"

[service]
start_cmd = "garage server --bind 127.0.0.1:3900"
stop_cmd = "pkill -f garage"
health_endpoint = "http://127.0.0.1:3903/health"
health_interval_sec = 30
restart_policy = "always"
"#;
        let (m, w) = Manifest::parse(body, &p()).unwrap();
        assert_eq!(m.plugin.kind, PluginKind::Service);
        let svc = m.service.expect("[service] table parsed");
        assert_eq!(
            svc.start_cmd.as_deref(),
            Some("garage server --bind 127.0.0.1:3900")
        );
        assert_eq!(
            svc.health_endpoint.as_deref(),
            Some("http://127.0.0.1:3903/health")
        );
        assert_eq!(svc.health_interval_sec, 30);
        assert_eq!(svc.restart_policy, RestartPolicy::Always);
        assert!(w.is_empty());
    }

    #[test]
    fn service_default_health_interval_is_60() {
        let body = r#"
[plugin]
name = "svc-test"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/svc"

[service]
start_cmd = "x start"
stop_cmd = "x stop"
health_endpoint = "x check"
"#;
        let (m, _) = Manifest::parse(body, &p()).unwrap();
        let svc = m.service.unwrap();
        assert_eq!(svc.health_interval_sec, 60);
        assert_eq!(svc.restart_policy, RestartPolicy::OnFailure);
    }

    #[test]
    fn service_kind_requires_start() {
        let body = r#"
[plugin]
name = "svc-test"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/svc"

[entrypoint]
stop = "x stop"
health = "x check"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("kind = service"));
        assert!(s.contains("start_cmd") || s.contains("entrypoint].start"));
    }

    #[test]
    fn service_kind_requires_stop() {
        let body = r#"
[plugin]
name = "svc-test"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/svc"

[service]
start_cmd = "x start"
health_endpoint = "x check"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        assert!(format!("{err}").contains("stop"));
    }

    #[test]
    fn service_kind_requires_health() {
        let body = r#"
[plugin]
name = "svc-test"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/svc"

[service]
start_cmd = "x start"
stop_cmd = "x stop"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        assert!(format!("{err}").contains("health"));
    }

    #[test]
    fn service_table_warns_if_kind_mismatch() {
        let body = r#"
[plugin]
name = "skill-test"
version = "0.1.0"
kind = "skill"
language = "python"

[source]
path = "local/skill"

[abi]
skill = "^1.0"

[entrypoint]
run = ".venv/bin/python -m skill"

[service]
start_cmd = "skill start"
stop_cmd = "skill stop"
health_endpoint = "skill check"
"#;
        let (_m, w) = Manifest::parse(body, &p()).unwrap();
        assert!(
            w.0.iter().any(|s| s.contains("[service] table") && s.contains("kind = skill")),
            "expected mismatched-kind warning, got {:?}",
            w.0
        );
    }

    #[test]
    fn service_kind_does_not_require_abi() {
        // Services (like libraries) don't expose a callable ABI surface.
        let body = r#"
[plugin]
name = "svc-test"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/svc"

[entrypoint]
start = "x start"
stop = "x stop"
health = "x check"
"#;
        let (m, _) = Manifest::parse(body, &p()).unwrap();
        assert_eq!(m.plugin.kind, PluginKind::Service);
    }

    #[test]
    fn rejects_unknown_restart_policy() {
        let body = r#"
[plugin]
name = "svc-test"
version = "0.1.0"
kind = "service"
language = "shell"

[source]
path = "local/svc"

[service]
start_cmd = "x"
stop_cmd = "x"
health_endpoint = "x"
restart_policy = "sometimes"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("restart_policy") || s.contains("variant"));
    }

    #[test]
    fn bootstrap_fragment_requires_fragments() {
        let body = r#"
[plugin]
name = "frag-a"
version = "0.1.0"
kind = "bootstrap-fragment"
language = "shell"

[source]
path = "local/f"

[abi]
bootstrap-fragment = "^1.0"
"#;
        let err = Manifest::parse(body, &p()).unwrap_err();
        assert!(format!("{err}").contains("infect.fragments"));
    }

    #[test]
    fn looks_like_tag_or_sha_detects_tags_and_shas() {
        assert!(looks_like_tag_or_sha("v0.3.1"));
        assert!(looks_like_tag_or_sha("abcdef1234"));
        assert!(looks_like_tag_or_sha("abcdef1")); // 7 hex chars
        assert!(!looks_like_tag_or_sha("main"));
        assert!(!looks_like_tag_or_sha("develop"));
        assert!(!looks_like_tag_or_sha("v"));
    }

    #[test]
    fn parses_full_agent_example() {
        // The §16.2 worked example from PLUGIN_MANIFEST.md.
        let body = r#"
[plugin]
name = "agent-arbitrage"
version = "0.3.1"
kind = "agent"
language = "python"
summary = "Polymarket BTC momentum trading agent"
authors = ["Sebastian Schkudlara <seb@traylinx.com>"]
license = "MIT"

[source]
git = "https://github.com/traylinx/makakoo-arbitrage"
rev = "v0.3.1"
blake3 = "abcd1234"

[abi]
agent = "^1.0"
sancho-task = "^1.0"
mcp-tool = "^1.0"

[depends]
plugins = ["brain ^1.0", "llm ^1.0"]
python = ">=3.11"

[depends.packages]
python = ["ccxt>=4.0", "numpy>=1.24"]

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
start = ".venv/bin/python -m arbitrage.main --start"
stop = ".venv/bin/python -m arbitrage.main --stop"
health = ".venv/bin/python -m arbitrage.main --health"

[capabilities]
grants = [
  "brain/read", "brain/write",
  "llm/chat:minimax/ail-compound",
  "net/http:https://clob.polymarket.com/*",
  "state/plugin",
  "secrets/read:POLYMARKET_API_KEY",
]

[sancho]
tasks = [
  { name = "arbitrage_tick", interval = "300s", active_hours = [6, 23] },
  { name = "arbitrage_evening_report", interval = "24h", active_hours = [21, 23] },
]

[mcp]
tools = [
  { name = "arbitrage_status", handler = "arbitrage.mcp:status" },
  { name = "arbitrage_tick_now", handler = "arbitrage.mcp:tick_now" },
]

[state]
dir = "$MAKAKOO_HOME/state/arbitrage"
retention = "keep"

[test]
command = ".venv/bin/pytest"
"#;
        let (m, w) = Manifest::parse(body, &p()).unwrap();
        assert_eq!(m.plugin.name, "agent-arbitrage");
        assert_eq!(m.plugin.kind, PluginKind::Agent);
        assert_eq!(m.sancho.tasks.len(), 2);
        assert_eq!(m.mcp.tools.len(), 2);
        assert_eq!(m.capabilities.grants.len(), 6);
        assert!(w.is_empty());
    }

    // ── v0.2 E.5 — parser fuzz (property style) ────────────────────────
    //
    // `Manifest::parse` is a plugin-authored trust boundary: every
    // third-party plugin.toml goes through it, and a single panic takes
    // down `plugin sync` / `infect --verify`. These tests pound it with
    // structurally-evil inputs and assert the parser only returns
    // `Result::Err(_)` — never panics.
    //
    // Rather than ship a cargo-fuzz dependency (which needs `cargo install
    // cargo-fuzz` + a nightly harness), we use a deterministic PRNG to
    // generate 10,000 inputs per run. That's fast enough to stay in the
    // unit-test suite and still catches the class of bugs cargo-fuzz
    // would surface (unbalanced brackets, overflows, encoding tricks).

    struct PseudoRng(u64);
    impl PseudoRng {
        fn next(&mut self) -> u64 {
            // xorshift64* — deterministic, zero deps, good enough for fuzzing.
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn range(&mut self, upper: usize) -> usize {
            (self.next() as usize) % upper.max(1)
        }
        fn byte(&mut self) -> u8 {
            (self.next() & 0xff) as u8
        }
    }

    fn random_body(rng: &mut PseudoRng) -> String {
        let shape = rng.range(8);
        let size = rng.range(512) + 4;
        match shape {
            // Garbage bytes (mostly ASCII so the string is preserved).
            0 => (0..size)
                .map(|_| {
                    let b = rng.byte() & 0x7f;
                    if b < 0x20 && b != b'\n' && b != b'\t' { b'?' } else { b }
                })
                .map(char::from)
                .collect(),
            // Looks like TOML but unbalanced brackets.
            1 => "[plugin\nname = \"x\"\n".repeat(rng.range(5) + 1),
            // Deeply nested arrays.
            2 => {
                let depth = rng.range(50) + 1;
                "[".repeat(depth)
            }
            // Extremely long string value.
            3 => format!("[plugin]\nname = \"{}\"\n", "a".repeat(size)),
            // Unicode & escape tricks.
            4 => "[plugin]\nname = \"\\u0000\\uFFFF\\u{10FFFF}\"\n".into(),
            // Empty string.
            5 => String::new(),
            // Just whitespace / newlines.
            6 => "\n\r\t  ".repeat(size / 5 + 1),
            // Valid TOML but wrong types everywhere.
            _ => format!(
                "[plugin]\nname = {num}\nversion = [1,2,3]\nkind = true\n",
                num = rng.range(10_000)
            ),
        }
    }

    #[test]
    fn parse_never_panics_on_random_bodies() {
        // Deterministic seed so a CI failure is reproducible by line number.
        let mut rng = PseudoRng(0xC0FFEE_DEAD_BEEF);
        let origin = p();
        for i in 0..10_000 {
            let body = random_body(&mut rng);
            // SAFETY: we only care that parse doesn't panic. Any
            // Ok/Err outcome is fine — the parser did its job.
            let res = std::panic::catch_unwind(|| {
                let _ = Manifest::parse(&body, &origin);
            });
            assert!(
                res.is_ok(),
                "Manifest::parse panicked on iteration {i} with body (first 200 bytes): {:?}",
                &body.chars().take(200).collect::<String>()
            );
        }
    }

    #[test]
    fn parse_never_panics_on_deliberately_malformed_toml() {
        // Specific patterns known to break naive TOML parsers over the
        // years. Pinned rather than random so the test documents the
        // intent of each hostile input.
        let origin = p();
        let hostile = [
            "",                                          // empty
            "\0\0\0",                                    // nulls
            "[",                                         // incomplete
            "[[[[[",                                     // very deep
            "name = \"",                                 // unterminated string
            "[plugin]\nname = \"\\uD800\"",              // unpaired surrogate
            "[plugin]\nname = 0x7FFFFFFFFFFFFFFF",       // int overflow territory
            "[plugin]\nname = 1.0e99999",                // float overflow
            "[plugin]\nkind = \"skill\"\n[[plugin]]",    // table/array clash
            "[plugin]\r\n\x1bname = \"x\"",              // ANSI escape
            &"#".repeat(100_000),                        // massive comment
            &"[x]\n".repeat(10_000),                     // repeated sections
        ];
        for (i, body) in hostile.iter().enumerate() {
            let r = std::panic::catch_unwind(|| {
                let _ = Manifest::parse(body, &origin);
            });
            assert!(
                r.is_ok(),
                "hostile input #{i} panicked (len={}): {:?}",
                body.len(),
                &body.chars().take(60).collect::<String>()
            );
        }
    }
}
