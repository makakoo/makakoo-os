//! YAML + TOML parser for AgentSpec.
//!
//! Auto-detects format from file extension (`.yaml`, `.yml`, `.toml`).
//! All parse errors are wrapped with file path and the underlying
//! serializer's line/column information so callers can produce
//! field-level diagnostics.

use std::fs;
use std::path::Path;

use super::AgentSpec;
use crate::{MakakooError, Result};

/// Parse a spec from a YAML or TOML string.
///
/// Format is selected by extension. Returns an error with file path
/// context for use by `load_from_file`.
pub fn parse_str(raw: &str, extension_hint: Option<&str>) -> Result<AgentSpec> {
    let ext = extension_hint.unwrap_or("");
    match ext {
        "yaml" | "yml" => serde_yaml_ng::from_str::<AgentSpec>(raw)
            .map_err(|e| MakakooError::Config(format!("YAML parse: {}", e))),
        "toml" => toml::from_str::<AgentSpec>(raw)
            .map_err(|e| MakakooError::Config(format!("TOML parse: {}", e))),
        other => Err(MakakooError::InvalidInput(format!(
            "unsupported extension '.{}' (expected .yaml, .yml, .toml)",
            other
        ))),
    }
}

/// Parse a spec from a file path. Auto-detects format by extension.
pub fn load_from_file(path: &Path) -> Result<AgentSpec> {
    let raw = fs::read_to_string(path)
        .map_err(|e| MakakooError::Config(format!("spec file {} read: {}", path.display(), e)))?;
    let ext = path.extension().and_then(|e| e.to_str()).ok_or_else(|| {
        MakakooError::InvalidInput(format!(
            "spec file {} has no extension (expected .yaml, .yml, .toml)",
            path.display()
        ))
    })?;
    let spec = parse_str(&raw, Some(ext)).map_err(|e| match e {
        MakakooError::Config(msg) => MakakooError::Config(format!("{}: {}", path.display(), msg)),
        other => other,
    })?;
    spec.validate().map_err(|e| match e {
        MakakooError::InvalidInput(msg) => {
            MakakooError::InvalidInput(format!("{}: {}", path.display(), msg))
        }
        other => other,
    })?;
    Ok(spec)
}

/// Serialize a spec to YAML.
pub fn to_yaml(spec: &AgentSpec) -> Result<String> {
    serde_yaml_ng::to_string(spec)
        .map_err(|e| MakakooError::Config(format!("spec YAML serialize: {}", e)))
}

/// Serialize a spec to TOML.
pub fn to_toml(spec: &AgentSpec) -> Result<String> {
    toml::to_string_pretty(spec)
        .map_err(|e| MakakooError::Config(format!("spec TOML serialize: {}", e)))
}

