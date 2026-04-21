//! Tier-B MCP handlers for the runtime user-grant layer.
//!
//! Pairs with the Python `HARVEY_TOOLS` entries shipped in Phase E.1b
//! — both paths share the store, the sidecar-lock protocol, and a
//! drift-gate test fixture at `tests/fixtures/grant_tool_vectors.json`.
//!
//! Three tools:
//!
//! | name | Python wrapper | behavior |
//! |---|---|---|
//! | `grant_write_access`  | `tool_grant_write_access`  | create a grant (default 1h) |
//! | `revoke_write_access` | `tool_revoke_write_access` | revoke by id or path (last/latest supported) |
//! | `list_write_grants`   | `tool_list_write_grants`   | baseline + active grants summary |
//!
//! **MCP caller identity (Phase E.3 / LD#13):** the `clientInfo.name`
//! captured at MCP `initialize` is trusted as the grant's `plugin`
//! attribution. For v0.3 we simply inherit `HARVEY_PLUGIN` from the
//! process env so the stdio child reports whatever its host exported
//! (claude-code, gemini-cli, codex, opencode, vibe, cursor, qwen, pi).
//! A formal `clientInfo` pull-through lands in v0.3.1 once the MCP
//! session struct stabilizes.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

use makakoo_core::capability::{
    is_conversational_channel, rate_limit, user_grants::UserGrant, AuditEntry,
    AuditLog, AuditResult, UserGrants, MAX_ACTIVE_GRANTS,
};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

// ═══════════════════════════════════════════════════════════════
//  Shared guardrails + grammar (mirrors perms_core.py)
// ═══════════════════════════════════════════════════════════════

const BARE_BROAD_SCOPES: &[&str] = &[
    "/", "~", "~/", "$HOME", "", "*", "**", ".", "./", "$HOME/", "~/**", "$HOME/**",
];

