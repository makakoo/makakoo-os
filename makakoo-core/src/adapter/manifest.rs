//! `adapter.toml` parser + validator.
//!
//! Source of truth: `spec/ADAPTER_MANIFEST.md` v1. Every validation rule
//! documented there is enforced at parse time. The canonical hash is a
//! sha256 over a deterministic normalization of the parsed manifest and
//! is used by the trust ledger to lock adapter identity across upgrades.
//!
//! Design mirrors `plugin::manifest`:
//! - strongly-typed structs with `deny_unknown_fields`
//! - enum-valued fields parsed via serde
//! - `Manifest::load` deserializes + `validate` checks cross-field rules
//! - stable `canonical_hash` used by the Phase C trust ledger
//!
//! Transport, install-lifecycle, sandbox, and signing live in separate
//! modules (Phase B/C) — this file is the pure data definition.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Current manifest schema version. The host rejects manifests with
/// `manifest_schema` > `MANIFEST_SCHEMA_VERSION`.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

static ADAPTER_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-z][a-z0-9-]{1,62}$").expect("valid regex"));

static SEMVER_TAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^v?\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$").expect("valid regex"));

static COMMIT_SHA_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-f0-9]{40}$").expect("valid regex"));

static SHA256_HEX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-f0-9]{64}$").expect("valid regex"));

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
    #[error("invalid adapter manifest {path}: {msg}")]
    Invalid { path: PathBuf, msg: String },
    #[error("unsupported manifest_schema {got} in {path} (host supports up to {supported})")]
    UnsupportedSchema {
        path: PathBuf,
        got: u32,
        supported: u32,
    },
}

impl ManifestError {
    fn invalid(path: &Path, msg: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.to_path_buf(),
            msg: msg.into(),
        }
    }
}

// ───────────────────────────────── Enums ────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    #[serde(rename = "openai-chat-v1")]
    OpenAiChatV1,
    #[serde(rename = "mcp-stdio")]
    McpStdio,
    #[serde(rename = "mcp-http")]
    McpHttp,
    #[serde(rename = "lope-verdict-block")]
    LopeVerdictBlock,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportKind {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    Subprocess,
    #[serde(rename = "mcp-stdio")]
    McpStdio,
    #[serde(rename = "mcp-http")]
    McpHttp,
    /// v0.6 — signed HTTP MCP (`X-Makakoo-{Peer,Ts,Sig}` headers,
    /// Ed25519 over sha256(body||ts)). Distinct from `mcp-http` so
    /// 3rd-party unauthenticated MCP HTTP servers still have a
    /// transport.
    #[serde(rename = "mcp-http-signed")]
    McpHttpSigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthScheme {
    Bearer,
    Header,
    Basic,
    None,
    #[serde(rename = "oauth")]
    OAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    #[serde(rename = "lope-verdict-block")]
    LopeVerdictBlock,
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    Plain,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceType {
    Local,
    Git,
    #[serde(rename = "https-tarball")]
    HttpsTarball,
    Pypi,
    Npm,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxProfile {
    None,
    #[serde(rename = "network-io")]
    NetworkIo,
    #[serde(rename = "fs-read")]
    FsRead,
    Isolated,
}

impl Default for SandboxProfile {
    fn default() -> Self {
        SandboxProfile::NetworkIo
    }
}

/// Role the adapter is willing to serve. Subset of
/// `{validator, delegate, swarm_member}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AdapterRole {
    Validator,
    Delegate,
    SwarmMember,
}

impl AdapterRole {
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterRole::Validator => "validator",
            AdapterRole::Delegate => "delegate",
            AdapterRole::SwarmMember => "swarm_member",
        }
    }
}

/// What the adapter fundamentally IS (informational — not the role matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Http,
    Subprocess,
    McpStdio,
    McpHttp,
}

