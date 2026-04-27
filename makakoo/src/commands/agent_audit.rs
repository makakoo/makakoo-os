//! `makakoo agent audit` — Phase 12 / Q14.
//!
//! Wraps `makakoo_core::agents::audit::tail_events` with a
//! human-friendly default rendering (and `--json` for scripting).

use std::path::Path;

use makakoo_core::agents::audit::{tail_events, AuditEvent, AuditKind};

use crate::context::CliContext;

pub fn run(
    ctx: &CliContext,
    last: usize,
    kind_str: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let kind = match kind_str.as_deref() {
        None => None,
        Some(s) => Some(parse_kind(s)?),
    };
    let events = tail_events(ctx.home(), last, kind)
        .map_err(|e| anyhow::anyhow!("read audit log: {e}"))?;
    if json {
        for e in &events {
            let line = serde_json::to_string(e)
                .map_err(|err| anyhow::anyhow!("serialize audit event: {err}"))?;
            println!("{line}");
        }
    } else {
        render_table(&events, ctx.home());
    }
    Ok(())
}

fn parse_kind(s: &str) -> anyhow::Result<AuditKind> {
    let lower = s.to_ascii_lowercase();
    let envelope = format!("\"{lower}\"");
    serde_json::from_str::<AuditKind>(&envelope)
        .map_err(|_| anyhow::anyhow!(
            "unknown audit kind '{s}' — see makakoo_core::agents::audit::AuditKind for accepted values"
        ))
}

fn render_table(events: &[AuditEvent], home: &Path) {
    if events.is_empty() {
        println!("(no audit events at {})", home.join("data/audit/agents.jsonl").display());
        return;
    }
    println!("{:<25} {:<24} {:<14} {:<10} {:<24} {}", "ts", "kind", "slot", "outcome", "actor", "target");
    println!("{}", "-".repeat(120));
    for e in events {
        let kind_s = serde_json::to_string(&e.kind).unwrap_or_default();
        let kind_s = kind_s.trim_matches('"');
        let outcome_s = serde_json::to_string(&e.outcome).unwrap_or_default();
        let outcome_s = outcome_s.trim_matches('"');
        println!(
            "{:<25} {:<24} {:<14} {:<10} {:<24} {}",
            e.ts.to_rfc3339(),
            kind_s,
            truncate(&e.slot_id, 14),
            outcome_s,
            truncate(e.actor.as_deref().unwrap_or("-"), 24),
            e.target.as_deref().unwrap_or("-"),
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_accepts_canonical_lowercase() {
        let k = parse_kind("scope_tool").unwrap();
        assert_eq!(k, AuditKind::ScopeTool);
    }

    #[test]
    fn parse_kind_normalizes_case() {
        let k = parse_kind("SCOPE_TOOL").unwrap();
        assert_eq!(k, AuditKind::ScopeTool);
    }

    #[test]
    fn parse_kind_rejects_unknown() {
        assert!(parse_kind("not_a_kind").is_err());
    }

    #[test]
    fn truncate_clamps_long_strings() {
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn truncate_passes_through_short_strings() {
        assert_eq!(truncate("hi", 5), "hi");
    }
}
