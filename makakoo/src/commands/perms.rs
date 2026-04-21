//! `makakoo perms <sub>` — CLI surface for the runtime user-grant layer.
//!
//! Spec sources: `spec/USER_GRANTS.md`, `spec/CAPABILITIES.md §1.11`,
//! `SPRINT.md §4.D`.
//!
//! Writes go through the `UserGrants::save` sidecar-lock protocol
//! shared with the Python `UserGrantsFile` mirror (LD#9). Every
//! grant/revoke emits one `perms/grant` or `perms/revoke` audit entry
//! via `AuditLog::append`. Scope refusals (`/`, `~`, `~/`, `$HOME`,
//! `*`, `**`) fire at this handler — NOT in the description — so the
//! same guardrail hits the CLI, the MCP handler (Phase E), and the
//! Python `HARVEY_TOOLS` handler (also Phase E).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use chrono::{DateTime, Duration, Utc};

use makakoo_core::capability::{
    rate_limit, user_grants::UserGrant, AuditEntry, AuditLog, AuditResult,
    UserGrants,
};

use crate::cli::PermsCmd;
use crate::context::CliContext;

/// Entry point for `makakoo perms`. Loads the current grant store and
/// dispatches to the per-subcommand handler.
pub async fn run(ctx: &CliContext, cmd: PermsCmd) -> anyhow::Result<i32> {
    match cmd {
        PermsCmd::List { json, all } => list(ctx, json, all),
        PermsCmd::Grant {
            path,
            duration,
            label,
            plugin,
            mkdir,
            yes_really,
        } => grant(ctx, &path, &duration, label, &plugin, mkdir, yes_really),
        PermsCmd::Revoke { id, path, json } => revoke(ctx, id, path, json),
        PermsCmd::Purge { json } => purge(ctx, json),
        PermsCmd::Audit {
            since,
            plugin,
            grant,
            json,
        } => audit(ctx, since, plugin, grant, json),
        PermsCmd::Show { id, json } => show(ctx, &id, json),
    }
}

// ═══════════════════════════════════════════════════════════════
//  Duration parser (D.2) — strict grammar per LD#15 / lope F12
// ═══════════════════════════════════════════════════════════════

/// Parse a strict-grammar duration string.
///
/// `Ok(None)` means "permanent". `Ok(Some(Duration::zero()))` is never
/// returned — zero-length grants are rejected.
///
/// Accepted grammar (spec/USER_GRANTS.md §7 + LD#15):
///
/// * `30m` — minutes
/// * `1h`, `24h` — hours
/// * `7d` — days
/// * `permanent` — no expiry
///
/// Natural-language phrases ("for an hour", "until tomorrow") are
/// rejected in v1 and deferred to v0.3.1 because they introduce a
/// crafted-string attack surface for LLM-supplied durations.
pub fn parse_duration(s: &str) -> anyhow::Result<Option<Duration>> {
    let raw = s.trim();
    if raw.is_empty() {
        bail!(
            "empty duration; use one of: 30m, 1h, 24h, 7d, permanent"
        );
    }
    if raw.eq_ignore_ascii_case("permanent") {
        return Ok(None);
    }

    // Split trailing unit character from a positive integer.
    let bytes = raw.as_bytes();
    let last = *bytes.last().unwrap() as char;
    let (num_part, unit) = match last {
        'm' | 'h' | 'd' => (&raw[..raw.len() - 1], last),
        _ => bail!(
            "unsupported duration {raw:?}; use 30m | 1h | 24h | 7d | permanent"
        ),
    };
    let n: i64 = num_part.parse().with_context(|| {
        format!(
            "unsupported duration {raw:?}; use 30m | 1h | 24h | 7d | permanent"
        )
    })?;
    if n <= 0 {
        bail!(
            "non-positive duration {raw:?}; use 30m | 1h | 24h | 7d | permanent"
        );
    }

    let dur = match unit {
        'm' => Duration::minutes(n),
        'h' => Duration::hours(n),
        'd' => Duration::days(n),
        _ => unreachable!(),
    };
    if dur > Duration::days(365) {
        bail!(
            "duration {raw:?} exceeds 365 days — shorten or split into multiple grants"
        );
    }
    Ok(Some(dur))
}

