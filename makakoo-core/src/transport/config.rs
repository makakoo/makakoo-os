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

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::transport::secrets::SecretRef;
use crate::{MakakooError, Result};

/// One `[[transport]]` block. Fields between `id` and `config` are
/// transport-agnostic; the `config` body is discriminated by the
/// outer `kind` field via a custom `Deserialize` (TOML doesn't
/// support internal-tagged enums whose discriminator sits one
/// level above the payload, and serde's `untagged` enum picks
/// `TelegramConfig` first because every Telegram field is optional).
#[derive(Debug, Clone, Serialize)]
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

/// Wire-shape used during deserialization.  Reads `config` as a
/// raw `toml::Value` so we can dispatch the inner type by the
/// outer `kind` field.
#[derive(Deserialize)]
struct TransportEntryWire {
    id: String,
    kind: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    secret_ref: Option<String>,
    #[serde(default)]
    secret_env: Option<String>,
    #[serde(default)]
    inline_secret_dev: Option<String>,
    #[serde(default)]
    app_token_ref: Option<String>,
    #[serde(default)]
    app_token_env: Option<String>,
    #[serde(default)]
    inline_app_token_dev: Option<String>,
    #[serde(default)]
    allowed_users: Vec<String>,
    config: toml::Value,
}

impl<'de> Deserialize<'de> for TransportEntry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TransportEntryWire::deserialize(deserializer)?;
        let config = match wire.kind.as_str() {
            "telegram" => {
                let cfg: TelegramConfig = wire.config.try_into().map_err(|e| {
                    de::Error::custom(format!(
                        "transport '{}' kind=telegram: invalid [config] body: {}",
                        wire.id, e
                    ))
                })?;
                TransportConfig::Telegram(cfg)
            }
            "slack" => {
                let cfg: SlackConfig = wire.config.try_into().map_err(|e| {
                    de::Error::custom(format!(
                        "transport '{}' kind=slack: invalid [config] body: {}",
                        wire.id, e
                    ))
                })?;
                TransportConfig::Slack(cfg)
            }
            "discord" => {
                let cfg: DiscordConfig = wire.config.try_into().map_err(|e| {
                    de::Error::custom(format!(
                        "transport '{}' kind=discord: invalid [config] body: {}",
                        wire.id, e
                    ))
                })?;
                TransportConfig::Discord(cfg)
            }
            "whatsapp" => {
                let cfg: WhatsAppConfig = wire.config.try_into().map_err(|e| {
                    de::Error::custom(format!(
                        "transport '{}' kind=whatsapp: invalid [config] body: {}",
                        wire.id, e
                    ))
                })?;
                TransportConfig::WhatsApp(cfg)
            }
            "web" => {
                let cfg: WebConfig = wire.config.try_into().map_err(|e| {
                    de::Error::custom(format!(
                        "transport '{}' kind=web: invalid [config] body: {}",
                        wire.id, e
                    ))
                })?;
                TransportConfig::Web(cfg)
            }
            "voice_twilio" => {
                let cfg: VoiceTwilioConfig = wire.config.try_into().map_err(|e| {
                    de::Error::custom(format!(
                        "transport '{}' kind=voice_twilio: invalid [config] body: {}",
                        wire.id, e
                    ))
                })?;
                TransportConfig::VoiceTwilio(cfg)
            }
            other => {
                return Err(de::Error::custom(format!(
                    "transport '{}' has unsupported kind '{}' (supported: telegram | slack | discord | whatsapp | web | voice_twilio)",
                    wire.id, other
                )));
            }
        };
        Ok(TransportEntry {
            id: wire.id,
            kind: wire.kind,
            enabled: wire.enabled,
            account_id: wire.account_id,
            secret_ref: wire.secret_ref,
            secret_env: wire.secret_env,
            inline_secret_dev: wire.inline_secret_dev,
            app_token_ref: wire.app_token_ref,
            app_token_env: wire.app_token_env,
            inline_app_token_dev: wire.inline_app_token_dev,
            allowed_users: wire.allowed_users,
            config,
        })
    }
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
    Discord(DiscordConfig),
    WhatsApp(WhatsAppConfig),
    Web(WebConfig),
    VoiceTwilio(VoiceTwilioConfig),
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    /// Whether the bot's Identify payload requests the privileged
    /// MESSAGE_CONTENT intent. Default `false` (Discord developer
    /// portal requires explicit opt-in for this intent — Q6).
    /// When false the bot still receives MESSAGE_CREATE events but
    /// the `content` field arrives empty for non-DM, non-mention
    /// messages.
    #[serde(default)]
    pub message_content: bool,

    /// Allowlist of guild ids the bot accepts inbound from. Empty =
    /// allow every guild the bot is in (the spec calls this "optional";
    /// production deployments typically pin to a fixed set).
    #[serde(default)]
    pub guild_ids: Vec<u64>,

    /// Allowlist of channel ids; empty means any channel within the
    /// guild allowlist is accepted.
    #[serde(default)]
    pub channels: Vec<String>,

    /// Whether to populate `thread_id` for messages that arrive in
    /// a Discord thread (vs the parent channel).
    #[serde(default)]
    pub support_thread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    /// WhatsApp Business phone number identifier (issued by Meta).
    /// Outbound message URL is `/v18.0/{phone_number_id}/messages`.
    pub phone_number_id: String,

    /// Cloud API graph version. Default `v18.0`. Bumped via slot
    /// TOML when Meta deprecates a version.
    #[serde(default = "default_whatsapp_graph_version")]
    pub graph_version: String,

    /// Verify token used for the `hub.challenge` GET handshake. The
    /// adapter stores it as a SecretRef shape (env/keyring/inline)
    /// so production keeps the literal value out of the TOML.
    #[serde(default)]
    pub verify_token_env: Option<String>,
    #[serde(default)]
    pub verify_token_ref: Option<String>,
    #[serde(default)]
    pub inline_verify_token_dev: Option<String>,

    /// App-secret used for the X-Hub-Signature-256 HMAC over the
    /// raw POST body. SecretRef shape (env/keyring/inline).
    #[serde(default)]
    pub app_secret_env: Option<String>,
    #[serde(default)]
    pub app_secret_ref: Option<String>,
    #[serde(default)]
    pub inline_app_secret_dev: Option<String>,

    /// Allowed sender wa_ids (E.164 without +). Empty = least-
    /// privilege deny-all.
    #[serde(default)]
    pub allowed_wa_ids: Vec<String>,
}

