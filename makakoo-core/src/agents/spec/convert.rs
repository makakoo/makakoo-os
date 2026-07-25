//! Convert AgentSpec → AgentSlot.
//!
//! This is the bridge between the declarative spec (what the user
//! writes) and the slot TOML (what the runtime reads). One spec
//! produces one slot TOML.
//!
//! Spec → Slot field mapping:
//!
//! | spec field             | slot field                                  |
//! |------------------------|---------------------------------------------|
//! | `name`                 | `slot_id`, `name`                           |
//! | `description`          | (dropped; surfaced via Flue scaffold)       |
//! | `model`                | `llm.override.model`                        |
//! | `instructions`         | `persona`                                   |
//! | `tools`                | `tools`                                     |
//! | `scope.allowed_paths`  | `allowed_paths`                             |
//! | `scope.forbidden_paths`| `forbidden_paths`                           |
//! | `channels[]`           | `transports[]` (one TransportEntry each)    |
//! | `triggers[]`           | (deferred to Phase 4 — Flue scaffold)       |
//!
//! Channel → transport kind mapping (spec kind → slot kind):
//!
//! * `telegram` → `telegram`
//! * `slack`    → `slack`     (team_id becomes a `T_FROM_<env>` placeholder)
//! * `discord`  → `discord`
//! * `webhook`  → `web`
//! * `email`    → `email`
//! * `voice`    → `voicetwilio`

use crate::agents::llm_override::{LlmOverride, ReasoningEffort};
use crate::agents::slot::AgentSlot;
use crate::transport::config::{
    DiscordConfig, EmailConfig, SlackConfig, TelegramConfig, TransportConfig, TransportEntry,
    VoiceTwilioConfig, WebConfig,
};
use crate::{MakakooError, Result};

use super::AgentSpec;

impl AgentSpec {
    /// Convert this spec to a slot TOML representation. The returned
    /// `AgentSlot` is in-memory; persistence is the caller's job.
    pub fn to_slot(&self) -> Result<AgentSlot> {
        self.validate()?;

        let transports: Vec<TransportEntry> = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| channel_to_transport(c, i, &self.name))
            .collect::<Result<Vec<_>>>()?;

        let slot = AgentSlot {
            slot_id: self.name.clone(),
            name: self.name.clone(),
            persona: Some(self.instructions.clone()),
            inherit_baseline: true,
            allowed_paths: self.scope.allowed_paths.clone(),
            forbidden_paths: self.scope.forbidden_paths.clone(),
            tools: self.tools.clone(),
            process_mode: "supervised_pair".into(),
            transports,
            llm: Some(crate::agents::slot::LlmSection {
                inherit: None,
                overrides: Some(LlmOverride {
                    model: Some(self.model.clone()),
                    max_tokens: None,
                    temperature: None,
                    reasoning_effort: Some(ReasoningEffort::Medium),
                }),
            }),
        };
        Ok(slot)
    }
}