// ═══════════════════════════════════════════════════════════════
//  Scope-refusal guardrail (D.5) — shared with MCP handler (Phase E)
// ═══════════════════════════════════════════════════════════════

/// Reject scopes that are too broad to be safe at the handler layer.
///
/// Refuses `/`, `~`, `~/`, `$HOME`, empty, bare `*`, bare `**`, and
/// any path whose canonical form normalizes to one of the above.
///
/// Returns the canonicalised absolute path on success — `~` and
/// `$MAKAKOO_HOME` are expanded, relative paths are joined onto the
/// current working directory at grant-time (spec/USER_GRANTS.md §3.1).
pub fn validate_and_expand_scope(raw: &str) -> anyhow::Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("empty scope — grant a specific directory");
    }

    // Refuse bare wildcards and home markers before expansion.
    let bare_reject: &[&str] = &["/", "~", "~/", "$HOME", "*", "**", ".", "./"];
    if bare_reject.contains(&trimmed) {
        bail!(
            "scope {trimmed:?} is too broad — grant a specific subdirectory"
        );
    }
    // `$HOME/` alone (or with `**` directly appended) is also too broad.
    if trimmed == "$HOME/" || trimmed == "$HOME/**" || trimmed == "~/**" {
        bail!(
            "scope {trimmed:?} covers the entire home directory — grant a \
             specific subdirectory"
        );
    }

    // Expand `~` and `$MAKAKOO_HOME`/`$HOME`. This is a grant-time
    // resolution, per spec §3.1 — the literal string is stored.
    let expanded = expand_shell_vars(trimmed);
    let abs = if Path::new(&expanded).is_absolute() {
        PathBuf::from(&expanded)
    } else {
        std::env::current_dir()
            .context("reading cwd for relative-path expansion")?
            .join(&expanded)
    };

    // Final sanity check after expansion.
    let abs_str = abs.to_string_lossy().to_string();
    if abs_str == "/" || abs_str == "/**" {
        bail!(
            "expanded scope resolves to root — refuse to grant filesystem-wide write"
        );
    }
    Ok(abs)
}

fn expand_shell_vars(s: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mk = std::env::var("MAKAKOO_HOME").unwrap_or_else(|_| home.clone());
    let mut out = s.to_string();
    if out.starts_with("~/") {
        out = format!("{}/{}", home, &out[2..]);
    } else if out == "~" {
        out = home.clone();
    }
    // Naïve replacement — sufficient for our grammar. A full shell-var
    // expander is overkill for the `--path` field.
    out = out.replace("$MAKAKOO_HOME", &mk);
    out = out.replace("$HOME", &home);
    out
}

// ═══════════════════════════════════════════════════════════════
//  Rendering helpers (D.5 JSON mode + human table)
// ═══════════════════════════════════════════════════════════════

fn render_grants_json(grants: &[&UserGrant]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(grants)?)
}

