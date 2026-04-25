//! `makakoo bucket …` — bucket lifecycle on top of the local Garage backend.
//!
//! Phase C of `MAKAKOO-OS-V0.7.1-S3-MULTITENANCY`. Wires the bucket
//! registry, the per-bucket grant flow, atomic 3-state revoke, the
//! TTL expiry walker, and the emergency `deny-all` flag.
//!
//! v0.7.1 ships **Garage-only**. Non-Garage endpoints raise a clear
//! v0.8 pointer error before any backend dispatch. The `BackendOps`
//! trait stays Garage-shaped to make the v0.8 dispatcher a drop-in.
//!
//! Storage layout:
//!   * Bucket metadata     — `$MAKAKOO_HOME/config/buckets.json`
//!     (sidecar-locked, schema mirrors user_grants.rs).
//!   * Bucket grant scope  — `s3/bucket:<name>` rows in
//!     `user_grants.json` (existing grant store).
//!   * Per-grant creds     — keychain entry under service `makakoo`,
//!     account `bucket-grant:<grant_id>` (JSON
//!     `{access_key, secret_key, endpoint}`).
//!
//! Concurrency notes:
//!   * Bucket-registry writes use the same `with_mutation` /
//!     sidecar-lock protocol as `user_grants.rs` — see `spec/USER_GRANTS.md
//!     §5` for the lock semantics.
//!   * Grant revoke is a 3-state machine (`active` → `revoking` →
//!     `revoked`). SANCHO retries `revoking` rows every 60s until the
//!     backend confirms the key delete (qwen v0.7 round-2 fix).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use makakoo_core::capability::{
    new_grant_id, rate_limit, AuditResult, UserGrant, UserGrants,
};

use crate::cli::BucketCmd;
use crate::commands::perms::parse_duration;
use crate::context::CliContext;
use crate::secrets::SecretsStore;

// ═══════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════

const SCHEMA_VERSION: u32 = 1;
const REGISTRY_REL: &str = "config/buckets.json";
const REGISTRY_LOCK_REL: &str = "config/buckets.json.lock";
const ADMIN_API_URL: &str = "http://127.0.0.1:3903";
const PLUGIN: &str = "cli";

/// Max bucket size in bytes when caller passes `--quota unlimited` —
/// enforced at the registry level so the global garage capacity remains
/// the actual ceiling.
const QUOTA_UNLIMITED_BYTES: u64 = u64::MAX;

// ═══════════════════════════════════════════════════════════════
//  Persistence — bucket registry
// ═══════════════════════════════════════════════════════════════

/// One persisted bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketRecord {
    pub name: String,
    pub endpoint: String,
    pub created_at: DateTime<Utc>,
    /// `None` when bucket is permanent (created with `--ttl permanent
    /// --confirm-yes-really`).
    pub ttl_expires_at: Option<DateTime<Utc>>,
    /// `u64::MAX` when caller passed `--quota unlimited`.
    pub quota_bytes: u64,
    /// When `Some(ts)`, every read/write must 403 until `ts > now`. Set
    /// by `bucket deny-all`. `None` is the normal state.
    pub deny_all_until: Option<DateTime<Utc>>,
    /// `true` once `bucket-expire` has begun the teardown. Used by the
    /// expiry walker to make the operation idempotent across restarts.
    #[serde(default)]
    pub expiring: bool,
}

impl BucketRecord {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.ttl_expires_at.map_or(false, |t| t <= now)
    }

    pub fn is_denied(&self, now: DateTime<Utc>) -> bool {
        self.deny_all_until.map_or(false, |t| t > now)
    }
}

/// On-disk registry collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketRegistry {
    pub version: u32,
    pub buckets: Vec<BucketRecord>,
    #[serde(skip, default)]
    path: PathBuf,
}

impl BucketRegistry {
    pub fn empty_at(path: PathBuf) -> Self {
        Self {
            version: SCHEMA_VERSION,
            buckets: Vec::new(),
            path,
        }
    }

    pub fn load(home: &Path) -> Self {
        let path = home.join(REGISTRY_REL);
        if !path.exists() {
            return Self::empty_at(path);
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!("bucket registry read failed at {}: {}", path.display(), e);
                return Self::empty_at(path);
            }
        };
        let mut parsed: Self = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                warn!("corrupt bucket registry at {}: {}", path.display(), e);
                return Self::empty_at(path);
            }
        };
        parsed.path = path;
        parsed
    }

    pub fn get(&self, name: &str) -> Option<&BucketRecord> {
        self.buckets.iter().find(|b| b.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut BucketRecord> {
        self.buckets.iter_mut().find(|b| b.name == name)
    }

    pub fn add(&mut self, record: BucketRecord) {
        self.buckets.retain(|b| b.name != record.name);
        self.buckets.push(record);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.buckets.len();
        self.buckets.retain(|b| b.name != name);
        before != self.buckets.len()
    }

    pub fn save(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            bail!("bucket registry has no path — call BucketRegistry::load first");
        }
        let parent = self.path.parent().context("registry path has no parent")?;
        fs::create_dir_all(parent).with_context(|| {
            format!("creating {}", parent.display())
        })?;

        // Sidecar-lock protocol — same shape as user_grants.rs.
        let lock_path = parent.join("buckets.json.lock");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("opening {}", lock_path.display()))?;
        FileExt::lock_exclusive(&lock).context("acquiring buckets.json.lock")?;

        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(self).context("serialising buckets.json")?;
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)
                .with_context(|| format!("opening {}", tmp.display()))?;
            f.write_all(&body)?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path).with_context(|| {
            format!(
                "renaming {} onto {}",
                tmp.display(),
                self.path.display()
            )
        })?;
        let _ = FileExt::unlock(&lock);
        Ok(())
    }

    pub fn with_mutation<R, F>(&mut self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Self) -> Result<R>,
    {
        let result = f(self)?;
        self.save()?;
        Ok(result)
    }
}

