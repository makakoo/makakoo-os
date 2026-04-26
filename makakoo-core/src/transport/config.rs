//! TOML schema types for `[[transport]]` blocks in
//! `~/MAKAKOO/config/agents/<slot>.toml`.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Q9 + Q11. Per-transport
//! validation rules (Q11 table) are enforced by
//! `TransportEntry::validate`; same-kind multi-transport guards are
//! enforced by `validate_transport_list` (schema-level) and by
//! adapter-specific verifiers when `makakoo agent create` runs
//! (network-level — duplicate `getMe.id`, duplicate `bot_token` in
//! same `team_id`).
//!
//! Secrets sit FLAT at the `[[transport]]` level (not nested under
//! `[transport.config]`) per the locked Q9 example. The
//! `[transport.config]` block holds adapter-specific routing /
//! polling / mode settings only.

use serde::{Deserialize, Serialize};

use crate::transport::secrets::SecretRef;
use crate::{MakakooError, Result};

/// One `[[transport]]` block. Fields between `id` and `config` are
/// transport-agnostic; the `config` body is discriminated by `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportEntry {
    /// Slot-unique transport identifier (e.g. `"telegram-main"`).
    pub id: String,
    /// `"telegram"` | `"slack"` in v1.
    pub kind: String,
    /// Whether the agent should start this transport. Default `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Optional human-readable account hint for `makakoo agent show`
    /// (e.g. `"@SecretaryBot"`, `"T0123TEAM:B0123BOT"`). Display
    /// only — not used for routing.
    #[serde(default)]
    pub account_id: Option<String>,

    // ── Telegram bot-token / Slack bot-token slot ─────────────────
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub secret_env: Option<String>,
    #[serde(default)]
    pub inline_secret_dev: Option<String>,

    // ── Slack app-token slot (Socket Mode) ────────────────────────
    #[serde(default)]
    pub app_token_ref: Option<String>,
    #[serde(default)]
    pub app_token_env: Option<String>,
    #[serde(default)]
    pub inline_app_token_dev: Option<String>,

    /// Per-transport allowlist (Q7, simplified). Absent or empty
    /// means least-privilege — the transport rejects all inbound
    /// messages.
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Adapter-specific config (routing / polling / mode).
    pub config: TransportConfig,
}

fn default_true() -> bool {
    true
}

/// Adapter-specific config payload, discriminated by the parent
/// `kind` field. In TOML this lives under `[transport.config]`.
///
/// Untagged enum + manual dispatch — TOML keeps the discriminator
/// at the outer `[[transport]]` level, not inside `[transport.config]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TransportConfig {
    Telegram(TelegramConfig),
    Slack(SlackConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    /// Long-poll timeout in seconds. Default 30 per Q11 table.
    #[serde(default = "default_polling_timeout_seconds")]
    pub polling_timeout_seconds: u64,

    /// Allowed Telegram chat ids (DM scope). Per-transport list.
    #[serde(default)]
    pub allowed_chat_ids: Vec<String>,

    /// Allowed Telegram group ids.
    #[serde(default)]
    pub allowed_group_ids: Vec<String>,

    /// Whether to populate `thread_id` for forum-topic messages.
    /// Default `false`.
    #[serde(default)]
    pub support_thread: bool,
}

fn default_polling_timeout_seconds() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// Slack workspace tenant identifier — must match the
    /// `team_id` returned by `auth.test`.
    pub team_id: String,

    /// Mode discriminator. v1 only supports `"socket"`. Webhook
    /// mode is a follow-on adapter.
    #[serde(default = "default_slack_mode")]
    pub mode: String,

    /// DM-only mode (default `true`). Set `false` to enable channel
    /// events; `channels` becomes required.
    #[serde(default = "default_true")]
    pub dm_only: bool,

    /// Channel ID allowlist — required when `dm_only = false`.
    #[serde(default)]
    pub channels: Vec<String>,

    /// Whether to populate `thread_id` for `thread_ts` messages.
    #[serde(default)]
    pub support_thread: bool,
}

fn default_slack_mode() -> String {
    "socket".into()
}

impl TransportEntry {
    /// Resolve the bot-token slot into a `SecretRef`.
    pub fn bot_token_ref(&self) -> SecretRef {
        SecretRef::from_flat(
            self.secret_env.clone(),
            self.secret_ref.clone(),
            self.inline_secret_dev.clone(),
        )
    }

    /// Resolve the Slack app-token slot into a `SecretRef`.
    pub fn app_token_ref(&self) -> SecretRef {
        SecretRef::from_flat(
            self.app_token_env.clone(),
            self.app_token_ref.clone(),
            self.inline_app_token_dev.clone(),
        )
    }