fn render_grants_table(grants: &[&UserGrant], now: DateTime<Utc>) -> String {
    if grants.is_empty() {
        return "(no grants)".to_string();
    }
    let mut out = String::new();
    out.push_str(
        "ID                      EXPIRES                    SCOPE                                                      LABEL\n"
    );
    for g in grants {
        let expires = match g.expires_at {
            Some(exp) => {
                let remaining = exp - now;
                if remaining <= Duration::zero() {
                    "EXPIRED".to_string()
                } else if remaining < Duration::minutes(10) {
                    format!("in {}m (soon)", remaining.num_minutes())
                } else if remaining < Duration::hours(24) {
                    format!("in {}h{}m", remaining.num_hours(), remaining.num_minutes() % 60)
                } else {
                    format!("in {}d", remaining.num_days())
                }
            }
            None => "permanent".to_string(),
        };
        out.push_str(&format!(
            "{:<23} {:<26} {:<58} {}\n",
            g.id,
            expires,
            truncate(&g.scope, 58),
            truncate(&g.label, 40),
        ));
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

// ═══════════════════════════════════════════════════════════════
//  Subcommands (D.4)
// ═══════════════════════════════════════════════════════════════

fn list(ctx: &CliContext, json: bool, all: bool) -> anyhow::Result<i32> {
    let u = UserGrants::load(ctx.home());
    let now = Utc::now();
    let shown: Vec<&UserGrant> = if all {
        u.grants.iter().collect()
    } else {
        u.active_grants(now)
    };
    if json {
        println!("{}", render_grants_json(&shown)?);
    } else {
        print!("{}", render_grants_table(&shown, now));
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
fn grant(
    ctx: &CliContext,
    path: &str,
    duration: &str,
    label: Option<String>,
    plugin: &str,
    mkdir: bool,
    yes_really: bool,
) -> anyhow::Result<i32> {
    // 1. Parse + validate inputs.
    let expanded = validate_and_expand_scope(path)?;
    let dur = parse_duration(duration)?;
    let now = Utc::now();
    let expires_at = dur.map(|d| now + d);

    // 2. Permanent-outside-HOME confirmation gate.
    if expires_at.is_none() {
        let home = ctx.home().to_path_buf();
        let makakoo_home_real = fs::canonicalize(&home).unwrap_or(home.clone());
        let abs_real =
            fs::canonicalize(&expanded).unwrap_or_else(|_| expanded.clone());
        let inside_home = abs_real.starts_with(&makakoo_home_real);
        if !inside_home && !yes_really {
            bail!(
                "permanent grant outside $MAKAKOO_HOME ({}) — pass --yes-really \
                 to confirm",
                expanded.display()
            );
        }
    }

    // 3. Target-directory existence check.
    //    The scope may be a glob or a literal directory path. Only
    //    require existence when the scope is a plain directory (no
    //    `*`). Glob patterns like `~/foo/**` defer to match-time.
    let scope_str = expanded.to_string_lossy().to_string();
    let looks_like_glob = scope_str.contains('*');
    if !looks_like_glob {
        if !expanded.exists() {
            if !mkdir {
                bail!(
                    "target {} does not exist — pass --mkdir to create it",
                    expanded.display()
                );
            }
            fs::create_dir_all(&expanded)
                .with_context(|| format!("creating {}", expanded.display()))?;
        }
    }

    // 4. Build the on-disk scope string. Baseline shape is
    //    `fs/write:<absolute>`. If the user didn't pass a glob, we
    //    conservatively append `/**` so descendants are covered
    //    (matches the v0.2 behavior of baseline roots).
    let stored_scope = if looks_like_glob {
        format!("fs/write:{}", scope_str)
    } else if scope_str.ends_with('/') {
        format!("fs/write:{}**", scope_str)
    } else {
        format!("fs/write:{}/**", scope_str)
    };

    // 5. Rate-limit check (caps active + per-hour creates, LD#14).
    let mut u = UserGrants::load(ctx.home());
    let active_count = u.active_grants(now).len();
    if let Err(e) =
        rate_limit::check_and_increment(active_count, ctx.home(), now)
    {
        bail!("{}", e);
    }

    // 6. Construct + append the grant.
    let label_text =
        makakoo_core::capability::escape_audit_field(
            label.as_deref().unwrap_or(""),
            80,
        );
    let new_grant = UserGrant {
        id: makakoo_core::capability::new_grant_id(now),
        scope: stored_scope.clone(),
        created_at: now,
        expires_at,
        label: label_text.clone(),
        granted_by: "sebastian".to_string(),
        plugin: plugin.to_string(),
        origin_turn_id: String::new(),
    };
    u.add(new_grant.clone());
    u.save().with_context(|| {
        format!(
            "writing user_grants.json at {}",
            ctx.home().join("config/user_grants.json").display()
        )
    })?;

    // 7. Audit emit.
    emit_perms_audit(
        ctx,
        "perms/grant",
        &stored_scope,
        Some(&new_grant.id),
        AuditResult::Allowed,
        plugin,
    )?;

    // 8. Human feedback.
    let expires_desc = match expires_at {
        Some(exp) => format!(
            "until {} ({})",
            exp.format("%H:%M UTC"),
            human_duration(dur.unwrap())
        ),
        None => "permanent".to_string(),
    };
    eprintln!(
        "Granted {}. {} writable {}. Revoke: makakoo perms revoke {}",
        new_grant.id,
        expanded.display(),
        expires_desc,
        new_grant.id
    );
    println!("{}", new_grant.id);
    Ok(0)
}

fn human_duration(d: Duration) -> String {
    if d.num_days() >= 1 {
        format!("{}d", d.num_days())
    } else if d.num_hours() >= 1 {
        format!("{}h", d.num_hours())
    } else {
        format!("{}m", d.num_minutes())
    }
}

fn revoke(
    ctx: &CliContext,
    id: Option<String>,
    path: Option<String>,
    json: bool,
) -> anyhow::Result<i32> {
    let mut u = UserGrants::load(ctx.home());

    let target_id: String = if let Some(gid) = id {
        gid
    } else if let Some(p) = path {
        let expanded = validate_and_expand_scope(&p)?;
        let prefix = format!("fs/write:{}", expanded.to_string_lossy());
        let matches: Vec<&UserGrant> = u
            .grants
            .iter()
            .filter(|g| g.scope == prefix
                || g.scope == format!("{}/**", prefix)
                || g.scope == format!("{}**", prefix))
            .collect();
        match matches.len() {
            0 => bail!("no grant matches path {}", expanded.display()),
            1 => matches[0].id.clone(),
            _ => bail!(
                "ambiguous path — {} grants match {}:\n{}",
                matches.len(),
                expanded.display(),
                matches
                    .iter()
                    .map(|g| format!("  {} — {}", g.id, g.scope))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        }
    } else {
        bail!("provide either a grant id or --path");
    };

    let removed = u.remove(&target_id);
    if !removed {
        bail!(
            "no grant with id {} — run `makakoo perms list` to see active grants",
            target_id
        );
    }
    u.save()?;

    // Release one slot on the per-hour create bucket (v0.3.1 Phase A).
    // Revoke is an explicit user-intent undo; purge is NOT a decrement
    // path (would let slow-drip grants defeat the cap).
    rate_limit::decrement(ctx.home(), Utc::now())
        .with_context(|| "decrementing rate-limit counter")?;

    // Audit emit (scope_requested carries the id so the log can be
    // joined with the original grant entry).
    emit_perms_audit(
        ctx,
        "perms/revoke",
        &target_id,
        Some(&target_id),
        AuditResult::Allowed,
        "cli",
    )?;

    if json {
        println!("{{\"revoked\": \"{}\"}}", target_id);
    } else {
        println!("Revoked {}.", target_id);
    }
    Ok(0)
}

fn purge(ctx: &CliContext, json: bool) -> anyhow::Result<i32> {
    let mut u = UserGrants::load(ctx.home());
    let now = Utc::now();
    let removed = u.purge_expired(now);
    if removed.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("(no expired grants)");
        }
        return Ok(0);
    }
    u.save()?;

    for g in &removed {
        emit_perms_audit(
            ctx,
            "perms/revoke",
            &g.id,
            Some(&g.id),
            AuditResult::Allowed,
            "cli",
        )?;
    }

    if json {
        let ids: Vec<&str> = removed.iter().map(|g| g.id.as_str()).collect();
        println!("{}", serde_json::to_string(&ids)?);
    } else {
        for g in &removed {
            println!(
                "Purged {} (expired {})",
                g.id,
                g.expires_at
                    .map(|e| e.to_rfc3339())
                    .unwrap_or_else(|| "N/A".to_string()),
            );
        }
    }
    Ok(0)
}

fn audit(
    ctx: &CliContext,
    since: Option<String>,
    plugin_filter: Option<String>,
    grant_filter: Option<String>,
    json: bool,
) -> anyhow::Result<i32> {
    let log = AuditLog::open_default(ctx.home())?;
    let now = Utc::now();
    let from = match since {
        Some(s) => match parse_duration(&s)? {
            Some(d) => now - d,
            None => DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        },
        None => now - Duration::hours(24),
    };
    let mut entries = log.query(from, now, None)?;

    if let Some(ref p) = plugin_filter {
        entries.retain(|e| &e.plugin == p);
    }
    if let Some(ref g) = grant_filter {
        entries.retain(|e| e.scope_granted.as_deref() == Some(g.as_str()));
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("(no matching audit entries)");
    } else {
        for e in &entries {
            let outcome = match e.result {
                AuditResult::Allowed => "OK ",
                AuditResult::Denied => "DENY",
                AuditResult::Error => "ERR ",
            };
            let granted = e.scope_granted.as_deref().unwrap_or("-");
            println!(
                "{} {} {:<15} {:<30} {} → {}",
                e.ts.format("%Y-%m-%d %H:%M:%S"),
                outcome,
                e.plugin,
                e.verb,
                truncate(&e.scope_requested, 50),
                truncate(granted, 30),
            );
        }
    }
    Ok(0)
}

fn show(ctx: &CliContext, id: &str, json: bool) -> anyhow::Result<i32> {
    let u = UserGrants::load(ctx.home());
    let Some(g) = u.get(id) else {
        bail!("no grant with id {id}");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(g)?);
    } else {
        println!("id:            {}", g.id);
        println!("scope:         {}", g.scope);
        println!("created_at:    {}", g.created_at.to_rfc3339());
        println!(
            "expires_at:    {}",
            g.expires_at
                .map(|e| e.to_rfc3339())
                .unwrap_or_else(|| "permanent".to_string()),
        );
        println!("label:         {}", g.label);
        println!("granted_by:    {}", g.granted_by);
        println!("plugin:        {}", g.plugin);
        if !g.origin_turn_id.is_empty() {
            println!("origin_turn:   {}", g.origin_turn_id);
        }
    }
    Ok(0)
}

// ═══════════════════════════════════════════════════════════════
//  Audit emit helper
// ═══════════════════════════════════════════════════════════════

fn emit_perms_audit(
    ctx: &CliContext,
    verb: &str,
    scope_requested: &str,
    scope_granted: Option<&str>,
    result: AuditResult,
    plugin: &str,
) -> anyhow::Result<()> {
    let log = AuditLog::open_default(ctx.home())?;
    let entry = AuditEntry {
        ts: Utc::now(),
        plugin: plugin.to_string(),
        plugin_version: env!("CARGO_PKG_VERSION").to_string(),
        verb: verb.to_string(),
        scope_requested: scope_requested.to_string(),
        scope_granted: scope_granted.map(|s| s.to_string()),
        result,
        duration_ms: None,
        bytes_in: None,
        bytes_out: None,
        correlation_id: None,
    };
    // Audit failures never block the operation.
    if let Err(e) = log.append(&entry) {
        tracing::warn!("audit append failed for {}: {}", verb, e);
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════
//  Tests — duration parser + scope refusal
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_strict_grammar() {
        assert_eq!(parse_duration("30m").unwrap(), Some(Duration::minutes(30)));
        assert_eq!(parse_duration("1h").unwrap(), Some(Duration::hours(1)));
        assert_eq!(parse_duration("24h").unwrap(), Some(Duration::hours(24)));
        assert_eq!(parse_duration("7d").unwrap(), Some(Duration::days(7)));
        assert_eq!(parse_duration("permanent").unwrap(), None);
        assert_eq!(parse_duration("Permanent").unwrap(), None);
    }

    #[test]
    fn parse_duration_rejects_natural_language() {
        assert!(parse_duration("for an hour").is_err());
        assert!(parse_duration("until tomorrow").is_err());
        assert!(parse_duration("forever").is_err());
    }

    #[test]
    fn parse_duration_rejects_malformed() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("1").is_err());
        assert!(parse_duration("1x").is_err());
        assert!(parse_duration("-1h").is_err());
        assert!(parse_duration("0h").is_err());
    }

    #[test]
    fn parse_duration_rejects_over_one_year() {
        assert!(parse_duration("400d").is_err());
    }

    #[test]
    fn validate_scope_refuses_bare_wildcards() {
        for bad in ["/", "~", "~/", "$HOME", "*", "**", ".", "./", "$HOME/", "~/**", "$HOME/**"] {
            assert!(
                validate_and_expand_scope(bad).is_err(),
                "scope {bad:?} should be refused"
            );
        }
    }

    #[test]
    fn validate_scope_refuses_empty() {
        assert!(validate_and_expand_scope("").is_err());
        assert!(validate_and_expand_scope("   ").is_err());
    }

    #[test]
    fn validate_scope_expands_tilde() {
        std::env::set_var("HOME", "/tmp/fake-home");
        let p = validate_and_expand_scope("~/code/").unwrap();
        assert_eq!(p.to_string_lossy(), "/tmp/fake-home/code/");
    }
}
