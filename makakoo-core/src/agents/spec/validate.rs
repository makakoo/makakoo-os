//! Schema-level validation for AgentSpec.
//!
//! Pure functions — no I/O, no network. Each violation produces a
//! field-level error message pointing at the offending field and value.
//!
//! Validation rules:
//!
//! * `name` — must match `^[a-z0-9][a-z0-9-]{0,62}$`
//! * `description` — non-empty after trim
//! * `model` — non-empty after trim
//! * `tools` — each name must be lowercase alphanumeric + underscore
//! * `channels` — kind-specific required fields per `ChannelSpec` variant
//! * `triggers` — `cron.schedule` must be 5 valid cron fields,
//!   `webhook.path` must start with `/`
//! * `scope.allowed_paths` and `scope.forbidden_paths` — non-empty entries

use regex::Regex;

use crate::{MakakooError, Result};
use super::{AgentSpec, ChannelSpec, ScopeSpec, TriggerSpec, NAME_REGEX};

pub fn validate(spec: &AgentSpec) -> Result<()> {
    validate_name(&spec.name)?;
    validate_description(&spec.description)?;
    validate_model(&spec.model)?;
    validate_tools(&spec.tools)?;
    for c in &spec.channels {
        validate_channel(c)?;
    }
    for t in &spec.triggers {
        validate_trigger(t)?;
    }
    validate_scope(&spec.scope)?;
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    let re = Regex::new(NAME_REGEX).expect("NAME_REGEX is valid");
    if !re.is_match(name) {
        return Err(MakakooError::InvalidInput(format!(
            "name '{}' must match {}",
            name, NAME_REGEX
        )));
    }
    Ok(())
}