    /// Schema-level validation. Pure (no network calls).
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(MakakooError::InvalidInput(
                "transport.id must not be empty".into(),
            ));
        }
        let bot_token = self.bot_token_ref();
        if bot_token.is_empty() {
            return Err(MakakooError::InvalidInput(format!(
                "transport '{}' has no bot-token source (set one of secret_env / secret_ref / inline_secret_dev)",
                self.id
            )));
        }
        match (&self.kind[..], &self.config) {
            ("telegram", TransportConfig::Telegram(_)) => Ok(()),
            ("slack", TransportConfig::Slack(s)) => {
                if s.team_id.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=slack: team_id must not be empty",
                        self.id
                    )));
                }
                if s.mode != "socket" {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=slack: only mode = \"socket\" is supported in v1",
                        self.id
                    )));
                }
                if !s.dm_only && s.channels.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=slack: channels list is required when dm_only = false",
                        self.id
                    )));
                }
                let app_token = self.app_token_ref();
                if app_token.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=slack: Socket Mode requires an app token (set one of app_token_env / app_token_ref / inline_app_token_dev)",
                        self.id
                    )));
                }
                Ok(())
            }
            (k, _) => Err(MakakooError::InvalidInput(format!(
                "transport '{}' has kind '{}' that doesn't match its config payload (v1 supports telegram | slack)",
                self.id, k
            ))),
        }
    }
}

/// Validate a slot's full transport list — uniqueness + per-entry
/// validation + same-kind guards.
///
/// Schema-level guards (Q11):
/// - duplicate `transport.id` within a slot → reject (any kind);
///
/// Network-level guards live in the adapter `verify_credentials`
/// step (duplicate Telegram `getMe.id`, duplicate Slack `bot_token`
/// in same `team_id`).  See `TransportRouter::verify_no_duplicate_identities`.
pub fn validate_transport_list(entries: &[TransportEntry]) -> Result<()> {
    let mut seen_ids = std::collections::HashSet::new();
    for entry in entries {
        if !seen_ids.insert(entry.id.clone()) {
            return Err(MakakooError::InvalidInput(format!(
                "duplicate transport.id '{}' in slot — every [[transport]] must have a slot-unique id",
                entry.id
            )));
        }
        entry.validate()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn telegram_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "telegram".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some("123:abc".into()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec![],
            config: TransportConfig::Telegram(TelegramConfig::default()),
        }
    }

    fn slack_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "slack".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some("xoxb-1".into()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: Some("xapp-1".into()),
            allowed_users: vec![],
            config: TransportConfig::Slack(SlackConfig {
                team_id: "T0123ABCD".into(),
                mode: "socket".into(),
                dm_only: true,
                channels: vec![],
                support_thread: false,
            }),
        }
    }

    #[test]
    fn entry_id_required() {
        let mut e = telegram_entry("ok");
        e.id = "".into();
        assert!(e.validate().is_err());
    }

    #[test]
    fn empty_token_sources_rejected() {
        let mut e = telegram_entry("t");
        e.inline_secret_dev = None;
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("no bot-token source"));
    }

    #[test]
    fn slack_channel_mode_requires_channels() {
        let mut e = slack_entry("slack-main");
        if let TransportConfig::Slack(ref mut s) = e.config {
            s.dm_only = false;
            s.channels = vec![];
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("channels"));
    }

    #[test]
    fn slack_requires_app_token() {
        let mut e = slack_entry("s");
        e.inline_app_token_dev = None;
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("app token"));
    }

    #[test]
    fn slack_only_socket_mode() {
        let mut e = slack_entry("s");
        if let TransportConfig::Slack(ref mut s) = e.config {
            s.mode = "webhook".into();
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("socket"));
    }

    #[test]
    fn kind_must_match_config_payload() {
        let mut e = telegram_entry("t");
        e.kind = "slack".into();
        assert!(e.validate().is_err());
    }

    #[test]
    fn duplicate_transport_id_rejected() {
        let entries = vec![telegram_entry("dup"), slack_entry("dup")];
        let err = validate_transport_list(&entries).unwrap_err();
        assert!(format!("{err}").contains("duplicate transport.id"));
    }

    #[test]
    fn distinct_ids_accepted() {
        let entries = vec![telegram_entry("tg-a"), slack_entry("slack-a")];
        validate_transport_list(&entries).unwrap();
    }

    #[test]
    fn two_telegram_distinct_ids_accepted() {
        let entries = vec![telegram_entry("tg-a"), telegram_entry("tg-b")];
        validate_transport_list(&entries).unwrap();
    }

    #[test]
    fn flat_secret_round_trip_via_toml() {
        let raw = r#"
id = "telegram-main"
kind = "telegram"
enabled = true
account_id = "@SecretaryBot"
secret_ref = "agent/secretary/telegram-main/bot_token"
secret_env = "SECRETARY_TELEGRAM_MAIN_TOKEN"
inline_secret_dev = ""
allowed_users = ["746496145"]

[config]
polling_timeout_seconds = 30
allowed_chat_ids = ["746496145"]
allowed_group_ids = []
support_thread = true
"#;
        let entry: TransportEntry = toml::from_str(raw).unwrap();
        assert_eq!(entry.id, "telegram-main");
        assert_eq!(
            entry.secret_ref,
            Some("agent/secretary/telegram-main/bot_token".into())
        );
        assert_eq!(
            entry.secret_env,
            Some("SECRETARY_TELEGRAM_MAIN_TOKEN".into())
        );
        // Empty inline_secret_dev should normalise to None on resolution.
        let bt = entry.bot_token_ref();
        assert!(bt.inline.is_none());
        assert!(bt.env.is_some());
        assert!(bt.keyring_ref.is_some());
    }
}