// ───────────────────────────────── Tables ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterTable {
    pub name: String,
    pub version: Version,
    pub manifest_schema: u32,
    pub description: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityTable {
    pub bridge_version: VersionReq,
    #[serde(default)]
    pub makakoo_version: Option<VersionReq>,
    pub protocols: Vec<Protocol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportTable {
    pub kind: TransportKind,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub stdin: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// v0.6 — name the peer knows us by. Sent in `X-Makakoo-Peer` on
    /// every `mcp-http-signed` request. Only meaningful for that
    /// transport kind.
    #[serde(default)]
    pub peer_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthTable {
    pub scheme: AuthScheme,
    #[serde(default)]
    pub key_env: Option<String>,
    #[serde(default)]
    pub header_name: Option<String>,
    #[serde(default)]
    pub user_env: Option<String>,
    #[serde(default)]
    pub pass_env: Option<String>,
}

impl Default for AuthTable {
    fn default() -> Self {
        Self {
            scheme: AuthScheme::None,
            key_env: None,
            header_name: None,
            user_env: None,
            pass_env: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputTable {
    pub format: OutputFormat,
    #[serde(default)]
    pub parser: Option<String>,
    #[serde(default)]
    pub verdict_field: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesTable {
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub max_context: Option<u64>,
    #[serde(default)]
    pub rate_limit_rpm: Option<u64>,
    pub supports_roles: Vec<AdapterRole>,
}

/// `[install]` table. `ref` is a Rust keyword so the struct field is
/// `ref_`; the TOML key remains `ref` via a manual (de)serializer below.
#[derive(Debug, Clone)]
pub struct InstallTable {
    pub source_type: SourceType,
    pub source: Option<String>,
    pub ref_: Option<String>,
    pub sha256: Option<String>,
    pub entry_point: Option<String>,
    pub setup_cmd: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityTable {
    pub requires_network: bool,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub requires_filesystem: Vec<String>,
    #[serde(default)]
    pub requires_env: Vec<String>,
    #[serde(default)]
    pub sandbox_profile: SandboxProfile,
    #[serde(default)]
    pub signed_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthTable {
    #[serde(default)]
    pub check_url: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub expected_field: Option<String>,
}

// ────────────────────────────── Top-level ───────────────────────────────

/// A parsed, validated `adapter.toml`. Loaded via [`Manifest::load`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub adapter: AdapterTable,
    pub compatibility: CompatibilityTable,
    pub transport: TransportTable,
    #[serde(default)]
    pub auth: AuthTable,
    pub output: OutputTable,
    #[serde(default)]
    pub capabilities: CapabilitiesTable,
    pub install: InstallTable,
    pub security: SecurityTable,
    #[serde(default)]
    pub health: HealthTable,

    #[serde(skip)]
    source_path: Option<PathBuf>,
}

impl Manifest {
    /// Load and validate a manifest from a path.
    pub fn load(path: impl AsRef<Path>) -> Result<Manifest, ManifestError> {
        let path = path.as_ref();
        let body = std::fs::read_to_string(path).map_err(|e| ManifestError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut manifest: Manifest = toml::from_str(&body).map_err(|e| ManifestError::Toml {
            path: path.to_path_buf(),
            source: e,
        })?;
        manifest.source_path = Some(path.to_path_buf());
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parse from an in-memory string (used by tests + `adapter spec`).
    pub fn parse_str(body: &str) -> Result<Manifest, ManifestError> {
        let dummy = PathBuf::from("<memory>");
        let mut manifest: Manifest = toml::from_str(body).map_err(|e| ManifestError::Toml {
            path: dummy.clone(),
            source: e,
        })?;
        manifest.source_path = None;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// Enforce every cross-field rule from `spec/ADAPTER_MANIFEST.md` §3.
    pub fn validate(&self) -> Result<(), ManifestError> {
        let path = self
            .source_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<memory>"));

        // [adapter]
        if !ADAPTER_NAME_RE.is_match(&self.adapter.name) {
            return Err(ManifestError::invalid(
                &path,
                format!(
                    "adapter.name `{}` must match ^[a-z][a-z0-9-]{{1,62}}$",
                    self.adapter.name
                ),
            ));
        }
        if self.adapter.manifest_schema > MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema {
                path,
                got: self.adapter.manifest_schema,
                supported: MANIFEST_SCHEMA_VERSION,
            });
        }
        if self.adapter.manifest_schema == 0 {
            return Err(ManifestError::invalid(
                &path,
                "adapter.manifest_schema must be >= 1",
            ));
        }
        if self.adapter.description.trim().is_empty() {
            return Err(ManifestError::invalid(
                &path,
                "adapter.description must be non-empty",
            ));
        }

        // [compatibility]
        if self.compatibility.protocols.is_empty() {
            return Err(ManifestError::invalid(
                &path,
                "compatibility.protocols must list at least one protocol",
            ));
        }

        // [transport]
        match self.transport.kind {
            TransportKind::OpenAiCompatible => {
                let Some(url) = self.transport.base_url.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        "transport.base_url is required when kind = openai-compatible",
                    ));
                };
                validate_http_url(&path, "transport.base_url", url)?;
            }
            TransportKind::McpHttp => {
                let Some(url) = self.transport.url.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        "transport.url is required when kind = mcp-http",
                    ));
                };
                validate_http_url(&path, "transport.url", url)?;
            }
            TransportKind::McpHttpSigned => {
                let Some(url) = self.transport.url.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        "transport.url is required when kind = mcp-http-signed",
                    ));
                };
                validate_http_url(&path, "transport.url", url)?;
                // Signed transport requires the caller to have a local
                // signing key. The `peer_name` field tells the remote
                // how to look us up in their trust file; it's required.
                if self
                    .transport
                    .peer_name
                    .as_deref()
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
                {
                    return Err(ManifestError::invalid(
                        &path,
                        "transport.peer_name is required when kind = mcp-http-signed",
                    ));
                }
            }
            TransportKind::Subprocess | TransportKind::McpStdio => {
                if self.transport.command.is_empty() {
                    return Err(ManifestError::invalid(
                        &path,
                        "transport.command must be non-empty when kind ∈ {subprocess, mcp-stdio}",
                    ));
                }
            }
        }

        // [auth]
        match self.auth.scheme {
            AuthScheme::Bearer => {
                if self.auth.key_env.as_deref().unwrap_or("").is_empty() {
                    return Err(ManifestError::invalid(
                        &path,
                        "auth.key_env is required when scheme = bearer",
                    ));
                }
            }
            AuthScheme::Header => {
                if self.auth.key_env.as_deref().unwrap_or("").is_empty() {
                    return Err(ManifestError::invalid(
                        &path,
                        "auth.key_env is required when scheme = header",
                    ));
                }
                if self.auth.header_name.as_deref().unwrap_or("").is_empty() {
                    return Err(ManifestError::invalid(
                        &path,
                        "auth.header_name is required when scheme = header",
                    ));
                }
            }
            AuthScheme::Basic => {
                if self.auth.user_env.as_deref().unwrap_or("").is_empty()
                    || self.auth.pass_env.as_deref().unwrap_or("").is_empty()
                {
                    return Err(ManifestError::invalid(
                        &path,
                        "auth.user_env and auth.pass_env are required when scheme = basic",
                    ));
                }
            }
            AuthScheme::None | AuthScheme::OAuth => {}
        }

        // [output]
        match self.output.format {
            OutputFormat::Custom => {
                let parser = self.output.parser.as_deref().unwrap_or("");
                if !parser.contains(':') {
                    return Err(ManifestError::invalid(
                        &path,
                        "output.parser is required as `module:function` when format = custom",
                    ));
                }
            }
            OutputFormat::OpenAiChat => {
                if self
                    .output
                    .verdict_field
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(ManifestError::invalid(
                        &path,
                        "output.verdict_field is required when format = openai-chat",
                    ));
                }
            }
            OutputFormat::LopeVerdictBlock | OutputFormat::Plain => {}
        }

        // [capabilities]
        if self.capabilities.supports_roles.is_empty() {
            return Err(ManifestError::invalid(
                &path,
                "capabilities.supports_roles must list at least one role",
            ));
        }

        // [install]
        match self.install.source_type {
            SourceType::Local => {}
            SourceType::Git => {
                let Some(source) = self.install.source.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        "install.source is required for source_type = git",
                    ));
                };
                if source.trim().is_empty() {
                    return Err(ManifestError::invalid(&path, "install.source is empty"));
                }
                let Some(r) = self.install.ref_.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        "install.ref is required for source_type = git",
                    ));
                };
                if !(SEMVER_TAG_RE.is_match(r) || COMMIT_SHA_RE.is_match(r)) {
                    return Err(ManifestError::invalid(
                        &path,
                        format!(
                            "install.ref `{}` must be a semver tag (v1.2.3) or 40-char commit SHA — branches are rejected",
                            r
                        ),
                    ));
                }
            }
            SourceType::HttpsTarball | SourceType::Binary => {
                let Some(source) = self.install.source.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        format!(
                            "install.source is required for source_type = {}",
                            source_type_str(self.install.source_type)
                        ),
                    ));
                };
                validate_http_url(&path, "install.source", source)?;
                let Some(hash) = self.install.sha256.as_deref() else {
                    return Err(ManifestError::invalid(
                        &path,
                        format!(
                            "install.sha256 is required for source_type = {}",
                            source_type_str(self.install.source_type)
                        ),
                    ));
                };
                if !SHA256_HEX_RE.is_match(hash) {
                    return Err(ManifestError::invalid(
                        &path,
                        "install.sha256 must be 64 lowercase hex chars",
                    ));
                }
            }
            SourceType::Pypi | SourceType::Npm => {
                if self.install.source.as_deref().unwrap_or("").is_empty() {
                    return Err(ManifestError::invalid(
                        &path,
                        format!(
                            "install.source (package name) is required for source_type = {}",
                            source_type_str(self.install.source_type)
                        ),
                    ));
                }
            }
        }

        // [security]
        if self.security.requires_network && self.security.allowed_hosts.is_empty() {
            return Err(ManifestError::invalid(
                &path,
                "security.allowed_hosts must be non-empty when requires_network = true",
            ));
        }
        for fs_item in &self.security.requires_filesystem {
            if !(fs_item.starts_with("read:") || fs_item.starts_with("write:")) {
                return Err(ManifestError::invalid(
                    &path,
                    format!(
                        "security.requires_filesystem entry `{}` must start with `read:` or `write:`",
                        fs_item
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Deterministic canonical hash of the parsed manifest. Same manifest
    /// → same hex string on any machine. Used by the trust ledger to lock
    /// adapter identity at trust time.
    pub fn canonical_hash(&self) -> String {
        let json = self.canonical_json();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(64);
        for b in digest.iter() {
            use std::fmt::Write as _;
            let _ = write!(out, "{:02x}", b);
        }
        out
    }

    /// Stable JSON normalization used both for hashing and for `spec` dumps.
    pub fn canonical_json(&self) -> String {
        let v = serde_json::to_value(self).expect("manifest serializes");
        let canonical = canonicalize_value(v);
        serde_json::to_string(&canonical).expect("canonical serializes")
    }

    /// Convenience: does this adapter advertise the given role?
    pub fn supports_role(&self, role: AdapterRole) -> bool {
        self.capabilities.supports_roles.contains(&role)
    }

    pub fn kind(&self) -> AdapterKind {
        match self.transport.kind {
            TransportKind::OpenAiCompatible => AdapterKind::Http,
            TransportKind::Subprocess => AdapterKind::Subprocess,
            TransportKind::McpStdio => AdapterKind::McpStdio,
            TransportKind::McpHttp | TransportKind::McpHttpSigned => AdapterKind::McpHttp,
        }
    }
}

// serde-rename workaround: accept `ref` in TOML and stash it into `ref_`.
// Using serde's field attribute with a rename-on-both-ser-and-deser.
// The struct definition above uses `#[serde(default)] pub ref_: ...` which
// wouldn't pick up `ref` in the TOML. We re-derive via a manual impl so
// existing structs stay clean.

mod install_serde {
    use super::{InstallTable, SourceType};
    use serde::ser::{SerializeStruct, Serializer};
    use serde::{Deserialize, Deserializer, Serialize};

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Shadow {
        source_type: SourceType,
        #[serde(default)]
        source: Option<String>,
        #[serde(default, rename = "ref")]
        ref_: Option<String>,
        #[serde(default)]
        sha256: Option<String>,
        #[serde(default)]
        entry_point: Option<String>,
        #[serde(default)]
        setup_cmd: Vec<String>,
    }

    impl<'de> Deserialize<'de> for InstallTable {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = Shadow::deserialize(d)?;
            Ok(InstallTable {
                source_type: s.source_type,
                source: s.source,
                ref_: s.ref_,
                sha256: s.sha256,
                entry_point: s.entry_point,
                setup_cmd: s.setup_cmd,
            })
        }
    }

    impl Serialize for InstallTable {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut st = s.serialize_struct("InstallTable", 6)?;
            st.serialize_field("source_type", &self.source_type)?;
            st.serialize_field("source", &self.source)?;
            st.serialize_field("ref", &self.ref_)?;
            st.serialize_field("sha256", &self.sha256)?;
            st.serialize_field("entry_point", &self.entry_point)?;
            st.serialize_field("setup_cmd", &self.setup_cmd)?;
            st.end()
        }
    }
}

// ───────────────────────────── Helpers ──────────────────────────────────

fn validate_http_url(path: &Path, field: &str, url: &str) -> Result<(), ManifestError> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(ManifestError::invalid(
            path,
            format!("{} `{}` must start with http:// or https://", field, url),
        ));
    }
    Ok(())
}