fn default_whatsapp_graph_version() -> String {
    "v18.0".into()
}

impl WhatsAppConfig {
    pub fn verify_token_secret(&self) -> SecretRef {
        SecretRef::from_flat(
            self.verify_token_env.clone(),
            self.verify_token_ref.clone(),
            self.inline_verify_token_dev.clone(),
        )
    }
    pub fn app_secret(&self) -> SecretRef {
        SecretRef::from_flat(
            self.app_secret_env.clone(),
            self.app_secret_ref.clone(),
            self.inline_app_secret_dev.clone(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebConfig {
    /// Origin allowlist for WS upgrade. Required in production
    /// (locked Q10 round-2 fix). When empty, only loopback origins
    /// (`http://localhost`, `http://127.0.0.1`) are accepted —
    /// useful for local dev but never sufficient for a public
    /// deployment. validate() enforces this when
    /// `production_mode = true`.
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Toggle the production-mode origin requirement. Set true to
    /// fail validation when `allowed_origins` is empty.
    #[serde(default)]
    pub production_mode: bool,

    /// Visitor cookie max-age in seconds. Default 30 days.
    #[serde(default = "default_web_cookie_ttl")]
    pub cookie_ttl_seconds: u64,
}

fn default_web_cookie_ttl() -> u64 {
    30 * 24 * 3600
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VoiceTwilioConfig {
    /// Twilio Account SID (`AC…`). Required for both webhook
    /// signature verification and Recording-URL basic-auth.
    pub account_sid: String,

    /// Auth token used for basic-auth on recording fetches AND for
    /// X-Twilio-Signature HMAC. SecretRef shape.
    #[serde(default)]
    pub auth_token_env: Option<String>,
    #[serde(default)]
    pub auth_token_ref: Option<String>,
    #[serde(default)]
    pub inline_auth_token_dev: Option<String>,

    /// Allowed caller E.164 numbers (with `+`). Empty = least-
    /// privilege deny-all.
    #[serde(default)]
    pub allowed_caller_ids: Vec<String>,

    /// Base URL the public webhook is reachable at — used to
    /// construct the recording-callback URL Twilio POSTs to.
    /// Production sets this to the public makakoo-mcp URL.
    pub public_base_url: String,
}

impl VoiceTwilioConfig {
    pub fn auth_token_secret(&self) -> SecretRef {
        SecretRef::from_flat(
            self.auth_token_env.clone(),
            self.auth_token_ref.clone(),
            self.inline_auth_token_dev.clone(),
        )
    }
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
        // Some transports manage credentials in their config block
        // rather than the top-level secret_*. Skip the bot-token
        // requirement for those:
        //   - web: bot-tokenless (cookie-signed visitor auth)
        //   - voice_twilio: auth_token lives under [config]
        let kind_without_top_token =
            matches!(self.kind.as_str(), "web" | "voice_twilio");
        if !kind_without_top_token {
            let bot_token = self.bot_token_ref();
            if bot_token.is_empty() {
                return Err(MakakooError::InvalidInput(format!(
                    "transport '{}' has no bot-token source (set one of secret_env / secret_ref / inline_secret_dev)",
                    self.id
                )));
            }
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
            ("discord", TransportConfig::Discord(_)) => Ok(()),
            ("whatsapp", TransportConfig::WhatsApp(w)) => {
                if w.phone_number_id.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=whatsapp: phone_number_id must not be empty",
                        self.id
                    )));
                }
                if w.verify_token_secret().is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=whatsapp: verify_token must be set (verify_token_env / verify_token_ref / inline_verify_token_dev)",
                        self.id
                    )));
                }
                if w.app_secret().is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=whatsapp: app_secret must be set (app_secret_env / app_secret_ref / inline_app_secret_dev)",
                        self.id
                    )));
                }
                Ok(())
            }
            ("web", TransportConfig::Web(w)) => {
                if w.production_mode && w.allowed_origins.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=web: production_mode = true requires \
                         a non-empty allowed_origins list (locked Q10)",
                        self.id
                    )));
                }
                Ok(())
            }
            ("voice_twilio", TransportConfig::VoiceTwilio(v)) => {
                if v.account_sid.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=voice_twilio: account_sid must not be empty",
                        self.id
                    )));
                }
                if v.auth_token_secret().is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=voice_twilio: auth_token must be set (locked Q9 — required for X-Twilio-Signature verify + recording basic-auth)",
                        self.id
                    )));
                }
                if v.public_base_url.is_empty() {
                    return Err(MakakooError::InvalidInput(format!(
                        "transport '{}' kind=voice_twilio: public_base_url must not be empty (used to build the recording-callback URL Twilio POSTs to)",
                        self.id
                    )));
                }
                Ok(())
            }
            (k, _) => Err(MakakooError::InvalidInput(format!(
                "transport '{}' has kind '{}' that doesn't match its config payload (supported: telegram | slack | discord | whatsapp | web | voice_twilio)",
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

    fn discord_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "discord".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some("BOTTOK".into()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec![],
            config: TransportConfig::Discord(DiscordConfig::default()),
        }
    }

    #[test]
    fn discord_entry_validates_with_bot_token() {
        let e = discord_entry("discord-main");
        e.validate().unwrap();
    }

    #[test]
    fn discord_entry_requires_bot_token() {
        let mut e = discord_entry("d");
        e.inline_secret_dev = None;
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("no bot-token source"));
    }

    #[test]
    fn discord_kind_must_match_config_payload() {
        let mut e = discord_entry("d");
        e.config = TransportConfig::Telegram(TelegramConfig::default());
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("doesn't match"));
    }

    fn voice_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "voice_twilio".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec![],
            config: TransportConfig::VoiceTwilio(VoiceTwilioConfig {
                account_sid: "ACdeadbeef".into(),
                auth_token_env: None,
                auth_token_ref: None,
                inline_auth_token_dev: Some("AUTHTOK".into()),
                allowed_caller_ids: vec![],
                public_base_url: "https://example.com".into(),
            }),
        }
    }

    #[test]
    fn voice_entry_validates() {
        voice_entry("voice-main").validate().unwrap();
    }

    #[test]
    fn voice_requires_account_sid() {
        let mut e = voice_entry("v");
        if let TransportConfig::VoiceTwilio(ref mut v) = e.config {
            v.account_sid = "".into();
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("account_sid"));
    }

    #[test]
    fn voice_requires_auth_token() {
        let mut e = voice_entry("v");
        if let TransportConfig::VoiceTwilio(ref mut v) = e.config {
            v.inline_auth_token_dev = None;
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("auth_token"));
    }

    #[test]
    fn voice_requires_public_base_url() {
        let mut e = voice_entry("v");
        if let TransportConfig::VoiceTwilio(ref mut v) = e.config {
            v.public_base_url = "".into();
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("public_base_url"));
    }

    fn web_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "web".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: None,
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec![],
            config: TransportConfig::Web(WebConfig::default()),
        }
    }

    #[test]
    fn web_entry_validates_without_bot_token() {
        // Web chat is bot-tokenless — visitors identify via signed cookies.
        web_entry("web-main").validate().unwrap();
    }

    #[test]
    fn web_production_mode_requires_allowed_origins() {
        let mut e = web_entry("web");
        if let TransportConfig::Web(ref mut w) = e.config {
            w.production_mode = true;
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("allowed_origins"));
    }

    #[test]
    fn web_production_mode_with_allowlist_is_ok() {
        let mut e = web_entry("web");
        if let TransportConfig::Web(ref mut w) = e.config {
            w.production_mode = true;
            w.allowed_origins = vec!["https://harvey.example".into()];
        }
        e.validate().unwrap();
    }

    #[test]
    fn web_round_trip_via_toml() {
        let raw = r#"
id = "web-main"
kind = "web"
enabled = true

[config]
allowed_origins = ["https://harvey.example"]
production_mode = true
cookie_ttl_seconds = 86400
"#;
        let entry: TransportEntry = toml::from_str(raw).unwrap();
        assert_eq!(entry.kind, "web");
        match &entry.config {
            TransportConfig::Web(w) => {
                assert!(w.production_mode);
                assert_eq!(w.allowed_origins, vec!["https://harvey.example".to_string()]);
                assert_eq!(w.cookie_ttl_seconds, 86400);
            }
            _ => panic!("expected web variant"),
        }
        entry.validate().unwrap();
    }

    fn whatsapp_entry(id: &str) -> TransportEntry {
        TransportEntry {
            id: id.into(),
            kind: "whatsapp".into(),
            enabled: true,
            account_id: None,
            secret_ref: None,
            secret_env: None,
            inline_secret_dev: Some("ACCESS".into()),
            app_token_ref: None,
            app_token_env: None,
            inline_app_token_dev: None,
            allowed_users: vec![],
            config: TransportConfig::WhatsApp(WhatsAppConfig {
                phone_number_id: "12345".into(),
                graph_version: "v18.0".into(),
                verify_token_env: None,
                verify_token_ref: None,
                inline_verify_token_dev: Some("HUBVERIFY".into()),
                app_secret_env: None,
                app_secret_ref: None,
                inline_app_secret_dev: Some("APPSECRET".into()),
                allowed_wa_ids: vec![],
            }),
        }
    }

    #[test]
    fn whatsapp_entry_validates() {
        whatsapp_entry("wa-main").validate().unwrap();
    }

    #[test]
    fn whatsapp_requires_phone_number_id() {
        let mut e = whatsapp_entry("wa");
        if let TransportConfig::WhatsApp(ref mut w) = e.config {
            w.phone_number_id = "".into();
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("phone_number_id"));
    }

    #[test]
    fn whatsapp_requires_verify_token() {
        let mut e = whatsapp_entry("wa");
        if let TransportConfig::WhatsApp(ref mut w) = e.config {
            w.inline_verify_token_dev = None;
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("verify_token"));
    }

    #[test]
    fn whatsapp_requires_app_secret() {
        let mut e = whatsapp_entry("wa");
        if let TransportConfig::WhatsApp(ref mut w) = e.config {
            w.inline_app_secret_dev = None;
        }
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("app_secret"));
    }

    #[test]
    fn whatsapp_round_trip_via_toml() {
        let raw = r#"
id = "wa-main"
kind = "whatsapp"
enabled = true
inline_secret_dev = "ACCESS"

[config]
phone_number_id = "12345"
graph_version = "v18.0"
inline_verify_token_dev = "HUBVERIFY"
inline_app_secret_dev = "APPSECRET"
allowed_wa_ids = ["34000000001"]
"#;
        let entry: TransportEntry = toml::from_str(raw).unwrap();
        assert_eq!(entry.kind, "whatsapp");
        match &entry.config {
            TransportConfig::WhatsApp(w) => {
                assert_eq!(w.phone_number_id, "12345");
                assert_eq!(w.allowed_wa_ids, vec!["34000000001".to_string()]);
            }
            _ => panic!("expected whatsapp variant"),
        }
        entry.validate().unwrap();
    }

    #[test]
    fn discord_round_trip_via_toml() {
        let raw = r#"
id = "discord-main"
kind = "discord"
enabled = true
inline_secret_dev = "BOTTOK"
allowed_users = ["9000"]

[config]
message_content = false
guild_ids = [42, 99]
support_thread = false
"#;
        let entry: TransportEntry = toml::from_str(raw).unwrap();
        assert_eq!(entry.kind, "discord");
        match &entry.config {
            TransportConfig::Discord(d) => {
                assert!(!d.message_content);
                assert_eq!(d.guild_ids, vec![42, 99]);
            }
            _ => panic!("expected discord variant"),
        }
        entry.validate().unwrap();
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