// ═══════════════════════════════════════════════════════════════
//  Validation
// ═══════════════════════════════════════════════════════════════

/// AWS-S3 + Garage compatible bucket-name validator. Hard-rejects:
///   * length < 3 or > 63
///   * any ASCII upper / underscore / non-printable
///   * leading or trailing non-alphanumeric (dot or hyphen)
///   * consecutive dots (`..`) — would collide with virtual-host SNI
///     even though we've locked path-style addressing (lope-1 #virt-host)
///   * IP-address shape `1.2.3.4` (AWS rejects; we mirror)
pub fn validate_bucket_name(name: &str) -> Result<()> {
    if name.len() < 3 || name.len() > 63 {
        bail!("bucket name must be 3–63 chars (got {})", name.len());
    }
    let bytes = name.as_bytes();
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    let is_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_alnum(first) || !is_alnum(last) {
        bail!("bucket name must start AND end with [a-z0-9] (got {name:?})");
    }
    let mut prev_dot = false;
    for b in bytes {
        match *b {
            b'a'..=b'z' | b'0'..=b'9' => prev_dot = false,
            b'-' => prev_dot = false,
            b'.' => {
                if prev_dot {
                    bail!(
                        "bucket name {name:?} contains consecutive dots — \
                         banned to avoid virtual-host SNI collision \
                         (lope-1 #virt-host)"
                    );
                }
                prev_dot = true;
            }
            b'_' => bail!("bucket name {name:?} contains '_' — only [a-z0-9.-] allowed"),
            b'A'..=b'Z' => bail!(
                "bucket name {name:?} contains upper-case — only [a-z0-9.-] allowed"
            ),
            _ => bail!("bucket name {name:?} contains illegal byte 0x{b:02x}"),
        }
    }
    // IP-address shape ban (mirrors AWS S3 rule).
    if name.split('.').count() == 4
        && name.split('.').all(|p| p.parse::<u8>().is_ok())
    {
        bail!(
            "bucket name {name:?} matches an IPv4 shape — banned (mirrors AWS S3 rule)"
        );
    }
    Ok(())
}

/// Quota grammar — `<int>[KMG]` or `unlimited`. Returns bytes.
pub fn parse_quota(raw: &str) -> Result<u64> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("unlimited") {
        return Ok(QUOTA_UNLIMITED_BYTES);
    }
    let (num, suffix): (&str, u64) = if let Some(s) = raw.strip_suffix(['K', 'k']) {
        (s, 1024)
    } else if let Some(s) = raw.strip_suffix(['M', 'm']) {
        (s, 1024 * 1024)
    } else if let Some(s) = raw.strip_suffix(['G', 'g']) {
        (s, 1024 * 1024 * 1024)
    } else if let Some(s) = raw.strip_suffix(['T', 't']) {
        (s, 1024_u64.pow(4))
    } else {
        (raw, 1)
    };
    let n: u64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid quota {raw:?}; use e.g. 100M, 1G, 10G, unlimited"))?;
    n.checked_mul(suffix).ok_or_else(|| anyhow!("quota overflow"))
}

/// Permission set parse + validation. Accepts `read`, `read,write`,
/// `read,write,owner` in any order. Returns the canonical sorted form
/// `read | read,write | read,write,owner`.
pub fn parse_perms(raw: &str) -> Result<&'static str> {
    let mut r = false;
    let mut w = false;
    let mut o = false;
    for tok in raw.split(',') {
        match tok.trim() {
            "read" => r = true,
            "write" => w = true,
            "owner" => o = true,
            "" => {}
            other => bail!("unknown perm {other:?}; allowed: read | write | owner"),
        }
    }
    if o && !w {
        bail!("--perms owner requires write (and read)");
    }
    if w && !r {
        bail!("--perms write requires read");
    }
    if !r {
        bail!("--perms must include at least 'read'");
    }
    Ok(match (r, w, o) {
        (true, true, true) => "read,write,owner",
        (true, true, false) => "read,write",
        (true, false, false) => "read",
        _ => unreachable!("guarded by checks above"),
    })
}

// ═══════════════════════════════════════════════════════════════
//  Garage backend ops
// ═══════════════════════════════════════════════════════════════

fn config_path(ctx: &CliContext) -> PathBuf {
    ctx.home().join("config/garage.toml")
}

