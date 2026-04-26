//! TOML schema types for `[[transport]]` blocks in
//! `~/MAKAKOO/config/agents/<slot>.toml`.
//!
//! Locked by SPRINT-MULTI-BOT-SUBAGENTS Q9 + Q11. The schema is
//! transport-agnostic: every block has `id`, `kind`, `enabled`,
//! plus a typed `config` body discriminated by `kind`.
//!
//! Per-transport validation rules (Q11 table) are enforced by
//! `TransportEntry::validate` and by adapter-specific verifiers
//! when `makakoo agent create` runs.

use serde::{Deserialize, Serialize};

use crate::transport::secrets::SecretRef;
use crate::{MakakooError, Result};

/// One `[[transport]]` block. The `config` payload is discriminated
/// by `kind`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportEntry {
    /// Slot-unique transport identifier (e.g. `"telegram-main"`).
    pub id: String,
    /// `"telegram"` | `"slack"` in v1.
    pub kind: String,
    /// Whether the agent should start this transport. Default `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Per-transport allowlist that NARROWS the slot-level allowlist
    /// (intersection rule, Q7).
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Whether the inbound frame's `thread_id` should be populated.
    /// Default `false`.
    #[serde(default)]
    pub support_thread: bool,
    /// Adapter-specific config.
    pub config: TransportConfig,
}

fn default_true() -> bool {
    true
}

/// Adapter-specific config payload, discriminated by the parent
/// `kind` field.  In TOML this lives under `[transport.config]`.
///
/// We use untagged enum + manual dispatch instead of serde's tag
/// internal because the TOML format puts the discriminator at the
/// outer `[[transport]]` level, not inside `[transport.config]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TransportConfig {
    Telegram(TelegramConfig),
    Slack(SlackConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token (xxx:yyy). Resolved through SecretsAdapter.
    pub token: SecretRef,
    /// Long-poll interval in milliseconds. Default 1000.
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
}

fn default_poll_interval_ms() -> u64 {
    1000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    /// App-level token (`xapp-…`) for Socket Mode WebSocket.
    pub app_token: SecretRef,
    /// Bot token (`xoxb-…`) for `chat.postMessage` / `auth.test`.
    pub bot_token: SecretRef,
    /// Slack workspace tenant identifier.
    pub team_id: String,
    /// DM-only mode (default `true`). Set `false` to enable channel
    /// events; `channels` becomes required.
    #[serde(default = "default_true")]
    pub dm_only: bool,
    /// Channel ID allowlist for `dm_only = false`.
    #[serde(default)]
    pub channels: Vec<String>,
}

impl TransportEntry {
    /// Validate the entry against Q11 rules. Pure schema-level checks
    /// only — no network calls (those live in
    /// `Transport::verify_credentials`).
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(MakakooError::InvalidInput(
                "transport.id must not be empty".into(),
            ));
        }
        match (&self.kind[..], &self.config) {
            ("telegram", TransportConfig::Telegram(_)) => Ok(()),
            ("slack", TransportConfig::Slack(s)) => {
                if !s.dm_only && s.channels.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=slack: channels list is required when dm_only = false",
                        self.id
                    )));
                }
                if s.team_id.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=slack: team_id must not be empty",
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
/// Same-kind guards (Q11):
/// - duplicate `transport.id` within a slot → reject (any kind);
/// - the higher-level `verify_credentials` step also rejects
///   duplicate bot identities (e.g. same Telegram `getMe.id`,
///   same Slack `bot_token` in same `team_id`); that lives at the
///   adapter layer because it requires network calls.
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
            allowed_users: vec![],
            support_thread: false,
            config: TransportConfig::Telegram(TelegramConfig {
                token: SecretRef::Inline("123:abc".into()),
                poll_interval_ms: 1000,
            }),
        }
    }

    fn slack_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "slack".into(),
            enabled: true,
            allowed_users: vec![],
            support_thread: false,
            config: TransportConfig::Slack(SlackConfig {
                app_token: SecretRef::Inline("xapp-1".into()),
                bot_token: SecretRef::Inline("xoxb-1".into()),
                team_id: "T0123ABCD".into(),
                dm_only: true,
                channels: vec![],
            }),
        }
    }

    #[test]
    fn entry_id_required() {
        let mut e = telegram_entry("");
        e.id = "".into();
        assert!(e.validate().is_err());
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
}