fn validate_and_expand_scope(raw: &str) -> Result<String, RpcError> {
    let trimmed = raw.trim();
    if BARE_BROAD_SCOPES.contains(&trimmed) {
        return Err(RpcError::invalid_params(format!(
            "scope {trimmed:?} is too broad — grant a specific subdirectory"
        )));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let mk = std::env::var("MAKAKOO_HOME").unwrap_or_else(|_| home.clone());
    let mut expanded = trimmed.to_string();
    if expanded.starts_with("~/") {
        expanded = format!("{home}/{}", &expanded[2..]);
    } else if expanded == "~" {
        expanded = home.clone();
    }
    expanded = expanded.replace("$MAKAKOO_HOME", &mk);
    expanded = expanded.replace("$HOME", &home);
    if !std::path::Path::new(&expanded).is_absolute() {
        let cwd = std::env::current_dir()
            .map_err(|e| RpcError::internal(format!("cwd: {e}")))?;
        expanded = cwd.join(&expanded).to_string_lossy().to_string();
    }
    if expanded == "/" || expanded == "/**" {
        return Err(RpcError::invalid_params(
            "expanded scope resolves to root — refuse",
        ));
    }
    Ok(expanded)
}

fn parse_duration_str(s: &str) -> Result<Option<Duration>, RpcError> {
    let raw = s.trim();
    if raw.is_empty() {
        return Err(RpcError::invalid_params(
            "empty duration; use 30m | 1h | 24h | 7d | permanent",
        ));
    }
    if raw.eq_ignore_ascii_case("permanent") {
        return Ok(None);
    }
    let bytes = raw.as_bytes();
    let last = *bytes.last().unwrap() as char;
    let (num_part, unit) = match last {
        'm' | 'h' | 'd' => (&raw[..raw.len() - 1], last),
        _ => {
            return Err(RpcError::invalid_params(format!(
                "unsupported duration {raw:?}; use 30m | 1h | 24h | 7d | permanent"
            )))
        }
    };
    let n: i64 = num_part.parse().map_err(|_| {
        RpcError::invalid_params(format!(
            "unsupported duration {raw:?}; use 30m | 1h | 24h | 7d | permanent"
        ))
    })?;
    if n <= 0 {
        return Err(RpcError::invalid_params(format!(
            "non-positive duration {raw:?}"
        )));
    }
    let dur = match unit {
        'm' => Duration::minutes(n),
        'h' => Duration::hours(n),
        'd' => Duration::days(n),
        _ => unreachable!(),
    };
    if dur > Duration::days(365) {
        return Err(RpcError::invalid_params(format!(
            "duration {raw:?} exceeds 365 days"
        )));
    }
    Ok(Some(dur))
}

fn build_stored_scope(abs_path: &str) -> String {
    if abs_path.contains('*') {
        format!("fs/write:{abs_path}")
    } else if abs_path.ends_with('/') {
        format!("fs/write:{abs_path}**")
    } else {
        format!("fs/write:{abs_path}/**")
    }
}

fn caller_plugin() -> String {
    std::env::var("HARVEY_PLUGIN").unwrap_or_else(|_| "mcp".to_string())
}

fn emit_perms_audit(
    ctx: &ToolContext,
    verb: &str,
    scope_requested: &str,
    scope_granted: Option<&str>,
    result: AuditResult,
    plugin: &str,
) {
    emit_perms_audit_with_correlation(
        ctx,
        verb,
        scope_requested,
        scope_granted,
        result,
        plugin,
        None,
    );
}

fn emit_perms_audit_with_correlation(
    ctx: &ToolContext,
    verb: &str,
    scope_requested: &str,
    scope_granted: Option<&str>,
    result: AuditResult,
    plugin: &str,
    correlation_id: Option<&str>,
) {
    if let Ok(log) = AuditLog::open_default(&ctx.home) {
        let entry = AuditEntry {
            ts: Utc::now(),
            plugin: plugin.to_string(),
            plugin_version: env!("CARGO_PKG_VERSION").to_string(),
            verb: verb.to_string(),
            scope_requested: scope_requested.to_string(),
            scope_granted: scope_granted.map(str::to_string),
            result,
            duration_ms: None,
            bytes_in: None,
            bytes_out: None,
            correlation_id: correlation_id.map(str::to_string),
        };
        let _ = log.append(&entry);
    }
}

/// v0.3.2 Phase B mirror — every grant refusal lands one
/// `result="denied"` entry with a `correlation_id="reason:<kind>"`
/// taxonomy tag before the error surfaces. Mirrors the Python
/// `_audit_grant_denial` helper in `perms_core.py`. The `path_for_audit`
/// is the user-supplied raw path (not the expanded one) so the audit
/// line shows exactly what was asked for.
fn audit_grant_denial(
    ctx: &ToolContext,
    path_for_audit: &str,
    plugin: &str,
    correlation_id: &str,
) {
    emit_perms_audit_with_correlation(
        ctx,
        "perms/grant",
        path_for_audit,
        None,
        AuditResult::Denied,
        plugin,
        Some(correlation_id),
    );
}

// ═══════════════════════════════════════════════════════════════
//  Reply templates — keep in lockstep with perms_core.py
// ═══════════════════════════════════════════════════════════════

fn grant_success_msg(g: &UserGrant) -> String {
    let expires = match g.expires_at {
        Some(t) => {
            let local = t.with_timezone(&chrono::Local);
            format!("until {}", local.format("%H:%M %Z"))
        }
        None => "permanent".to_string(),
    };
    let scope_glob = g
        .scope
        .strip_prefix("fs/write:")
        .unwrap_or(&g.scope);
    format!(
        "Granted. {scope_glob} writable {expires}. Revoke: makakoo perms revoke {id}",
        id = g.id,
    )
}

fn revoke_success_msg(grant_id: &str, scope_glob: &str) -> String {
    format!("Revoked {grant_id}. {scope_glob} no longer writable.")
}

fn list_summary_msg(
    baseline_roots: &[String],
    active: &[&UserGrant],
    expired_count: usize,
) -> String {
    let baseline_str = if baseline_roots.is_empty() {
        "(none)".to_string()
    } else {
        baseline_roots.join(", ")
    };
    if active.is_empty() {
        return format!(
            "Baseline: {baseline_str}. No active grants. {expired_count} expired today."
        );
    }
    let active_lines: Vec<String> = active
        .iter()
        .map(|g| {
            let scope = g.scope.strip_prefix("fs/write:").unwrap_or(&g.scope);
            let when = match g.expires_at {
                Some(t) => format!(
                    "until {}",
                    t.with_timezone(&chrono::Local).format("%H:%M %Z")
                ),
                None => "permanent".to_string(),
            };
            format!("{scope} {when}")
        })
        .collect();
    let plural = if active.len() == 1 { "" } else { "s" };
    format!(
        "Baseline: {baseline_str}. {count} active grant{plural}: {active_desc}. {expired_count} expired today.",
        count = active.len(),
        active_desc = active_lines.join(", "),
    )
}

fn baseline_roots_for(home: &std::path::Path) -> Vec<String> {
    vec![
        home.join("data/reports").to_string_lossy().to_string(),
        home.join("data/drafts").to_string_lossy().to_string(),
        home.join("tmp").to_string_lossy().to_string(),
        "/tmp".to_string(),
    ]
}

// ═══════════════════════════════════════════════════════════════
//  grant_write_access
// ═══════════════════════════════════════════════════════════════

pub struct GrantWriteAccessHandler {
    ctx: Arc<ToolContext>,
}

impl GrantWriteAccessHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for GrantWriteAccessHandler {
    fn name(&self) -> &str {
        "grant_write_access"
    }
    fn description(&self) -> &str {
        "Grant Harvey temporary write access to a file path. DEFAULT duration is 1 \
         hour — don't ask for a longer grant unless the user explicitly says so. \
         When the user says 'grant yourself access to X', 'let me edit Y for an \
         hour', or similar — call this with the user-specified duration or the \
         default 1h. On a write_file rejection, ask the user ('Want me to grant \
         myself 1h access?'), then call this on 'yes'. NEVER call without \
         user-facing confirmation. Quote the return string verbatim."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "duration": {"type": "string", "enum": ["30m", "1h", "24h", "7d", "permanent"]},
                "label": {"type": "string"},
                "confirm": {"type": "string"},
                "user_turn_id": {"type": "string"}
            },
            "required": ["path"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing 'path'"))?;
        let duration = params
            .get("duration")
            .and_then(|v| v.as_str())
            .unwrap_or("1h");
        let label = params.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let confirm = params
            .get("confirm")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let user_turn_id = params
            .get("user_turn_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let plugin = caller_plugin();

        // v0.3.2 Phase C — conversational channels require non-empty
        // origin_turn_id. Fail-fast before any other check so
        // prompt-injected calls show up in audit with a provenance
        // signal, not a guardrail one. Mirrors the Python check in
        // `perms_core.do_grant`. CLI + sancho-native + unknown plugins
        // bypass by design — they don't carry a human turn.
        if is_conversational_channel(&plugin) && user_turn_id.is_empty() {
            audit_grant_denial(
                &self.ctx,
                path,
                &plugin,
                "reason:missing_origin_turn_id",
            );
            return Err(RpcError::invalid_params(format!(
                "origin_turn_id required on conversational channels \
                 (plugin={plugin}); this grant call appears to be \
                 agent-initiated without a human turn binding"
            )));
        }

        let abs = match validate_and_expand_scope(path) {
            Ok(a) => a,
            Err(e) => {
                audit_grant_denial(&self.ctx, path, &plugin, "reason:too_broad");
                return Err(e);
            }
        };
        let dur = match parse_duration_str(duration) {
            Ok(d) => d,
            Err(e) => {
                audit_grant_denial(
                    &self.ctx,
                    path,
                    &plugin,
                    "reason:bad_duration",
                );
                return Err(e);
            }
        };
        let now = Utc::now();
        let expires_at: Option<DateTime<Utc>> = dur.map(|d| now + d);

        // Permanent-outside-HOME confirm gate.
        if expires_at.is_none() {
            let home_real =
                std::fs::canonicalize(&self.ctx.home).unwrap_or_else(|_| self.ctx.home.clone());
            let path_real =
                std::fs::canonicalize(&abs).unwrap_or_else(|_| std::path::PathBuf::from(&abs));
            let inside_home = path_real.starts_with(&home_real);
            if !inside_home && confirm != "yes-really" {
                audit_grant_denial(
                    &self.ctx,
                    path,
                    &plugin,
                    "reason:permanent_outside_home_unconfirmed",
                );
                return Err(RpcError::invalid_params(format!(
                    "permanent grant outside $MAKAKOO_HOME ({abs}) requires confirm=\"yes-really\""
                )));
            }
        }

        // Rate-limit check + store mutate.
        let mut grants = UserGrants::load(&self.ctx.home);
        let active_count = grants.active_grants(now).len();
        // Pre-derive the rate-limit denial reason: if the active cap
        // is already at/above the limit, `check_and_increment` bails
        // on that branch (creates_in_window is not incremented). Any
        // other failure is the per-hour cap. Mirrors Python's
        // `e.creates_in_window == 0` discriminator.
        if let Err(e) = rate_limit::check_and_increment(active_count, &self.ctx.home, now) {
            let reason = if active_count >= MAX_ACTIVE_GRANTS {
                "reason:rate_limit_active"
            } else {
                "reason:rate_limit_hourly"
            };
            audit_grant_denial(&self.ctx, path, &plugin, reason);
            return Err(RpcError::invalid_params(e.to_string()));
        }

        let new_grant = UserGrant {
            id: makakoo_core::capability::new_grant_id(now),
            scope: build_stored_scope(&abs),
            created_at: now,
            expires_at,
            label: makakoo_core::capability::escape_audit_field(label, 80),
            granted_by: "sebastian".to_string(),
            plugin: plugin.clone(),
            origin_turn_id: user_turn_id.to_string(),
            // v0.3.3 — owner captures the caller's plugin. Revoke side
            // (`RevokeWriteAccessHandler`) refuses unless the revoke
            // caller matches OR is on the admin bypass list.
            owner: plugin.clone(),
        };
        grants.add(new_grant.clone());
        grants
            .save()
            .map_err(|e| RpcError::internal(format!("save user_grants: {e}")))?;

        emit_perms_audit(
            &self.ctx,
            "perms/grant",
            &new_grant.scope,
            Some(&new_grant.id),
            AuditResult::Allowed,
            &plugin,
        );

        Ok(json!({ "reply": grant_success_msg(&new_grant), "grant_id": new_grant.id }))
    }
}

// ═══════════════════════════════════════════════════════════════
//  revoke_write_access
// ═══════════════════════════════════════════════════════════════

pub struct RevokeWriteAccessHandler {
    ctx: Arc<ToolContext>,
}

impl RevokeWriteAccessHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for RevokeWriteAccessHandler {
    fn name(&self) -> &str {
        "revoke_write_access"
    }
    fn description(&self) -> &str {
        "Revoke a runtime user grant. Accepts grant_id or path. Path 'last' / \
         'latest' resolves to the most recently-created active grant. Quote the \
         return string verbatim."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "grant_id": {"type": "string"},
                "path": {"type": "string"}
            },
            "required": []
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let grant_id = params
            .get("grant_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        if grant_id.is_none() && path.is_none() {
            return Err(RpcError::invalid_params(
                "provide either grant_id or path",
            ));
        }

        let mut grants = UserGrants::load(&self.ctx.home);
        let now = Utc::now();

        let target_id: String = if let Some(gid) = grant_id {
            if grants.get(&gid).is_none() {
                return Err(RpcError::invalid_params(format!(
                    "no grant with id {gid}; run list_write_grants to see active grants"
                )));
            }
            gid
        } else {
            let p = path.unwrap();
            let lower = p.trim().to_lowercase();
            let active: Vec<&UserGrant> = grants.active_grants(now);
            if lower == "last" || lower == "latest" {
                let newest = active
                    .iter()
                    .max_by_key(|g| g.created_at)
                    .copied()
                    .ok_or_else(|| {
                        RpcError::invalid_params("no active grants to revoke")
                    })?;
                newest.id.clone()
            } else {
                let abs = validate_and_expand_scope(&p)?;
                let prefix = format!("fs/write:{abs}");
                let candidates: Vec<&UserGrant> = active
                    .iter()
                    .filter(|g| {
                        g.scope == prefix
                            || g.scope == format!("{prefix}/**")
                            || g.scope == format!("{prefix}**")
                    })
                    .copied()
                    .collect();
                match candidates.len() {
                    0 => {
                        return Err(RpcError::invalid_params(format!(
                            "no grant matches path {abs}"
                        )));
                    }
                    1 => candidates[0].id.clone(),
                    _ => {
                        let ids = candidates
                            .iter()
                            .map(|g| g.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Err(RpcError::invalid_params(format!(
                            "ambiguous path — {} grants match: {ids}",
                            candidates.len()
                        )));
                    }
                }
            }
        };

        // v0.3.3 Phase A — ownership gate. A caller may revoke a grant
        // only if its plugin matches the grant's `owner` OR if it is
        // an admin bypass (`cli` / `sancho-native`). Prevents a
        // compromised skill from wiping another agent's active grants.
        let caller = caller_plugin();
        let is_admin_bypass = caller == "cli" || caller == "sancho-native";
        let target_owner = grants
            .get(&target_id)
            .map(|g| g.owner.clone())
            .unwrap_or_default();
        if !is_admin_bypass && target_owner != caller {
            emit_perms_audit_with_correlation(
                &self.ctx,
                "perms/revoke",
                &target_id,
                None,
                AuditResult::Denied,
                &caller,
                Some("reason:not_owner"),
            );
            return Err(RpcError::invalid_params(format!(
                "revoke refused: grant {target_id} is owned by {target_owner:?}, \
                 not {caller:?}. Only the creating plugin or an admin caller (cli) \
                 may revoke."
            )));
        }

        // Stable view of the scope before we drop it from the store.
        let scope_glob = grants
            .get(&target_id)
            .map(|g| {
                g.scope
                    .strip_prefix("fs/write:")
                    .unwrap_or(&g.scope)
                    .to_string()
            })
            .unwrap_or_default();
        let removed = grants.remove(&target_id);
        if !removed {
            return Err(RpcError::internal(format!(
                "race: grant {target_id} already removed"
            )));
        }
        grants
            .save()
            .map_err(|e| RpcError::internal(format!("save user_grants: {e}")))?;

        emit_perms_audit(
            &self.ctx,
            "perms/revoke",
            &target_id,
            Some(&target_id),
            AuditResult::Allowed,
            &caller_plugin(),
        );

        Ok(json!({
            "reply": revoke_success_msg(&target_id, &scope_glob),
            "grant_id": target_id
        }))
    }
}

// ═══════════════════════════════════════════════════════════════
//  list_write_grants
// ═══════════════════════════════════════════════════════════════

pub struct ListWriteGrantsHandler {
    ctx: Arc<ToolContext>,
}

impl ListWriteGrantsHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for ListWriteGrantsHandler {
    fn name(&self) -> &str {
        "list_write_grants"
    }
    fn description(&self) -> &str {
        "List baseline write directories + active runtime grants. Returns a \
         short summary string quotable verbatim. Set include_expired=true to \
         show today's expired grants too."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_expired": {"type": "boolean"}
            },
            "required": []
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let include_expired = params
            .get("include_expired")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let grants = UserGrants::load(&self.ctx.home);
        let now = Utc::now();
        let baseline = baseline_roots_for(&self.ctx.home);
        let today_midnight = DateTime::<Utc>::from_naive_utc_and_offset(
            chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            Utc,
        );

        let active: Vec<&UserGrant> = grants.active_grants(now);
        let expired_today: Vec<&UserGrant> = grants
            .grants
            .iter()
            .filter(|g| match g.expires_at {
                Some(e) => e >= today_midnight && e < now,
                None => false,
            })
            .collect();

        let summary = if include_expired {
            let mut shown: Vec<&UserGrant> = active.clone();
            shown.extend(expired_today.iter().copied());
            list_summary_msg(&baseline, &shown, 0)
        } else {
            list_summary_msg(&baseline, &active, expired_today.len())
        };

        let active_json: Vec<Value> = active
            .iter()
            .map(|g| serde_json::to_value(g).unwrap())
            .collect();

        Ok(json!({
            "reply": summary,
            "baseline": baseline,
            "active": active_json,
            "expired_today_count": expired_today.len()
        }))
    }
}

use chrono::Datelike;

// ═══════════════════════════════════════════════════════════════
//  Tests — drive the shared fixture identical to the Python mirror
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const FIXTURE_JSON: &str =
        include_str!("../../../../tests/fixtures/grant_tool_vectors.json");

    struct Harness {
        _tmp: TempDir,
        home: std::path::PathBuf,
        ctx: Arc<ToolContext>,
        // `_lock` serialises any test touching HARVEY_PLUGIN /
        // MAKAKOO_HOME env vars against every other test in this
        // module. The env is process-global — without this, Rust's
        // default parallel runner races plugin attribution across
        // concurrent handler invocations.
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Harness {
        fn new() -> Self {
            let lock = env_lock();
            let tmp = TempDir::new().unwrap();
            let home = std::fs::canonicalize(tmp.path()).unwrap();
            for d in ["config", "state", "logs"] {
                std::fs::create_dir_all(home.join(d)).unwrap();
            }
            // Isolate env — no HARVEY_PLUGIN (default "mcp") and no
            // HARVEY_TELEGRAM_CHAT_ID so the allowlist gate short-circuits.
            std::env::set_var("HOME", &home);
            std::env::set_var("MAKAKOO_HOME", &home);
            std::env::remove_var("HARVEY_PLUGIN");
            std::env::remove_var("HARVEY_TELEGRAM_CHAT_ID");
            let ctx = Arc::new(ToolContext::empty(home.clone()));
            Self {
                _tmp: tmp,
                home,
                ctx,
                _lock: lock,
            }
        }

        fn sub(&self, v: &Value) -> Value {
            let h = self.home.to_string_lossy().to_string();
            substitute_home(v, &h)
        }
    }

    /// Guard: serialise tests that mutate `HARVEY_PLUGIN` or rely on
    /// a predictable env state. Every Harness::new() acquires this;
    /// plain `#[test]` functions that don't use Harness should also
    /// acquire it if they touch env.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn substitute_home(v: &Value, home: &str) -> Value {
        match v {
            Value::String(s) => Value::String(s.replace("<HOME>", home)),
            Value::Object(m) => {
                let mut out = serde_json::Map::new();
                for (k, v) in m {
                    out.insert(k.clone(), substitute_home(v, home));
                }
                Value::Object(out)
            }
            Value::Array(a) => {
                Value::Array(a.iter().map(|x| substitute_home(x, home)).collect())
            }
            other => other.clone(),
        }
    }

    fn reply_of(v: &Value) -> Option<&str> {
        v.get("reply").and_then(Value::as_str)
    }

    /// Run one scenario in an isolated Harness — grants from one
    /// scenario must not leak into the next.
    async fn run_scenario(sc: &Value) -> String {
        let name = sc["name"].as_str().unwrap_or("<unnamed>");
        let h = Harness::new();

        let mut seed_id: Option<String> = None;

        if let Some(seed) = sc.get("seed_grant") {
            let grant_h = GrantWriteAccessHandler::new(h.ctx.clone());
            let mut seed_args = h.sub(seed);
            // mkdir_target handled here too.
            if let Some(path) = seed_args.get("path").and_then(Value::as_str) {
                let _ = std::fs::create_dir_all(path);
            }
            if let Some(obj) = seed_args.as_object_mut() {
                obj.remove("mkdir_target");
            }
            match grant_h.call(seed_args).await {
                Ok(v) => {
                    seed_id = v
                        .get("grant_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                Err(e) => {
                    return format!("{name}: seed_grant failed: {e:?}");
                }
            }
        }

        let raw_in = sc.get("input").or_else(|| sc.get("input_from_seed"));
        let mut args = match raw_in {
            Some(v) => h.sub(v),
            None => serde_json::json!({}),
        };
        // Substitute $GRANT_ID placeholder.
        if let Some(obj) = args.as_object_mut() {
            let sid = seed_id.clone();
            for (_, val) in obj.iter_mut() {
                if let Some(s) = val.as_str() {
                    if s == "$GRANT_ID" {
                        if let Some(id) = &sid {
                            *val = Value::String(id.clone());
                        }
                    }
                }
            }
            // mkdir_target path handling
            let mkdir = obj.get("mkdir_target").and_then(Value::as_bool).unwrap_or(false);
            if mkdir {
                if let Some(path) = obj.get("path").and_then(Value::as_str) {
                    let _ = std::fs::create_dir_all(path);
                }
                obj.remove("mkdir_target");
            }
        }

        let tool = sc["tool"].as_str().unwrap_or_default();
        let result = match tool {
            "grant_write_access" => {
                GrantWriteAccessHandler::new(h.ctx.clone()).call(args).await
            }
            "revoke_write_access" => {
                RevokeWriteAccessHandler::new(h.ctx.clone()).call(args).await
            }
            "list_write_grants" => {
                ListWriteGrantsHandler::new(h.ctx.clone()).call(args).await
            }
            other => return format!("{name}: unknown tool {other}"),
        };

        let expect_ok = sc["expect"]["ok"].as_bool().unwrap_or(false);
        let (ok, reply_text) = match &result {
            Ok(v) => (
                true,
                reply_of(v).map(str::to_string).unwrap_or_default(),
            ),
            Err(e) => (false, format!("{e:?}")),
        };
        if ok != expect_ok {
            return format!(
                "{name}: expected ok={expect_ok}, got ok={ok}; reply={reply_text:?}"
            );
        }
        let contains = sc["expect"]["reply_contains"].as_array().cloned().unwrap_or_default();
        for c in contains {
            let s = c.as_str().unwrap_or_default();
            if !reply_text.contains(s) {
                return format!("{name}: reply missing {s:?}; reply={reply_text:?}");
            }
        }
        String::new()
    }

    #[tokio::test]
    async fn shared_fixture_drift_gate() {
        let fixture: Value = serde_json::from_str(FIXTURE_JSON).expect("valid fixture");
        let scenarios = fixture["scenarios"].as_array().expect("scenarios array");
        let mut failures: Vec<String> = Vec::new();
        for sc in scenarios {
            let err = run_scenario(sc).await;
            if !err.is_empty() {
                failures.push(err);
            }
        }
        assert!(
            failures.is_empty(),
            "{}/{} scenarios failed:\n  - {}",
            failures.len(),
            scenarios.len(),
            failures.join("\n  - ")
        );
    }

    #[tokio::test]
    async fn rate_limit_surfaces_through_mcp_handler() {
        let h = Harness::new();
        let grant_h = GrantWriteAccessHandler::new(h.ctx.clone());
        for i in 0..makakoo_core::capability::MAX_ACTIVE_GRANTS {
            let target = h.home.join(format!("burst-{i}"));
            std::fs::create_dir_all(&target).unwrap();
            let args = serde_json::json!({
                "path": target.to_string_lossy(),
                "duration": "1h"
            });
            let result = grant_h.call(args).await;
            assert!(result.is_ok(), "burst {i} failed: {result:?}");
        }
        let over = h.home.join("burst-over");
        std::fs::create_dir_all(&over).unwrap();
        let args = serde_json::json!({
            "path": over.to_string_lossy(),
            "duration": "1h"
        });
        let err = grant_h.call(args).await.unwrap_err();
        assert!(
            format!("{err:?}").contains("rate limit"),
            "expected rate limit, got {err:?}"
        );
    }

    // ═══════════════════════════════════════════════════════════
    //  v0.3.2 Phase B — denial-audit wrapping (reason: taxonomy)
    // ═══════════════════════════════════════════════════════════

    /// Parse `<home>/logs/audit.jsonl` into a Vec<Value>. Lines that
    /// don't deserialize are dropped — matches the Python helper.
    fn read_audit(home: &std::path::Path) -> Vec<Value> {
        let p = home.join("logs/audit.jsonl");
        let Ok(data) = std::fs::read_to_string(&p) else {
            return Vec::new();
        };
        data.lines()
            .filter_map(|line| serde_json::from_str(line.trim()).ok())
            .collect()
    }

    fn last_denial<'a>(entries: &'a [Value]) -> Option<&'a Value> {
        entries
            .iter()
            .rev()
            .find(|e| e.get("result").and_then(Value::as_str) == Some("denied"))
    }

    #[tokio::test]
    async fn phase_b_too_broad_denial_audited() {
        let h = Harness::new();
        let err = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(json!({"path": "/"}))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("too broad"));
        let entries = read_audit(&h.home);
        let d = last_denial(&entries).expect("denial entry not written");
        assert_eq!(d["verb"].as_str(), Some("perms/grant"));
        assert_eq!(
            d["correlation_id"].as_str(),
            Some("reason:too_broad"),
            "{d:?}"
        );
        assert!(d["scope_granted"].is_null());
        assert_eq!(d["scope_requested"].as_str(), Some("/"));
    }

    #[tokio::test]
    async fn phase_b_bad_duration_denial_audited() {
        let h = Harness::new();
        let tgt = h.home.join("phase-b-dur");
        std::fs::create_dir_all(&tgt).unwrap();
        let err = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(json!({"path": tgt.to_string_lossy(), "duration": "forever"}))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("unsupported duration"));
        let entries = read_audit(&h.home);
        let d = last_denial(&entries).expect("denial entry not written");
        assert_eq!(
            d["correlation_id"].as_str(),
            Some("reason:bad_duration"),
            "{d:?}"
        );
    }

    #[tokio::test]
    async fn phase_b_permanent_outside_home_denial_audited() {
        let h = Harness::new();
        // tmpdir outside the isolated HOME — `/tmp/phase-b-perm-<pid>`
        let outside = std::env::temp_dir()
            .join(format!("phase-b-perm-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        let outside_str = outside.to_string_lossy().to_string();

        let err = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(json!({
                "path": outside_str.clone(),
                "duration": "permanent"
            }))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("yes-really"));
        let entries = read_audit(&h.home);
        let d = last_denial(&entries).expect("denial entry not written");
        assert_eq!(
            d["correlation_id"].as_str(),
            Some("reason:permanent_outside_home_unconfirmed"),
            "{d:?}"
        );
        std::fs::remove_dir_all(&outside).ok();
    }

    #[tokio::test]
    async fn phase_b_rate_limit_active_denial_audited() {
        let h = Harness::new();
        let grant_h = GrantWriteAccessHandler::new(h.ctx.clone());
        for i in 0..MAX_ACTIVE_GRANTS {
            let tgt = h.home.join(format!("active-seed-{i}"));
            std::fs::create_dir_all(&tgt).unwrap();
            grant_h
                .call(json!({
                    "path": tgt.to_string_lossy(),
                    "duration": "1h"
                }))
                .await
                .unwrap();
        }
        let over = h.home.join("over");
        std::fs::create_dir_all(&over).unwrap();
        let err = grant_h
            .call(json!({"path": over.to_string_lossy(), "duration": "1h"}))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("active grants"));
        let entries = read_audit(&h.home);
        let d = last_denial(&entries).expect("denial entry not written");
        assert_eq!(
            d["correlation_id"].as_str(),
            Some("reason:rate_limit_active"),
            "{d:?}"
        );
    }

    // ═══════════════════════════════════════════════════════════
    //  v0.3.2 Phase C — origin_turn_id enforcement
    // ═══════════════════════════════════════════════════════════

    #[tokio::test]
    async fn phase_c_rejects_conversational_without_origin_turn_id() {
        let h = Harness::new();
        std::env::set_var("HARVEY_PLUGIN", "claude-code");
        let tgt = h.home.join("phase-c-conv");
        std::fs::create_dir_all(&tgt).unwrap();
        let err = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(json!({
                "path": tgt.to_string_lossy(),
                "duration": "1h"
            }))
            .await
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("origin_turn_id required"),
            "{err:?}"
        );
        let entries = read_audit(&h.home);
        let d = last_denial(&entries).expect("denial entry not written");
        assert_eq!(
            d["correlation_id"].as_str(),
            Some("reason:missing_origin_turn_id"),
            "{d:?}"
        );
        assert_eq!(d["plugin"].as_str(), Some("claude-code"), "{d:?}");
        std::env::remove_var("HARVEY_PLUGIN");
    }

    #[tokio::test]
    async fn phase_c_accepts_conversational_with_origin_turn_id() {
        let h = Harness::new();
        std::env::set_var("HARVEY_PLUGIN", "gemini-cli");
        let tgt = h.home.join("phase-c-conv-ok");
        std::fs::create_dir_all(&tgt).unwrap();
        let out = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(json!({
                "path": tgt.to_string_lossy(),
                "duration": "1h",
                "user_turn_id": "t_abc123"
            }))
            .await
            .unwrap();
        assert!(out["reply"].as_str().unwrap_or("").contains("Granted."));
        std::env::remove_var("HARVEY_PLUGIN");
    }

    #[tokio::test]
    async fn phase_c_cli_bypasses_origin_turn_id_gate() {
        let h = Harness::new();
        std::env::set_var("HARVEY_PLUGIN", "cli");
        let tgt = h.home.join("phase-c-cli");
        std::fs::create_dir_all(&tgt).unwrap();
        let out = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(json!({
                "path": tgt.to_string_lossy(),
                "duration": "1h"
            }))
            .await
            .unwrap();
        assert!(out["reply"].as_str().unwrap_or("").contains("Granted."));
        std::env::remove_var("HARVEY_PLUGIN");
    }

    // ═══════════════════════════════════════════════════════════
    //  v0.3.3 Phase A — grant ownership check on revoke
    // ═══════════════════════════════════════════════════════════

    /// Seed one grant with the given owner plugin into the temp home,
    /// return its id.
    async fn seed_grant_with_owner(h: &Harness, owner: &str) -> String {
        std::env::set_var("HARVEY_PLUGIN", owner);
        let tgt = h.home.join(format!("owner-{owner}"));
        std::fs::create_dir_all(&tgt).unwrap();
        let args = json!({
            "path": tgt.to_string_lossy(),
            "duration": "1h",
            "user_turn_id": "t_test_owner",
        });
        let out = GrantWriteAccessHandler::new(h.ctx.clone())
            .call(args)
            .await
            .unwrap();
        std::env::remove_var("HARVEY_PLUGIN");
        out["grant_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn phase_a_owner_can_revoke_own_grant() {
        let h = Harness::new();
        let id = seed_grant_with_owner(&h, "claude-code").await;
        std::env::set_var("HARVEY_PLUGIN", "claude-code");
        let out = RevokeWriteAccessHandler::new(h.ctx.clone())
            .call(json!({"grant_id": id}))
            .await
            .unwrap();
        assert!(out["reply"].as_str().unwrap_or("").contains("Revoked"));
        std::env::remove_var("HARVEY_PLUGIN");
    }

    #[tokio::test]
    async fn phase_a_cross_plugin_revoke_refused() {
        let h = Harness::new();
        let id = seed_grant_with_owner(&h, "claude-code").await;
        std::env::set_var("HARVEY_PLUGIN", "gemini-cli");
        let err = RevokeWriteAccessHandler::new(h.ctx.clone())
            .call(json!({"grant_id": id}))
            .await
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("revoke refused"),
            "{err:?}"
        );
        let entries = read_audit(&h.home);
        let d = last_denial(&entries).expect("denial entry");
        assert_eq!(d["verb"].as_str(), Some("perms/revoke"));
        assert_eq!(
            d["correlation_id"].as_str(),
            Some("reason:not_owner"),
            "{d:?}"
        );
        assert_eq!(d["plugin"].as_str(), Some("gemini-cli"), "{d:?}");
        std::env::remove_var("HARVEY_PLUGIN");
    }

    #[tokio::test]
    async fn phase_a_cli_admin_bypass_revokes_any_grant() {
        let h = Harness::new();
        let id = seed_grant_with_owner(&h, "gemini-cli").await;
        std::env::set_var("HARVEY_PLUGIN", "cli");
        let out = RevokeWriteAccessHandler::new(h.ctx.clone())
            .call(json!({"grant_id": id}))
            .await
            .unwrap();
        assert!(out["reply"].as_str().unwrap_or("").contains("Revoked"));
        std::env::remove_var("HARVEY_PLUGIN");
    }

    #[tokio::test]
    async fn phase_a_sancho_native_admin_bypass() {
        let h = Harness::new();
        let id = seed_grant_with_owner(&h, "harveychat").await;
        std::env::set_var("HARVEY_PLUGIN", "sancho-native");
        let out = RevokeWriteAccessHandler::new(h.ctx.clone())
            .call(json!({"grant_id": id}))
            .await
            .unwrap();
        assert!(out["reply"].as_str().unwrap_or("").contains("Revoked"));
        std::env::remove_var("HARVEY_PLUGIN");
    }

    #[tokio::test]
    async fn phase_a_shared_fixture_vectors() {
        let fixture_bytes = include_bytes!(
            "../../../../plugins-core/lib-harvey-core/tests/fixtures/grant_ownership_vectors.json"
        );
        let fixture: Value =
            serde_json::from_slice(fixture_bytes).expect("valid fixture");
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap().to_string();
            let owner = case["grant_owner"].as_str().unwrap().to_string();
            let caller =
                case["revoke_caller_plugin"].as_str().unwrap().to_string();
            let expected = case["expected"].as_str().unwrap().to_string();

            let h = Harness::new();
            let id = seed_grant_with_owner(&h, &owner).await;
            std::env::set_var("HARVEY_PLUGIN", &caller);
            let result = RevokeWriteAccessHandler::new(h.ctx.clone())
                .call(json!({"grant_id": id}))
                .await;
            std::env::remove_var("HARVEY_PLUGIN");
            match (expected.as_str(), result) {
                ("allow", Ok(_)) => {}
                ("reject", Err(e)) => {
                    assert!(
                        format!("{e:?}").contains("revoke refused"),
                        "{name}: expected ownership refusal, got {e:?}"
                    );
                }
                ("allow", Err(e)) => panic!("{name}: expected allow, got {e:?}"),
                ("reject", Ok(v)) => panic!(
                    "{name}: expected reject, got success: {}",
                    v
                ),
                (other, _) => panic!("{name}: unknown expected {other:?}"),
            }
        }
    }

    // ═══════════════════════════════════════════════════════════
    //  v0.3.2 drift-gate — CONVERSATIONAL_CHANNELS shared fixture
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn conversational_channels_fixture_matches_rust_const() {
        use makakoo_core::capability::CONVERSATIONAL_CHANNELS;
        let bytes = include_bytes!(
            "../../../../plugins-core/lib-harvey-core/tests/fixtures/conversational_channels.json"
        );
        let fixture: Value = serde_json::from_slice(bytes).expect("valid fixture");
        let fixture_set: std::collections::BTreeSet<&str> = fixture["channels"]
            .as_array()
            .expect("channels array")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        let rust_set: std::collections::BTreeSet<&str> =
            CONVERSATIONAL_CHANNELS.iter().copied().collect();
        assert_eq!(
            fixture_set, rust_set,
            "Rust CONVERSATIONAL_CHANNELS diverged from shared fixture.\n\
             Only in fixture: {:?}\nOnly in Rust: {:?}",
            fixture_set.difference(&rust_set).collect::<Vec<_>>(),
            rust_set.difference(&fixture_set).collect::<Vec<_>>()
        );
        // Sanity on the negative examples too — none should be in the set.
        for non in fixture["non_conversational_examples"]
            .as_array()
            .expect("non_conversational_examples array")
        {
            let name = non.as_str().unwrap();
            assert!(
                !is_conversational_channel(name),
                "{name} should NOT be a conversational channel"
            );
        }
    }

    #[tokio::test]
    async fn list_handler_structured_json_shape() {
        let h = Harness::new();
        let zone = h.home.join("listzone-shape");
        std::fs::create_dir_all(&zone).unwrap();
        GrantWriteAccessHandler::new(h.ctx.clone())
            .call(serde_json::json!({
                "path": zone.to_string_lossy(),
                "duration": "1h"
            }))
            .await
            .unwrap();
        let v = ListWriteGrantsHandler::new(h.ctx.clone())
            .call(serde_json::json!({}))
            .await
            .unwrap();
        assert!(v.get("reply").is_some());
        assert!(v.get("baseline").is_some());
        assert!(v.get("active").and_then(Value::as_array).map_or(false, |a| a.len() == 1));
        assert_eq!(v.get("expired_today_count").and_then(Value::as_u64), Some(0));
    }
}