fn source_type_str(t: SourceType) -> &'static str {
    match t {
        SourceType::Local => "local",
        SourceType::Git => "git",
        SourceType::HttpsTarball => "https-tarball",
        SourceType::Pypi => "pypi",
        SourceType::Npm => "npm",
        SourceType::Binary => "binary",
    }
}

/// Recursively normalize a `serde_json::Value` so that object keys are
/// emitted in sorted order. `serde_json::to_value` doesn't guarantee key
/// ordering, so we canonicalize explicitly.
fn canonicalize_value(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(map) => {
            let mut ordered: BTreeMap<String, Value> = BTreeMap::new();
            for (k, val) in map {
                ordered.insert(k, canonicalize_value(val));
            }
            let mut obj = serde_json::Map::new();
            for (k, val) in ordered {
                obj.insert(k, val);
            }
            Value::Object(obj)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(canonicalize_value).collect()),
        other => other,
    }
}

// ───────────────────────────── Tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_MINIMAL: &str = r#"
[adapter]
name = "openclaw"
version = "1.4.2"
manifest_schema = 1
description = "OpenClaw agent"

[compatibility]
bridge_version = "^2.0"
protocols = ["openai-chat-v1"]

[transport]
kind = "openai-compatible"
base_url = "http://127.0.0.1:3000/v1"