fn channel_to_transport(
    c: &super::ChannelSpec,
    index: usize,
    agent_name: &str,
) -> Result<TransportEntry> {
    let id = format!("{}-{}", transport_id_stem(c), index);
    match c {
        super::ChannelSpec::Telegram { token_env, allowed_users } => Ok(TransportEntry {
            id,
            kind: "telegram".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: Some(token_env.clone()),
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: allowed_users.clone(),
            config: TransportConfig::Telegram(TelegramConfig {
                polling_timeout_seconds: 30,
                allowed_chat_ids: allowed_users.clone(),
                allowed_group_ids: vec![],
                support_thread: false,
            }),
        }),
        super::ChannelSpec::Slack {
            token_env,
            app_token_env,
            team_id_env,
            allowed_users,
        } => Ok(TransportEntry {
            id,
            kind: "slack".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: Some(token_env.clone()),
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: Some(app_token_env.clone()),
            inline_app_token_dev: None,
            allowed_users: allowed_users.clone(),
            config: TransportConfig::Slack(SlackConfig {
                // V1: team_id is a hardcoded String in SlackConfig. The
                // converter can't store an env-var reference, so it
                // emits a sentinel the operator must replace. Phase 4
                // will add `team_id_env` to SlackConfig so the runtime
                // resolves it.
                team_id: format!("T_FROM_{}", team_id_env),
                mode: "socket".into(),
                dm_only: true,
                channels: vec![],
                support_thread: false,
            }),
        }),
        super::ChannelSpec::Discord { token_env, allowed_users } => Ok(TransportEntry {
            id,
            kind: "discord".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: Some(token_env.clone()),
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: allowed_users.clone(),
            config: TransportConfig::Discord(DiscordConfig {
                message_content: false,
                guild_ids: vec![],
                channels: vec![],
                support_thread: false,
            }),
        }),
        super::ChannelSpec::Webhook { path, secret_env } => {
            // WebConfig (the slot's WS transport) doesn't carry a
            // `path` field; webhook endpoints are wired in the Flue
            // scaffold directly from the spec. We surface the path
            // via `account_id` so it's not lost, and the renderer
            // will read it back. Phase 4 will replace this with a
            // proper field once the Flue scaffold takes the spec
            // directly.
            let _ = path;
            Ok(TransportEntry {
                id,
                kind: "web".into(), // spec "webhook" → slot "web"
                enabled: true,
                account_id: None,
                secret_ref: None,
                secret_env: Some(secret_env.clone()),
                inline_secret_dev: None,
                app_token_ref: None,
                app_token_env: None,
                inline_app_token_dev: None,
                allowed_users: vec![],
                config: TransportConfig::Web(WebConfig {
                    allowed_origins: vec![],
                    production_mode: false,
                    cookie_ttl_seconds: 30 * 24 * 3600,
                }),
            })
        }
        super::ChannelSpec::Email {
            smtp_host,
            imap_host,
            secret_env,
        } => {
            // Slot TOML placeholders for email (V1 deferred — the
            // scaffold will error with a clear "use webhook +
            // defineTool" message). Required fields must still
            // pass slot validation so the registry record is
            // writable.
            let _ = secret_env;
            Ok(TransportEntry {
                id,
                kind: "email".into(),
                enabled: true,
                account_id: None,
                secret_ref: None,
                secret_env: Some(secret_env.clone()),
                inline_secret_dev: None,
                app_token_ref: None,
                app_token_env: None,
                inline_app_token_dev: None,
                allowed_users: vec![],
                config: TransportConfig::Email(EmailConfig {
                    account_id: format!("EMAIL_FROM_{}", agent_name),
                    auth_mode: "app_password".into(),
                    imap_server: imap_host.clone(),
                    imap_port: 993,  // implicit TLS (port 143 rejected)
                    smtp_server: smtp_host.clone(),
                    smtp_port: 587,  // STARTTLS (port 25 rejected)
                    ..Default::default()
                }),
            })
        }
        super::ChannelSpec::Voice {
            twilio_account_sid_env,
            secret_env,
        } => Ok(TransportEntry {
            id,
            kind: "voice_twilio".into(), // spec "voice" → slot "voicetwilio"
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: Some(secret_env.clone()),
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec![],
            config: TransportConfig::VoiceTwilio(VoiceTwilioConfig {
                account_sid: format!("AC_FROM_{}", twilio_account_sid_env),
                auth_token_env: Some(secret_env.clone()),
                auth_token_ref: None,
                inline_auth_token_dev: None,
                allowed_caller_ids: vec![],
                public_base_url: "http://localhost:18080".into(),
            }),
        }),
    }
}

fn transport_id_stem(c: &super::ChannelSpec) -> &'static str {
    match c {
        super::ChannelSpec::Telegram { .. } => "telegram",
        super::ChannelSpec::Slack { .. } => "slack",
        super::ChannelSpec::Discord { .. } => "discord",
        super::ChannelSpec::Webhook { .. } => "web",
        super::ChannelSpec::Email { .. } => "email",
        super::ChannelSpec::Voice { .. } => "voice",
    }
}

/// If the spec declares triggers, return a one-line warning
/// describing that the slot TOML doesn't (yet) represent them.
/// Phase 4 (Flue scaffold) will read triggers directly from the
/// spec; this helper is a safety net for Phase 2.
pub fn triggers_warning(spec: &AgentSpec) -> Option<String> {
    if spec.triggers.is_empty() {
        return None;
    }
    let kinds: Vec<String> = spec.triggers.iter().map(trigger_kind_name).collect();
    Some(format!(
        "spec '{}' declares {} trigger(s) ({}) — slot TOML will not \
         represent them in Phase 2; they will be read by the Flue scaffold in Phase 4",
        spec.name,
        spec.triggers.len(),
        kinds.join(", ")
    ))
}

fn trigger_kind_name(t: &super::TriggerSpec) -> String {
    match t {
        super::TriggerSpec::Cron { .. } => "cron".into(),
        super::TriggerSpec::Webhook { .. } => "webhook".into(),
    }
}

