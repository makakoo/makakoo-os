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
//! * `tools` — each name must be lowercase alphanumeric + underscore,
//!   AND must name a tool some registered MCP handler actually serves
//!   (see `agents::tool_catalog`)
//! * `channels` — kind-specific required fields per `ChannelSpec` variant
//! * `triggers` — `cron.schedule` must be 5 valid cron fields,
//!   `webhook.path` must start with `/`
//! * `scope.allowed_paths` and `scope.forbidden_paths` — non-empty entries

use regex::Regex;

use super::{AgentSpec, ChannelSpec, ScopeSpec, TriggerSpec, NAME_REGEX};
use crate::{MakakooError, Result};

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
        return Err(MakakooError::InvalidInput("model must not be empty".into()));
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
    // A well-formed name for a tool that does not exist is worse than a
    // malformed one: `scoped_tools` filters the registry to the intersection,
    // so the slot is created, looks correct, and silently cannot do the job.
    let unknown = crate::agents::tool_catalog::unknown_tools(tools);
    if !unknown.is_empty() {
        return Err(MakakooError::InvalidInput(format!(
            concat!(
                "unknown tool(s): {}. No registered MCP handler serves {}. ",
                "Run `makakoo-mcp --list-tools` for the full list."
            ),
            unknown.join(", "),
            if unknown.len() == 1 { "it" } else { "them" }
        )));
    }
    Ok(())
}

fn validate_channel(c: &ChannelSpec) -> Result<()> {
    match c {
        ChannelSpec::Telegram {
            token_env,
            allowed_users,
        } => {
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
        ChannelSpec::Discord {
            token_env,
            allowed_users,
        } => {
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
        TriggerSpec::Cron {
            schedule, timezone, ..
        } => {
            require_non_empty(schedule, "triggers[cron].schedule")?;
            // The ONLY dialect gate. A second, hand-written grammar
            // used to run first and was stricter than the scheduler:
            // it rejected `7`, `Mon` and wrapping ranges that the
            // supervisor runs happily, so `agent create` refused
            // schedules that would have worked. One parser, one dialect.
            crate::agents::schedule::CronSchedule::parse(schedule, timezone).map_err(|e| {
                MakakooError::InvalidInput(format!("triggers[cron]: {}", plain(&e)))
            })?;
        }
        TriggerSpec::Webhook { path, secret_env } => {
            require_webhook_path(path, "triggers[webhook].path")?;
            require_env_name(secret_env, "triggers[webhook].secret_env")?;
        }
    }
    Ok(())
}

/// Strip the outer error-kind prefix so a nested message does not read
/// "invalid input: ... invalid input: ...".
fn plain(e: &MakakooError) -> String {
    let t = e.to_string();
    match t.split_once(": ") {
        Some((_, rest)) if t.starts_with("invalid input") => rest.to_string(),
        _ => t,
    }
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
    fn rejects_a_well_formed_name_for_a_tool_that_does_not_exist() {
        // The failure this closes: `web_search` passes every character rule,
        // so the slot was created and then quietly had no such tool. It was
        // the wizard's own suggested answer.
        let mut s = minimal_spec();
        s.tools = vec!["web_search".into()];
        let err = format!("{}", s.validate().unwrap_err());
        assert!(err.contains("unknown tool"), "{err}");
        assert!(err.contains("web_search"), "{err}");
        assert!(
            err.contains("--list-tools"),
            "must say how to find the real names: {err}"
        );
    }

    #[test]
    fn names_every_unknown_tool_not_just_the_first() {
        let mut s = minimal_spec();
        s.tools = vec![
            "brain_search".into(),
            "made_up_one".into(),
            "made_up_two".into(),
        ];
        let err = format!("{}", s.validate().unwrap_err());
        assert!(
            err.contains("made_up_one") && err.contains("made_up_two"),
            "{err}"
        );
        assert!(
            !err.contains("brain_search"),
            "must not blame a valid tool: {err}"
        );
    }

    #[test]
    fn accepts_per_machine_pattern_tools() {
        // Pattern tools are discovered from $MAKAKOO_HOME at boot and cannot
        // be in a compile-time catalog. Rejecting one the user has installed
        // would be a worse failure than accepting one they have not.
        let mut s = minimal_spec();
        s.tools = vec!["pattern_summarize".into()];
        s.validate().unwrap();
    }

    #[test]
    fn rejects_invalid_cron_schedule() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 */6 *".into(), // only 3 fields
            timezone: "".into(),
            prompt: String::new(),
            deliver_to: Vec::new(),
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
            prompt: String::new(),
            deliver_to: Vec::new(),
        }];
        s.validate().unwrap();
    }

    #[test]
    fn rejects_cron_field_out_of_range() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "60 * * * *".into(), // minute 60 invalid
            timezone: "".into(),
            prompt: String::new(),
            deliver_to: Vec::new(),
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
            prompt: String::new(),
            deliver_to: Vec::new(),
        }];
        s.validate().unwrap();
    }

    #[test]
    fn rejects_bogus_timezone() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 0 * * *".into(),
            timezone: "Berlin".into(),
            prompt: String::new(),
            deliver_to: Vec::new(),
        }];
        assert!(s.validate().is_err());
    }
}