[auth]
scheme = "bearer"
key_env = "OPENCLAW_API_KEY"

[output]
format = "lope-verdict-block"

[capabilities]
supports_roles = ["validator"]

[install]
source_type = "git"
source = "https://github.com/traylinx/openclaw-adapter.git"
ref = "v1.4.2"

[security]
requires_network = true
allowed_hosts = ["127.0.0.1"]
sandbox_profile = "network-io"
signed_by = "traylinx"
"#;

    #[test]
    fn parses_valid_minimal() {
        let m = Manifest::parse_str(VALID_MINIMAL).expect("parses");
        assert_eq!(m.adapter.name, "openclaw");
        assert_eq!(m.adapter.version.to_string(), "1.4.2");
        assert_eq!(m.transport.kind, TransportKind::OpenAiCompatible);
        assert_eq!(m.output.format, OutputFormat::LopeVerdictBlock);
        assert!(m.supports_role(AdapterRole::Validator));
        assert!(!m.supports_role(AdapterRole::Delegate));
    }

    #[test]
    fn rejects_unknown_field() {
        let body = VALID_MINIMAL.replace("[output]", "[output]\nmystery_field = 42\n");
        let err = Manifest::parse_str(&body).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_branch_ref() {
        let body = VALID_MINIMAL.replace(r#"ref = "v1.4.2""#, r#"ref = "main""#);
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("branches are rejected"), "got {msg}");
    }

    #[test]
    fn accepts_commit_sha_ref() {
        let body = VALID_MINIMAL.replace(
            r#"ref = "v1.4.2""#,
            r#"ref = "a1b2c3d4e5f6789012345678901234567890abcd""#,
        );
        Manifest::parse_str(&body).expect("40-char SHA accepted");
    }

    #[test]
    fn rejects_bad_name() {
        let body = VALID_MINIMAL.replace(r#"name = "openclaw""#, r#"name = "Bad Name""#);
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("must match"), "got {msg}");
    }

    #[test]
    fn rejects_bad_semver() {
        let body = VALID_MINIMAL.replace(r#"version = "1.4.2""#, r#"version = "not-a-version""#);
        let err = Manifest::parse_str(&body).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_missing_base_url_for_http_transport() {
        let body = VALID_MINIMAL.replace(
            r#"base_url = "http://127.0.0.1:3000/v1""#,
            r#"# base_url removed"#,
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("base_url"), "got {msg}");
    }

    #[test]
    fn rejects_non_http_base_url() {
        let body = VALID_MINIMAL.replace(
            r#"base_url = "http://127.0.0.1:3000/v1""#,
            r#"base_url = "ftp://example.com/""#,
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("http://"), "got {msg}");
    }

    #[test]
    fn rejects_unsupported_schema() {
        let body = VALID_MINIMAL.replace("manifest_schema = 1", "manifest_schema = 99");
        let err = Manifest::parse_str(&body).unwrap_err();
        assert!(matches!(err, ManifestError::UnsupportedSchema { .. }));
    }

    #[test]
    fn rejects_missing_allowed_hosts_when_network() {
        let body = VALID_MINIMAL.replace(r#"allowed_hosts = ["127.0.0.1"]"#, "allowed_hosts = []");
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("allowed_hosts"), "got {msg}");
    }

    #[test]
    fn rejects_unknown_protocol() {
        let body = VALID_MINIMAL.replace(
            r#"protocols = ["openai-chat-v1"]"#,
            r#"protocols = ["made-up-protocol-v9"]"#,
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err:?}");
    }

    #[test]
    fn custom_output_requires_parser() {
        let body = VALID_MINIMAL.replace(
            r#"format = "lope-verdict-block""#,
            r#"format = "custom""#,
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("parser"), "got {msg}");
    }

    #[test]
    fn openai_chat_requires_verdict_field() {
        let body = VALID_MINIMAL.replace(
            r#"format = "lope-verdict-block""#,
            r#"format = "openai-chat""#,
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("verdict_field"), "got {msg}");
    }

    #[test]
    fn bearer_auth_requires_key_env() {
        let body = VALID_MINIMAL.replace(r#"key_env = "OPENCLAW_API_KEY""#, "");
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("key_env"), "got {msg}");
    }

    #[test]
    fn tarball_requires_sha256() {
        let body = VALID_MINIMAL
            .replace(
                r#"source_type = "git""#,
                r#"source_type = "https-tarball""#,
            )
            .replace(
                r#"source = "https://github.com/traylinx/openclaw-adapter.git""#,
                r#"source = "https://example.com/adapter.tgz""#,
            )
            .replace(r#"ref = "v1.4.2""#, "");
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("sha256"), "got {msg}");
    }

    #[test]
    fn rejects_filesystem_spec_without_prefix() {
        let insert = r#"requires_filesystem = ["~/HARVEY/state"]"#;
        let body = VALID_MINIMAL.replace(
            r#"sandbox_profile = "network-io""#,
            &format!("{}\nsandbox_profile = \"network-io\"", insert),
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("read:"), "got {msg}");
    }

    #[test]
    fn canonical_hash_is_deterministic() {
        let a = Manifest::parse_str(VALID_MINIMAL).unwrap();
        let b = Manifest::parse_str(VALID_MINIMAL).unwrap();
        assert_eq!(a.canonical_hash(), b.canonical_hash());
        assert_eq!(a.canonical_hash().len(), 64);
    }

    #[test]
    fn canonical_hash_changes_on_field_change() {
        let a = Manifest::parse_str(VALID_MINIMAL).unwrap();
        let body = VALID_MINIMAL.replace(r#"version = "1.4.2""#, r#"version = "1.4.3""#);
        let b = Manifest::parse_str(&body).unwrap();
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn supports_roles_must_not_be_empty() {
        let body = VALID_MINIMAL.replace(
            r#"supports_roles = ["validator"]"#,
            r#"supports_roles = []"#,
        );
        let err = Manifest::parse_str(&body).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("supports_roles"), "got {msg}");
    }
}