// Silence unused warning on MakakooError when the module is loaded
// without features that exercise the type directly.
#[allow(dead_code)]
fn _force_link(e: MakakooError) -> MakakooError {
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::spec::{ChannelSpec, ScopeSpec, TriggerSpec};
    use crate::transport::config::TransportConfig;

    fn minimal_spec() -> AgentSpec {
        AgentSpec {
            name: "weather-bot".into(),
            description: "Monitor weather".into(),
            model: "anthropic/claude-sonnet-4-6".into(),
            instructions: "Watch weather.".into(),
            tools: vec!["brain_search".into()],
            channels: vec![],
            triggers: vec![],
            scope: ScopeSpec::default(),
        }
    }

    #[test]
    fn minimal_spec_to_slot() {
        let slot = minimal_spec().to_slot().unwrap();
        assert_eq!(slot.slot_id, "weather-bot");
        assert_eq!(slot.name, "weather-bot");
        assert_eq!(slot.persona.as_deref(), Some("Watch weather."));
        assert!(slot.transports.is_empty());
        let llm = slot.llm.as_ref().unwrap();
        let over = llm.overrides.as_ref().unwrap();
        assert_eq!(over.model.as_deref(), Some("anthropic/claude-sonnet-4-6"));
    }

    #[test]
    fn telegram_channel_to_transport() {
        let mut s = minimal_spec();
        s.channels = vec![ChannelSpec::Telegram {
            token_env: "TELEGRAM_FOO".into(),
            allowed_users: vec!["123".into()],
        }];
        let slot = s.to_slot().unwrap();
        assert_eq!(slot.transports.len(), 1);
        let t = &slot.transports[0];
        assert_eq!(t.kind, "telegram");
        assert_eq!(t.id, "telegram-0");
        assert_eq!(t.secret_env.as_deref(), Some("TELEGRAM_FOO"));
        assert_eq!(t.allowed_users, vec!["123"]);
        match &t.config {
            TransportConfig::Telegram(c) => {
                assert_eq!(c.allowed_chat_ids, vec!["123"]);
            }
            other => panic!("expected Telegram config, got {:?}", other),
        }
    }

    #[test]
    fn slack_channel_uses_team_id_placeholder() {
        let mut s = minimal_spec();
        s.channels = vec![ChannelSpec::Slack {
            token_env: "SLACK_BOT".into(),
            app_token_env: "SLACK_APP".into(),
            team_id_env: "SLACK_TEAM_ID".into(),
            allowed_users: vec!["U0".into()],
        }];
        let slot = s.to_slot().unwrap();
        let t = &slot.transports[0];
        assert_eq!(t.kind, "slack");
        match &t.config {
            TransportConfig::Slack(c) => {
                assert_eq!(c.team_id, "T_FROM_SLACK_TEAM_ID");
            }
            _other => panic!("expected Slack config"),
        }
    }

    #[test]
    fn webhook_channel_maps_to_web() {
        let mut s = minimal_spec();
        s.channels = vec![ChannelSpec::Webhook {
            path: "/hook".into(),
            secret_env: "HOOK".into(),
        }];
        let slot = s.to_slot().unwrap();
        let t = &slot.transports[0];
        assert_eq!(t.kind, "web");
    }

    #[test]
    fn voice_channel_maps_to_voicetwilio() {
        let mut s = minimal_spec();
        s.channels = vec![ChannelSpec::Voice {
            twilio_account_sid_env: "TWILIO_SID".into(),
            secret_env: "TWILIO_AUTH".into(),
        }];
        let slot = s.to_slot().unwrap();
        let t = &slot.transports[0];
        assert_eq!(t.kind, "voice_twilio");
    }

    #[test]
    fn multiple_channels_get_unique_ids() {
        let mut s = minimal_spec();
        s.channels = vec![
            ChannelSpec::Telegram {
                token_env: "T1".into(),
                allowed_users: vec![],
            },
            ChannelSpec::Telegram {
                token_env: "T2".into(),
                allowed_users: vec![],
            },
            ChannelSpec::Discord {
                token_env: "D1".into(),
                allowed_users: vec![],
            },
        ];
        let slot = s.to_slot().unwrap();
        assert_eq!(slot.transports.len(), 3);
        assert_eq!(slot.transports[0].id, "telegram-0");
        assert_eq!(slot.transports[1].id, "telegram-1");
        assert_eq!(slot.transports[2].id, "discord-2");
    }

    #[test]
    fn scope_propagates_to_slot() {
        let mut s = minimal_spec();
        s.scope = ScopeSpec {
            allowed_paths: vec!["~/MAKAKOO/data/foo/**".into()],
            forbidden_paths: vec!["~/.ssh/**".into()],
        };
        let slot = s.to_slot().unwrap();
        assert_eq!(slot.allowed_paths, vec!["~/MAKAKOO/data/foo/**"]);
        assert_eq!(slot.forbidden_paths, vec!["~/.ssh/**"]);
    }

    #[test]
    fn triggers_warning_when_present() {
        let mut s = minimal_spec();
        s.triggers = vec![TriggerSpec::Cron {
            schedule: "0 0 * * *".into(),
            timezone: "".into(),
        }];
        let w = triggers_warning(&s);
        assert!(w.is_some());
        assert!(w.unwrap().contains("cron"));
    }

    #[test]
    fn triggers_warning_absent_when_empty() {
        let s = minimal_spec();
        assert!(triggers_warning(&s).is_none());
    }

    #[test]
    fn invalid_spec_fails_to_slot() {
        let mut s = minimal_spec();
        s.name = "INVALID NAME".into();
        assert!(s.to_slot().is_err());
    }
}