fn validate_description(desc: &str) -> Result<()> {
    if desc.trim().is_empty() {
        return Err(MakakooError::InvalidInput(
            "description must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_model(model: &str) -> Result<()> {
    if model.trim().is_empty() {
        return Err(MakakooError::InvalidInput(
            "model must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_tools(tools: &[String]) -> Result<()> {
    let re = Regex::new(r"^[a-z][a-z0-9_]*$").expect("regex valid");
    for t in tools {
        if !re.is_match(t) {
            return Err(MakakooError::InvalidInput(format!(
                "tool '{}' must be lowercase alphanumeric + underscore (matches mcp__harvey__* convention)",
                t
            )));
        }
    }
    Ok(())
}

fn validate_channel(c: &ChannelSpec) -> Result<()> {
    match c {
        ChannelSpec::Telegram { token_env, allowed_users } => {
            require_env_name(token_env, "channels[telegram].token_env")?;
            validate_user_ids(allowed_users, "channels[telegram].allowed_users")?;
        }
        ChannelSpec::Slack {
            token_env,
            app_token_env,
            team_id_env,
            allowed_users,
        } => {
            require_env_name(token_env, "channels[slack].token_env")?;
            require_env_name(app_token_env, "channels[slack].app_token_env")?;
            require_env_name(team_id_env, "channels[slack].team_id_env")?;
            validate_user_ids(allowed_users, "channels[slack].allowed_users")?;
        }
        ChannelSpec::Discord { token_env, allowed_users } => {
            require_env_name(token_env, "channels[discord].token_env")?;
            validate_user_ids(allowed_users, "channels[discord].allowed_users")?;
        }
        ChannelSpec::Webhook { path, secret_env } => {
            require_webhook_path(path, "channels[webhook].path")?;
            require_env_name(secret_env, "channels[webhook].secret_env")?;
        }
        ChannelSpec::Email {
            smtp_host,
            imap_host,
            secret_env,
        } => {
            require_non_empty(smtp_host, "channels[email].smtp_host")?;
            require_non_empty(imap_host, "channels[email].imap_host")?;
            require_env_name(secret_env, "channels[email].secret_env")?;
        }
        ChannelSpec::Voice {
            twilio_account_sid_env,
            secret_env,
        } => {
            require_env_name(
                twilio_account_sid_env,
                "channels[voice].twilio_account_sid_env",
            )?;
            require_env_name(secret_env, "channels[voice].secret_env")?;
        }
    }
    Ok(())
}

fn validate_trigger(t: &TriggerSpec) -> Result<()> {
    match t {
        TriggerSpec::Cron { schedule, timezone } => {
            require_non_empty(schedule, "triggers[cron].schedule")?;
            validate_cron(schedule)?;
            if !timezone.is_empty() && !is_valid_timezone(timezone) {
                return Err(MakakooError::InvalidInput(format!(
                    "triggers[cron].timezone '{}' must be empty, 'UTC', or an IANA timezone (e.g. 'Europe/Berlin')",
                    timezone
                )));
            }
        }
        TriggerSpec::Webhook { path, secret_env } => {
            require_webhook_path(path, "triggers[webhook].path")?;
            require_env_name(secret_env, "triggers[webhook].secret_env")?;
        }
    }
    Ok(())
}

/// Validate a 5-field cron expression.
fn validate_cron(schedule: &str) -> Result<()> {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(MakakooError::InvalidInput(format!(
            "cron.schedule '{}' must have 5 space-separated fields (got {})",
            schedule,
            parts.len()
        )));
    }
    // Field ranges per standard cron (no extensions like L/W/#).
    let ranges = [(0u32, 59u32), (0, 23), (1, 31), (1, 12), (0, 6)];
    for (i, field) in parts.iter().enumerate() {
        let (lo, hi) = ranges[i];
        if !is_valid_cron_field(field, lo, hi) {
            return Err(MakakooError::InvalidInput(format!(
                "cron.schedule field {} ('{}') is not a valid cron field (range {}-{})",
                i + 1,
                field,
                lo,
                hi
            )));
        }
    }
    Ok(())
}

/// Validate a single cron field against its numeric range.
fn is_valid_cron_field(field: &str, min: u32, max: u32) -> bool {
    if field == "*" {
        return true;
    }
    // Split on optional /step first.
    let (range_part, step) = match field.split_once('/') {
        Some((r, s)) => {
            let step = match s.parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => return false,
            };
            if step > max {
                return false;
            }
            (r, step)
        }
        None => (field, 1u32),
    };
    // After stripping the step, the range can be `*` (e.g. `*/6`).
    if range_part == "*" {
        return true;
    }
    // Otherwise each comma-separated piece must be a single value or range.
    for piece in range_part.split(',') {
        if !is_valid_cron_piece(piece, min, max, step) {
            return false;
        }
    }
    true
}

fn is_valid_cron_piece(piece: &str, min: u32, max: u32, step: u32) -> bool {
    let (lo, hi) = match piece.split_once('-') {
        Some((l, h)) => {
            let lo = l.parse::<u32>().ok();
            let hi = h.parse::<u32>().ok();
            match (lo, hi) {
                (Some(lo), Some(hi)) if lo >= min && hi <= max && lo <= hi => (lo, hi),
                _ => return false,
            }
        }
        None => {
            let n = match piece.parse::<u32>() {
                Ok(n) if n >= min && n <= max => n,
                _ => return false,
            };
            (n, n)
        }
    };
    // step must divide the range (lo..=hi) evenly enough; we accept
    // any step here since invalid steps simply produce no matches at
    // runtime.
    let _ = step;
    let _ = (lo, hi);
    true
}

fn is_valid_timezone(tz: &str) -> bool {
    if tz == "UTC" || tz == "GMT" {
        return true;
    }
    // IANA timezones look like "Region/City" or "Region/Subregion/City".
    // We accept the structural shape; full validation requires the
    // `tz` crate or chrono::Tz, neither of which is currently a
    // dependency. V1 syntactic check is good enough — runtime
    // validation happens at the Flue level via `Intl.DateTimeFormat`.
    if !tz.contains('/') {
        return false;
    }
    let re = Regex::new(r"^[A-Z][A-Za-z]+(/[A-Z][A-Za-z]+)*$").expect("regex valid");
    re.is_match(tz)
}

fn validate_user_ids(ids: &[String], field: &str) -> Result<()> {
    let re = Regex::new(r"^[A-Za-z0-9_-]+$").expect("regex valid");
    for id in ids {
        if !re.is_match(id) {
            return Err(MakakooError::InvalidInput(format!(
                "{} entry '{}' contains invalid characters (allowed: alphanumeric, _, -)",
                field, id
            )));
        }
    }
    Ok(())
}

fn require_non_empty(s: &str, field: &str) -> Result<()> {
    if s.trim().is_empty() {
        return Err(MakakooError::InvalidInput(format!(
            "{} must not be empty",
            field
        )));
    }
    Ok(())
}

fn require_env_name(env: &str, field: &str) -> Result<()> {
    let re = Regex::new(r"^[A-Z][A-Z0-9_]*$").expect("regex valid");
    if !re.is_match(env) {
        return Err(MakakooError::InvalidInput(format!(
            "{} '{}' must be uppercase env var name (e.g. TELEGRAM_FOO)",
            field, env
        )));
    }
    Ok(())
}

fn require_webhook_path(path: &str, field: &str) -> Result<()> {
    require_non_empty(path, field)?;
    if !path.starts_with('/') {
        return Err(MakakooError::InvalidInput(format!(
            "{} '{}' must start with '/'",
            field, path
        )));
    }
    Ok(())
}

fn validate_scope(scope: &ScopeSpec) -> Result<()> {
    for p in &scope.allowed_paths {
        require_non_empty(p, "scope.allowed_paths")?;
    }
    for p in &scope.forbidden_paths {
        require_non_empty(p, "scope.forbidden_paths")?;
    }
    // V1: warn on exact-string overlap between forbidden and allowed.
    // Proper glob overlap detection requires the `globset` crate
    // (deferred — not currently a dependency).
    for a in &scope.allowed_paths {
        for f in &scope.forbidden_paths {
            if a == f {
                return Err(MakakooError::InvalidInput(format!(
                    "scope.allowed_paths '{}' overlaps with scope.forbidden_paths '{}'",
                    a, f
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::spec::{ChannelSpec, ScopeSpec, TriggerSpec};

    fn minimal_spec() -> AgentSpec {
        AgentSpec {
            name: "weather-bot".into(),
            description: "Monitor weather".into(),
            model: "anthropic/claude-sonnet-4-6".into(),
            instructions: "You watch weather.".into(),
            tools: vec!["brain_search".into()],
            channels: vec![],
            triggers: vec![],
            scope: ScopeSpec::default(),
        }
    }

    #[test]
    fn valid_minimal_spec() {
        minimal_spec().validate().unwrap();
    }

    #[test]
    fn rejects_invalid_name() {
        let mut s = minimal_spec();
        s.name = "Weather-Bot".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_empty_description() {
        let mut s = minimal_spec();
        s.description = "   ".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_empty_model() {
        let mut s = minimal_spec();
        s.model = "".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_uppercase_tool_name() {
        let mut s = minimal_spec();
        s.tools = vec!["BrainSearch".into()];
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("lowercase"));
    }

    #[test]
    fn accepts_mcp_harvey_double_underscore() {
        let mut s = minimal_spec();
        s.tools = vec!["mcp__harvey__brain_search".into()];
        s.validate().unwrap();
    }

    #[test]
    fn rejects_invalid_cron_schedule() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 */6 *".into(),  // only 3 fields
            timezone: "".into(),
        }];
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("5 space-separated fields"));
    }

    #[test]
    fn accepts_valid_cron_schedule() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 */6 * * *".into(),
            timezone: "UTC".into(),
        }];
        s.validate().unwrap();
    }

    #[test]
    fn rejects_cron_field_out_of_range() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "60 * * * *".into(),  // minute 60 invalid
            timezone: "".into(),
        }];
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_webhook_path_without_slash() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Webhook {
            path: "hook".into(),
            secret_env: "HOOK".into(),
        }];
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("must start with '/'"));
    }

    #[test]
    fn rejects_lowercase_env_name() {
        let mut s = minimal_spec();
        s.channels = vec![ChannelSpec::Telegram {
            token_env: "telegram_foo".into(),
            allowed_users: vec![],
        }];
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("uppercase env var name"));
    }

    #[test]
    fn rejects_scope_overlap() {
        let mut s = minimal_spec();
        s.scope = ScopeSpec {
            allowed_paths: vec!["~/MAKAKOO/data/foo/**".into()],
            forbidden_paths: vec!["~/MAKAKOO/data/foo/**".into()],
        };
        let err = s.validate().unwrap_err();
        assert!(format!("{err}").contains("overlaps"));
    }

    #[test]
    fn accepts_iana_timezone() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 0 * * *".into(),
            timezone: "Europe/Berlin".into(),
        }];
        s.validate().unwrap();
    }

    #[test]
    fn rejects_bogus_timezone() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 0 * * *".into(),
            timezone: "Berlin".into(),
        }];
        assert!(s.validate().is_err());
    }
}