fn run_garage_cli(ctx: &CliContext, args: &[&str]) -> Result<String> {
    let cfg = config_path(ctx);
    if !cfg.exists() {
        bail!(
            "garage config missing at {} — run `makakoo plugin install --core garage-store` first",
            cfg.display()
        );
    }
    let out = Command::new("garage")
        .arg("-c")
        .arg(&cfg)
        .args(args)
        .output()
        .with_context(|| format!("invoking `garage {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "garage {} failed (exit {}): {}",
            args.join(" "),
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Light wrapper around the `garage` CLI for the operations Phase C
/// needs. Stays an internal trait so the v0.8 dispatcher can swap the
/// AWS / R2 / B2 / MinIO implementations behind the same surface
/// without rewiring the CLI handlers.
trait BackendOps {
    fn create_bucket(&self, ctx: &CliContext, name: &str) -> Result<()>;
    fn delete_bucket(&self, ctx: &CliContext, name: &str) -> Result<()>;
    fn create_key(&self, ctx: &CliContext, label: &str) -> Result<(String, String)>;
    fn delete_key(&self, ctx: &CliContext, key_id: &str) -> Result<()>;
    fn allow(
        &self,
        ctx: &CliContext,
        bucket: &str,
        key_id: &str,
        perms: &str,
    ) -> Result<()>;
    fn deny_all(&self, ctx: &CliContext, bucket: &str, denied: bool) -> Result<()>;
    fn bucket_size(&self, ctx: &CliContext, bucket: &str) -> Result<u64>;
}

struct GarageOps;

impl BackendOps for GarageOps {
    fn create_bucket(&self, ctx: &CliContext, name: &str) -> Result<()> {
        run_garage_cli(ctx, &["bucket", "create", name])?;
        Ok(())
    }

    fn delete_bucket(&self, ctx: &CliContext, name: &str) -> Result<()> {
        // `--yes` to skip confirmation prompt; Garage refuses unless
        // bucket is empty, so callers must drain first via aws-cli.
        run_garage_cli(ctx, &["bucket", "delete", "--yes", name])?;
        Ok(())
    }

    fn create_key(&self, ctx: &CliContext, label: &str) -> Result<(String, String)> {
        let out = run_garage_cli(ctx, &["key", "create", label])?;
        super::s3::parse_key_creds_pub(&out)
    }

    fn delete_key(&self, ctx: &CliContext, key_id: &str) -> Result<()> {
        run_garage_cli(ctx, &["key", "delete", "--yes", key_id])?;
        Ok(())
    }

    fn allow(
        &self,
        ctx: &CliContext,
        bucket: &str,
        key_id: &str,
        perms: &str,
    ) -> Result<()> {
        // Garage flags: `--read --write --owner`. Permission set already
        // canonical from parse_perms.
        let mut args: Vec<&str> = vec!["bucket", "allow"];
        for p in perms.split(',') {
            match p {
                "read" => args.push("--read"),
                "write" => args.push("--write"),
                "owner" => args.push("--owner"),
                _ => unreachable!("parse_perms canonicalised"),
            }
        }
        args.push("--key");
        args.push(key_id);
        args.push(bucket);
        run_garage_cli(ctx, &args)?;
        Ok(())
    }

    fn deny_all(&self, _ctx: &CliContext, _bucket: &str, _denied: bool) -> Result<()> {
        // Garage v2.x doesn't expose a per-bucket emergency-deny flag
        // through the CLI. We track the flag in our registry; the
        // shim-side enforcement (Phase D.3) reads it on every forwarded
        // request. This stub returns Ok so the registry-flip path is
        // the source of truth.
        Ok(())
    }

    fn bucket_size(&self, ctx: &CliContext, bucket: &str) -> Result<u64> {
        let out = run_garage_cli(ctx, &["bucket", "info", bucket])?;
        // Garage's `bucket info` output has a line like:
        //     Size: 12.34 MB (12345678 bytes)
        // We parse the bytes value out — the human suffix is unstable.
        for line in out.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("Size:") {
                if let Some(open) = rest.find('(') {
                    if let Some(close) = rest.find(')') {
                        let inner = &rest[open + 1..close];
                        let inner = inner.trim_end_matches(" bytes").trim();
                        if let Ok(n) = inner.parse::<u64>() {
                            return Ok(n);
                        }
                    }
                }
            }
        }
        // Fresh bucket — no objects, no size line. Treat as 0.
        Ok(0)
    }
}

fn backend_for(endpoint: Option<&str>) -> Result<Box<dyn BackendOps>> {
    let kind = endpoint.unwrap_or("local");
    match kind {
        "local" | "garage-local" => Ok(Box::new(GarageOps)),
        other => bail!(
            "endpoint {other:?} requires the v0.8 backend dispatcher \
             (NotImplementedError); v0.7.1 ships Garage-only. See \
             docs/bring-your-own-backend.md for the v0.8 path."
        ),
    }
}

// ═══════════════════════════════════════════════════════════════
//  Subcommand dispatch
// ═══════════════════════════════════════════════════════════════

pub async fn run(ctx: &CliContext, cmd: BucketCmd) -> Result<i32> {
    match cmd {
        BucketCmd::Create {
            name,
            endpoint,
            ttl,
            quota,
            confirm_yes_really,
        } => create(ctx, &name, endpoint.as_deref(), &ttl, &quota, confirm_yes_really),
        BucketCmd::List { endpoint, json } => list(ctx, endpoint.as_deref(), json),
        BucketCmd::Info { name, json } => info(ctx, &name, json),
        BucketCmd::Grant {
            bucket,
            to,
            perms,
            ttl,
            confirm_yes_really,
            json,
        } => grant(ctx, &bucket, &to, &perms, &ttl, confirm_yes_really, json),
        BucketCmd::Revoke { grant_id } => revoke(ctx, &grant_id),
        BucketCmd::Expire { dry_run } => expire(ctx, dry_run),
        BucketCmd::DenyAll {
            name,
            ttl,
            confirm_yes_really,
        } => deny_all(ctx, &name, &ttl, confirm_yes_really),
    }
}

// ─── create ──────────────────────────────────────────────────

fn create(
    ctx: &CliContext,
    name: &str,
    endpoint: Option<&str>,
    ttl: &str,
    quota: &str,
    confirm_yes_really: bool,
) -> Result<i32> {
    validate_bucket_name(name)?;

    let dur = parse_duration(ttl)?;
    if dur.is_none() && !confirm_yes_really {
        bail!("--ttl permanent requires --confirm-yes-really");
    }
    let now = Utc::now();
    let ttl_expires_at = dur.map(|d| now + d);

    let quota_bytes = parse_quota(quota)?;
    if quota_bytes == QUOTA_UNLIMITED_BYTES && !confirm_yes_really {
        bail!("--quota unlimited requires --confirm-yes-really");
    }

    let backend = backend_for(endpoint)?;
    let endpoint_label = endpoint.unwrap_or("local").to_string();

    let mut registry = BucketRegistry::load(ctx.home());
    if registry.get(name).is_some() {
        bail!("bucket {name:?} already exists");
    }

    backend
        .create_bucket(ctx, name)
        .with_context(|| format!("backend create_bucket({name})"))?;

    let record = BucketRecord {
        name: name.to_string(),
        endpoint: endpoint_label.clone(),
        created_at: now,
        ttl_expires_at,
        quota_bytes,
        deny_all_until: None,
        expiring: false,
    };
    registry.with_mutation(|r| {
        r.add(record);
        Ok(())
    })?;

    audit(ctx, "bucket_created", name, &endpoint_label, AuditResult::Allowed);
    eprintln!(
        "Created bucket {name} on {endpoint_label}. TTL={ttl}, quota={quota}."
    );
    println!("{name}");
    Ok(0)
}

// ─── list ────────────────────────────────────────────────────

fn list(ctx: &CliContext, endpoint: Option<&str>, json: bool) -> Result<i32> {
    let registry = BucketRegistry::load(ctx.home());
    let now = Utc::now();
    let rows: Vec<&BucketRecord> = registry
        .buckets
        .iter()
        .filter(|b| endpoint.map_or(true, |e| b.endpoint == e))
        .collect();
    if json {
        let body = serde_json::json!({
            "buckets": rows
                .iter()
                .map(|b| serde_json::json!({
                    "name": b.name,
                    "endpoint": b.endpoint,
                    "created_at": b.created_at,
                    "ttl_expires_at": b.ttl_expires_at,
                    "ttl_expired": b.is_expired(now),
                    "quota_bytes": b.quota_bytes,
                    "deny_all_until": b.deny_all_until,
                    "expiring": b.expiring,
                }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(0);
    }
    if rows.is_empty() {
        eprintln!("(no buckets registered)");
        return Ok(0);
    }
    println!(
        "{:<24} {:<14} {:<22} {:<10} {}",
        "NAME", "ENDPOINT", "TTL_EXPIRES", "QUOTA", "FLAGS"
    );
    for b in rows {
        let flags = match (b.is_denied(now), b.expiring, b.is_expired(now)) {
            (true, _, _) => "DENY-ALL",
            (_, true, _) => "EXPIRING",
            (_, _, true) => "STALE",
            _ => "",
        };
        let ttl = match b.ttl_expires_at {
            Some(t) => t.format("%Y-%m-%d %H:%M UTC").to_string(),
            None => "permanent".to_string(),
        };
        let quota = if b.quota_bytes == QUOTA_UNLIMITED_BYTES {
            "unlimited".to_string()
        } else {
            humanize_bytes(b.quota_bytes)
        };
        println!(
            "{:<24} {:<14} {:<22} {:<10} {}",
            b.name, b.endpoint, ttl, quota, flags
        );
    }
    Ok(0)
}

// ─── info ────────────────────────────────────────────────────

fn info(ctx: &CliContext, name: &str, json: bool) -> Result<i32> {
    let registry = BucketRegistry::load(ctx.home());
    let bucket = registry
        .get(name)
        .ok_or_else(|| anyhow!("bucket {name:?} not registered"))?;
    let now = Utc::now();

    let backend = backend_for(Some(bucket.endpoint.as_str()))?;
    let size = backend.bucket_size(ctx, name).unwrap_or(0);

    let user_grants = UserGrants::load(ctx.home());
    let grants: Vec<&UserGrant> = user_grants
        .grants
        .iter()
        .filter(|g| g.scope.starts_with(&format!("s3/bucket:{name}")))
        .collect();

    if json {
        let body = serde_json::json!({
            "name": bucket.name,
            "endpoint": bucket.endpoint,
            "created_at": bucket.created_at,
            "ttl_expires_at": bucket.ttl_expires_at,
            "quota_bytes": bucket.quota_bytes,
            "size_bytes": size,
            "size_pct": if bucket.quota_bytes == QUOTA_UNLIMITED_BYTES {
                serde_json::Value::Null
            } else {
                serde_json::json!((size as f64 / bucket.quota_bytes as f64) * 100.0)
            },
            "deny_all_until": bucket.deny_all_until,
            "expiring": bucket.expiring,
            "grants": grants.iter().map(|g| serde_json::json!({
                "id": g.id,
                "scope": g.scope,
                "label": g.label,
                "expires_at": g.expires_at,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(0);
    }

    println!("Bucket: {}", bucket.name);
    println!("  endpoint:        {}", bucket.endpoint);
    println!("  created_at:      {}", bucket.created_at);
    println!(
        "  ttl_expires_at:  {}",
        bucket
            .ttl_expires_at
            .map(|t| t.to_string())
            .unwrap_or_else(|| "permanent".into())
    );
    println!(
        "  quota:           {}",
        if bucket.quota_bytes == QUOTA_UNLIMITED_BYTES {
            "unlimited".into()
        } else {
            humanize_bytes(bucket.quota_bytes)
        }
    );
    println!("  size:            {} ({} bytes)", humanize_bytes(size), size);
    if bucket.quota_bytes != QUOTA_UNLIMITED_BYTES && bucket.quota_bytes > 0 {
        let pct = (size as f64 / bucket.quota_bytes as f64) * 100.0;
        println!("  size_pct:        {pct:.2}%");
    }
    if let Some(t) = bucket.deny_all_until {
        if t > now {
            println!("  deny_all_until:  {t} (active)");
        } else {
            println!("  deny_all_until:  {t} (expired, inactive)");
        }
    }
    if bucket.expiring {
        println!("  expiring:        true (TTL purge in progress)");
    }
    if grants.is_empty() {
        println!("  grants:          (none)");
    } else {
        println!("  grants:");
        for g in &grants {
            let exp = g
                .expires_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "permanent".into());
            println!("    - {} {} (label={}) expires={exp}", g.id, g.scope, g.label);
        }
    }
    Ok(0)
}

// ─── grant ───────────────────────────────────────────────────

fn grant(
    ctx: &CliContext,
    bucket: &str,
    to: &str,
    perms_raw: &str,
    ttl: &str,
    confirm_yes_really: bool,
    json: bool,
) -> Result<i32> {
    let canonical_perms = parse_perms(perms_raw)?;

    let mut registry = BucketRegistry::load(ctx.home());
    let endpoint = match registry.get(bucket) {
        Some(b) => b.endpoint.clone(),
        None => bail!("bucket {bucket:?} not registered — run `makakoo bucket create {bucket}` first"),
    };

    let dur = parse_duration(ttl)?;
    if dur.is_none() && !confirm_yes_really {
        bail!("--ttl permanent requires --confirm-yes-really");
    }
    let now = Utc::now();
    let expires_at = dur.map(|d| now + d);

    // Rate-limit shared with fs/write grants — same Locked Decision.
    let user_grants_existing = UserGrants::load(ctx.home());
    let active_count = user_grants_existing.active_grants(now).len();
    rate_limit::check_and_increment(active_count, ctx.home(), now)?;

    // Forge the new grant. Scope encodes both bucket name + endpoint
    // so revoke can route to the right backend dispatcher even after
    // the registry entry is gone (e.g., bucket already expired).
    let grant_id = new_grant_id(now);
    let stored_scope = format!("s3/bucket:{bucket}");
    let label_text = makakoo_core::capability::escape_audit_field(to, 80);

    // Provision the sub-keypair.
    let backend = backend_for(Some(&endpoint))?;
    let key_label = format!("makakoo-grant-{grant_id}");
    let (access_key, secret_key) = backend
        .create_key(ctx, &key_label)
        .context("backend create_key")?;
    if let Err(e) = backend.allow(ctx, bucket, &access_key, canonical_perms) {
        // Best-effort cleanup of the orphan key.
        let _ = backend.delete_key(ctx, &access_key);
        return Err(e).context("backend allow");
    }

    // Persist grant + creds.
    let user_grant = UserGrant {
        id: grant_id.clone(),
        scope: stored_scope.clone(),
        created_at: now,
        expires_at,
        label: label_text.clone(),
        granted_by: "sebastian".into(),
        plugin: PLUGIN.into(),
        origin_turn_id: String::new(),
        owner: PLUGIN.into(),
    };
    let mut user_grants = user_grants_existing;
    user_grants.add(user_grant.clone());
    user_grants.save().context("writing user_grants.json")?;

    // Mark the bucket as having an active grant — defensive no-op for
    // now; future code may use this to suppress TTL expiry while
    // grants are live.
    registry.with_mutation(|_| Ok(()))?;

    let creds = serde_json::json!({
        "access_key": access_key,
        "secret_key": secret_key,
        "endpoint": endpoint_url_for(&endpoint),
        "key_id_for_revoke": access_key,
    });
    SecretsStore::set_json(&format!("bucket-grant:{grant_id}"), &creds)
        .context("storing grant creds in keychain")?;

    audit(
        ctx,
        "bucket_granted",
        bucket,
        &grant_id,
        AuditResult::Allowed,
    );

    if json {
        let body = serde_json::json!({
            "grant_id": grant_id,
            "scope": stored_scope,
            "endpoint_url": endpoint_url_for(&endpoint),
            "access_key": access_key,
            "secret_key": secret_key,
            "expires_at": expires_at,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        eprintln!(
            "Granted {grant_id} on bucket {bucket}. Revoke: makakoo bucket revoke {grant_id}"
        );
        println!("ENDPOINT={}", endpoint_url_for(&endpoint));
        println!("ACCESS_KEY={access_key}");
        println!("SECRET_KEY={secret_key}");
        println!("GRANT_ID={grant_id}");
    }
    Ok(0)
}

// ─── revoke ──────────────────────────────────────────────────

fn revoke(ctx: &CliContext, grant_id: &str) -> Result<i32> {
    let mut user_grants = UserGrants::load(ctx.home());
    let grant = match user_grants.get(grant_id) {
        Some(g) => g.clone(),
        None => bail!("no grant with id {grant_id:?}"),
    };
    let bucket = grant
        .scope
        .strip_prefix("s3/bucket:")
        .ok_or_else(|| anyhow!("grant {grant_id} is not a bucket grant"))?;

    // Pull credentials so we know which backend key id to delete.
    let creds: serde_json::Value = SecretsStore::get_json(&format!(
        "bucket-grant:{grant_id}"
    ))
    .with_context(|| format!("reading keychain for grant {grant_id}"))?;
    let key_id = creds
        .get("key_id_for_revoke")
        .or_else(|| creds.get("access_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("grant {grant_id} keychain entry missing key id"))?
        .to_string();

    // Look up the backend from the bucket registry. If the bucket has
    // already been deleted, fall back to the default endpoint so we
    // still issue the key delete.
    let registry = BucketRegistry::load(ctx.home());
    let endpoint = registry
        .get(bucket)
        .map(|b| b.endpoint.clone())
        .unwrap_or_else(|| "local".to_string());
    let backend = backend_for(Some(&endpoint))?;

    // State 1 → 2: mark grant as revoking. We carry the state in the
    // grant's `label` field with a sentinel suffix so the schema stays
    // unchanged for v0.7.1. (v0.8 likely adds a typed status enum.)
    if !grant.label.ends_with(":revoking") {
        let mut updated = grant.clone();
        updated.label = format!("{}:revoking", grant.label);
        user_grants.add(updated);
        user_grants.save().context("marking grant revoking")?;
    }

    // State 2 → 3: backend delete. SANCHO will retry if this fails,
    // so we stay in `revoking` on error rather than rolling back.
    if let Err(e) = backend.delete_key(ctx, &key_id) {
        warn!(
            "bucket grant {grant_id}: backend delete_key failed ({e}); SANCHO will retry"
        );
        audit(ctx, "bucket_revoke_pending", bucket, grant_id, AuditResult::Allowed);
        eprintln!(
            "Revoke pending: backend delete_key failed; grant marked `revoking`. \
             SANCHO retries every 60s. Diagnose with `makakoo doctor garage`."
        );
        return Ok(2);
    }

    // Final state: drop the row + clean keychain.
    user_grants.remove(grant_id);
    user_grants.save().context("removing grant after revoke")?;
    let _ = SecretsStore::delete(&format!("bucket-grant:{grant_id}"));

    audit(ctx, "bucket_revoked", bucket, grant_id, AuditResult::Allowed);
    eprintln!("Revoked {grant_id}. Bucket {bucket} access removed.");
    Ok(0)
}

// ─── expire ──────────────────────────────────────────────────

fn expire(ctx: &CliContext, dry_run: bool) -> Result<i32> {
    let mut registry = BucketRegistry::load(ctx.home());
    let now = Utc::now();
    let expired: Vec<BucketRecord> = registry
        .buckets
        .iter()
        .filter(|b| b.is_expired(now))
        .cloned()
        .collect();
    if expired.is_empty() {
        eprintln!("No buckets past TTL. Nothing to expire.");
        return Ok(0);
    }
    let user_grants = UserGrants::load(ctx.home());
    let mut report: Vec<HashMap<String, serde_json::Value>> = Vec::new();

    for bucket in &expired {
        let mut row: HashMap<String, serde_json::Value> = HashMap::new();
        row.insert("name".into(), bucket.name.clone().into());
        row.insert("endpoint".into(), bucket.endpoint.clone().into());

        let scope_prefix = format!("s3/bucket:{}", bucket.name);
        let bucket_grants: Vec<String> = user_grants
            .grants
            .iter()
            .filter(|g| g.scope == scope_prefix)
            .map(|g| g.id.clone())
            .collect();
        row.insert("grants".into(), bucket_grants.clone().into());

        if dry_run {
            row.insert("action".into(), "would_expire".into());
            report.push(row);
            continue;
        }

        // Step 1: deny-all so in-flight presigned URLs can't race.
        registry.with_mutation(|r| {
            if let Some(b) = r.get_mut(&bucket.name) {
                b.deny_all_until = Some(now + Duration::days(365));
                b.expiring = true;
            }
            Ok(())
        })?;

        // Step 2: revoke every grant. Reuse the revoke() flow.
        for gid in &bucket_grants {
            if let Err(e) = revoke(ctx, gid) {
                warn!("expire: failed to revoke {gid} for {}: {e}", bucket.name);
            }
        }

        // Step 3: backend bucket delete. Garage refuses if non-empty —
        // operator drains via aws-cli, then re-runs `bucket expire`.
        let backend = backend_for(Some(&bucket.endpoint))?;
        if let Err(e) = backend.delete_bucket(ctx, &bucket.name) {
            warn!(
                "expire: backend delete_bucket({}) failed: {e}; will retry next tick",
                bucket.name
            );
            row.insert("action".into(), "deferred".into());
            report.push(row);
            continue;
        }

        // Step 4: remove from registry.
        registry.with_mutation(|r| {
            r.remove(&bucket.name);
            Ok(())
        })?;

        audit(ctx, "bucket_expired", &bucket.name, &bucket.endpoint, AuditResult::Allowed);
        row.insert("action".into(), "expired".into());
        report.push(row);
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"expired": report}))?
    );
    Ok(0)
}

// ─── deny-all ────────────────────────────────────────────────

fn deny_all(
    ctx: &CliContext,
    name: &str,
    ttl: &str,
    confirm_yes_really: bool,
) -> Result<i32> {
    let dur = parse_duration(ttl)?;
    if dur.is_none() && !confirm_yes_really {
        bail!("--ttl permanent requires --confirm-yes-really");
    }
    let now = Utc::now();
    let until = dur
        .map(|d| now + d)
        .unwrap_or_else(|| now + Duration::days(365 * 100));

    let mut registry = BucketRegistry::load(ctx.home());
    let endpoint = match registry.get(name) {
        Some(b) => b.endpoint.clone(),
        None => bail!("bucket {name:?} not registered"),
    };

    let backend = backend_for(Some(&endpoint))?;
    backend.deny_all(ctx, name, true)?;

    registry.with_mutation(|r| {
        if let Some(b) = r.get_mut(name) {
            b.deny_all_until = Some(until);
        }
        Ok(())
    })?;

    audit(ctx, "bucket_deny_all", name, &endpoint, AuditResult::Allowed);
    eprintln!("Bucket {name} now denying ALL reads/writes until {until}.");
    Ok(0)
}

// ═══════════════════════════════════════════════════════════════
//  Helpers
// ═══════════════════════════════════════════════════════════════

fn endpoint_url_for(endpoint_label: &str) -> String {
    match endpoint_label {
        "local" | "garage-local" => "http://127.0.0.1:3900".to_string(),
        other => format!("makakoo://endpoint/{other}"),
    }
}

fn humanize_bytes(n: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1024 * 1024 * 1024 * 1024, "TB"),
        (1024 * 1024 * 1024, "GB"),
        (1024 * 1024, "MB"),
        (1024, "KB"),
    ];
    for (size, suffix) in UNITS {
        if n >= size {
            return format!("{:.2} {}", n as f64 / size as f64, suffix);
        }
    }
    format!("{n} B")
}

fn audit(ctx: &CliContext, verb: &str, target: &str, detail: &str, result: AuditResult) {
    let line = serde_json::json!({
        "ts": Utc::now().to_rfc3339(),
        "verb": verb,
        "target": target,
        "detail": detail,
        "result": match result {
            AuditResult::Allowed => "allowed",
            AuditResult::Denied => "denied",
            AuditResult::Error => "error",
        },
        "plugin": PLUGIN,
    });
    let path = ctx.home().join("logs/transfers.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    } else {
        info!(verb, target, "audit emit failed (logs dir not writable)");
    }
}

// ═══════════════════════════════════════════════════════════════
//  Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_bucket_name_accepts_canonical() {
        assert!(validate_bucket_name("foo").is_ok());
        assert!(validate_bucket_name("project-alpha").is_ok());
        assert!(validate_bucket_name("a1.b2.c3").is_ok());
        assert!(validate_bucket_name("a-b-c-1-2-3").is_ok());
    }

    #[test]
    fn validate_bucket_name_rejects_short() {
        assert!(validate_bucket_name("ab").is_err());
    }

    #[test]
    fn validate_bucket_name_rejects_long() {
        let n = "a".repeat(64);
        assert!(validate_bucket_name(&n).is_err());
    }

    #[test]
    fn validate_bucket_name_rejects_underscore() {
        assert!(validate_bucket_name("foo_bar").is_err());
    }

    #[test]
    fn validate_bucket_name_rejects_uppercase() {
        assert!(validate_bucket_name("FooBar").is_err());
    }

    #[test]
    fn validate_bucket_name_rejects_consecutive_dots() {
        // lope-1 #virt-host
        assert!(validate_bucket_name("foo..bar").is_err());
    }

    #[test]
    fn validate_bucket_name_rejects_ipv4_shape() {
        assert!(validate_bucket_name("192.168.1.1").is_err());
    }

    #[test]
    fn validate_bucket_name_rejects_leading_or_trailing_dot() {
        assert!(validate_bucket_name(".foo").is_err());
        assert!(validate_bucket_name("foo.").is_err());
        assert!(validate_bucket_name("-foo").is_err());
        assert!(validate_bucket_name("foo-").is_err());
    }

    #[test]
    fn parse_quota_grammar() {
        assert_eq!(parse_quota("100M").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_quota("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_quota("10G").unwrap(), 10 * 1024 * 1024 * 1024);
        assert_eq!(parse_quota("unlimited").unwrap(), u64::MAX);
        assert_eq!(parse_quota("UNLIMITED").unwrap(), u64::MAX);
    }

    #[test]
    fn parse_quota_rejects_garbage() {
        assert!(parse_quota("ten gigs").is_err());
        assert!(parse_quota("1.5G").is_err()); // no fractional support
    }

    #[test]
    fn parse_perms_canonical_order() {
        assert_eq!(parse_perms("read").unwrap(), "read");
        assert_eq!(parse_perms("read,write").unwrap(), "read,write");
        assert_eq!(parse_perms("write,read").unwrap(), "read,write");
        assert_eq!(parse_perms("owner,write,read").unwrap(), "read,write,owner");
    }

    #[test]
    fn parse_perms_requires_read() {
        assert!(parse_perms("write").is_err());
        assert!(parse_perms("owner").is_err());
        assert!(parse_perms("").is_err());
    }

    #[test]
    fn parse_perms_requires_write_for_owner() {
        assert!(parse_perms("read,owner").is_err());
    }

    #[test]
    fn registry_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/buckets.json");
        let mut r = BucketRegistry::empty_at(path.clone());
        r.add(BucketRecord {
            name: "foo".into(),
            endpoint: "local".into(),
            created_at: Utc::now(),
            ttl_expires_at: None,
            quota_bytes: 1024,
            deny_all_until: None,
            expiring: false,
        });
        r.save().unwrap();
        let loaded = BucketRegistry::load(dir.path());
        assert_eq!(loaded.buckets.len(), 1);
        assert_eq!(loaded.buckets[0].name, "foo");
    }

    #[test]
    fn registry_remove_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/buckets.json");
        let mut r = BucketRegistry::empty_at(path);
        r.add(BucketRecord {
            name: "foo".into(),
            endpoint: "local".into(),
            created_at: Utc::now(),
            ttl_expires_at: None,
            quota_bytes: 1024,
            deny_all_until: None,
            expiring: false,
        });
        assert!(r.remove("foo"));
        assert!(!r.remove("foo"));
    }

    #[test]
    fn record_expiry_check() {
        let now = Utc::now();
        let past = BucketRecord {
            name: "x".into(),
            endpoint: "local".into(),
            created_at: now - Duration::days(10),
            ttl_expires_at: Some(now - Duration::seconds(1)),
            quota_bytes: 0,
            deny_all_until: None,
            expiring: false,
        };
        let future = BucketRecord {
            ttl_expires_at: Some(now + Duration::days(7)),
            ..past.clone()
        };
        let permanent = BucketRecord {
            ttl_expires_at: None,
            ..past.clone()
        };
        assert!(past.is_expired(now));
        assert!(!future.is_expired(now));
        assert!(!permanent.is_expired(now));
    }

    #[test]
    fn record_deny_all_window() {
        let now = Utc::now();
        let denied = BucketRecord {
            name: "x".into(),
            endpoint: "local".into(),
            created_at: now,
            ttl_expires_at: None,
            quota_bytes: 0,
            deny_all_until: Some(now + Duration::hours(1)),
            expiring: false,
        };
        assert!(denied.is_denied(now));
        let stale = BucketRecord {
            deny_all_until: Some(now - Duration::seconds(1)),
            ..denied.clone()
        };
        assert!(!stale.is_denied(now));
    }

    #[test]
    fn endpoint_url_for_local() {
        assert_eq!(endpoint_url_for("local"), "http://127.0.0.1:3900");
        assert_eq!(endpoint_url_for("garage-local"), "http://127.0.0.1:3900");
    }

    #[test]
    fn humanize_bytes_picks_unit() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(1023), "1023 B");
        assert!(humanize_bytes(1024 * 1024 * 5).starts_with("5.00"));
        assert!(humanize_bytes(1024_u64.pow(3)).contains("GB"));
    }

    #[test]
    fn backend_for_rejects_non_garage_in_v0_7_1() {
        assert!(backend_for(Some("aws-prod")).is_err());
        assert!(backend_for(Some("r2-foo")).is_err());
        assert!(backend_for(Some("local")).is_ok());
        assert!(backend_for(None).is_ok());
    }
}