/// Return the canonical spec file extension for a given format.
pub fn canonical_extension(format: SpecFormat) -> &'static str {
    match format {
        SpecFormat::Yaml => "yaml",
        SpecFormat::Toml => "toml",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecFormat {
    Yaml,
    Toml,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::spec::{ChannelSpec, TriggerSpec};

    const MINIMAL_YAML: &str = r#"
name: weather-bot
description: "Monitor weather forecasts, alert on severe conditions"
model: anthropic/claude-sonnet-4-6
instructions: |-
  You are a weather monitoring agent.
tools: [brain_search, write_file]

scope:
  allowed_paths: ["~/MAKAKOO/data/weather/**"]
"#;

    const MINIMAL_TOML: &str = r#"
name = "weather-bot"
description = "Monitor weather forecasts, alert on severe conditions"
model = "anthropic/claude-sonnet-4-6"
instructions = "You are a weather monitoring agent."
tools = ["brain_search", "write_file"]

[scope]
allowed_paths = ["~/MAKAKOO/data/weather/**"]
"#;

    #[test]
    fn parse_minimal_yaml() {
        let s = parse_str(MINIMAL_YAML, Some("yaml")).unwrap();
        assert_eq!(s.name, "weather-bot");
        assert_eq!(s.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(s.tools, vec!["brain_search", "write_file"]);
        assert!(s.channels.is_empty());
        assert!(s.triggers.is_empty());
    }

    #[test]
    fn parse_minimal_toml() {
        let s = parse_str(MINIMAL_TOML, Some("toml")).unwrap();
        assert_eq!(s.name, "weather-bot");
        assert_eq!(s.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(s.tools, vec!["brain_search", "write_file"]);
    }

    #[test]
    fn yaml_and_toml_semantically_equal() {
        let y = parse_str(MINIMAL_YAML, Some("yaml")).unwrap();
        let t = parse_str(MINIMAL_TOML, Some("toml")).unwrap();
        assert_eq!(y, t);
    }

    #[test]
    fn parse_yaml_with_channels() {
        let raw = r#"
name: multi
description: "multi-channel agent"
model: claude
instructions: hi
tools: [brain_search]
channels:
  - kind: telegram
    token_env: TELEGRAM_FOO
    allowed_users: ["123", "456"]
  - kind: slack
    token_env: SLACK_BOT
    app_token_env: SLACK_APP
    team_id_env: SLACK_TEAM_ID
scope: {}
"#;
        let s = parse_str(raw, Some("yaml")).unwrap();
        assert_eq!(s.channels.len(), 2);
        match &s.channels[0] {
            ChannelSpec::Telegram {
                token_env,
                allowed_users,
            } => {
                assert_eq!(token_env, "TELEGRAM_FOO");
                assert_eq!(allowed_users, &vec!["123".to_string(), "456".to_string()]);
            }
            _ => panic!("expected telegram"),
        }
        match &s.channels[1] {
            ChannelSpec::Slack {
                token_env,
                app_token_env,
                team_id_env,
                ..
            } => {
                assert_eq!(token_env, "SLACK_BOT");
                assert_eq!(app_token_env, "SLACK_APP");
                assert_eq!(team_id_env, "SLACK_TEAM_ID");
            }
            _ => panic!("expected slack"),
        }
    }

    #[test]
    fn parse_yaml_with_triggers() {
        let raw = r#"
name: cron-agent
description: "triggered"
model: claude
instructions: hi
tools: []
triggers:
  - kind: cron
    schedule: "0 */6 * * *"
    timezone: "UTC"
  - kind: webhook
    path: /hook
    secret_env: HOOK_SECRET
scope: {}
"#;
        let s = parse_str(raw, Some("yaml")).unwrap();
        assert_eq!(s.triggers.len(), 2);
        match &s.triggers[0] {
            TriggerSpec::Cron {
                schedule, timezone, ..
            } => {
                assert_eq!(schedule, "0 */6 * * *");
                assert_eq!(timezone, "UTC");
            }
            _ => panic!("expected cron"),
        }
        match &s.triggers[1] {
            TriggerSpec::Webhook { path, secret_env } => {
                assert_eq!(path, "/hook");
                assert_eq!(secret_env, "HOOK_SECRET");
            }
            _ => panic!("expected webhook"),
        }
    }

    #[test]
    fn parse_rejects_unknown_field() {
        let raw = r#"
name: foo
description: "x"
model: claude
instructions: hi
tools: []
unknown_field: "should fail"
scope: {}
"#;
        let err = parse_str(raw, Some("yaml")).unwrap_err();
        assert!(format!("{err}").contains("unknown_field"));
    }

    #[test]
    fn parse_rejects_unsupported_extension() {
        let err = parse_str("name: foo", Some("json")).unwrap_err();
        assert!(format!("{err}").contains("unsupported extension"));
    }

    #[test]
    fn round_trip_yaml() {
        let s = parse_str(MINIMAL_YAML, Some("yaml")).unwrap();
        let yaml = to_yaml(&s).unwrap();
        let s2 = parse_str(&yaml, Some("yaml")).unwrap();
        assert_eq!(s, s2);
    }

    #[test]
    fn round_trip_toml() {
        let s = parse_str(MINIMAL_TOML, Some("toml")).unwrap();
        let toml_str = to_toml(&s).unwrap();
        let s2 = parse_str(&toml_str, Some("toml")).unwrap();
        assert_eq!(s, s2);
    }

    #[test]
    fn load_from_file_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, MINIMAL_YAML).unwrap();
        let s = AgentSpec::load_from_file(&path).unwrap();
        assert_eq!(s.name, "weather-bot");
    }

    #[test]
    fn load_from_file_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.toml");
        std::fs::write(&path, MINIMAL_TOML).unwrap();
        let s = AgentSpec::load_from_file(&path).unwrap();
        assert_eq!(s.name, "weather-bot");
    }

    #[test]
    fn load_from_file_missing_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent");
        std::fs::write(&path, MINIMAL_YAML).unwrap();
        let err = AgentSpec::load_from_file(&path).unwrap_err();
        assert!(format!("{err}").contains("no extension"));
    }
}
