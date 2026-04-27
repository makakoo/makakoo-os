//! IPC frame types exchanged between the Rust transport layer and the
//! per-slot Python chat gateway. Locked by SPRINT-MULTI-BOT-SUBAGENTS
//! Phase 0 (`## IPC envelope schema (locked)` section).
//!
//! Wire encoding: newline-delimited JSON. Each side writes one envelope
//! object per line. The envelope discriminates between inbound and
//! outbound payloads.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Discriminator for thread semantics across transports.
///
/// Telegram forum topics and Slack thread replies are not the same
/// abstraction — the inbound frame carries this discriminator so the
/// outbound demux layer can route a reply back into the correct kind
/// of thread (or refuse if the kinds don't match).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    /// Telegram forum topic (`message_thread_id`).
    TelegramForum,
    /// Slack thread reply (`thread_ts`).
    SlackThread,
}

/// Inbound message frame: Rust transport → Python gateway.
///
/// One frame per inbound message, carrying enough metadata for the LLM
/// dispatcher to know who sent what, where to reply, and which transport
/// delivered the message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakakooInboundFrame {
    /// Set by the transport task from its spawn context. Every frame
    /// from a given transport task carries the slot id of the agent it
    /// belongs to. No per-message registry lookup needed.
    pub agent_slot_id: String,

    /// PRIMARY routing key. Matches the `[[transport]].id` field in
    /// the agent's TOML (e.g. `"telegram-main"`). Outbound frames
    /// must echo this to be accepted (cross-transport reply forbidden).
    pub transport_id: String,

    /// Type dispatch. `"telegram"` or `"slack"` in v1.
    pub transport_kind: String,

    /// Auxiliary diagnostic — the resolved bot identity from the
    /// transport's identity API (Telegram `getMe.id`; Slack
    /// `auth.test.bot_id`+`team_id`). NOT used for routing.
    pub account_id: String,

    /// Where to reply. Transport-native conversation identifier:
    /// Telegram `chat_id` (string-encoded); Slack `im_id` (`D…`) for
    /// DMs or channel id (`C…`) for channels.
    pub conversation_id: String,

    /// Canonical sender identifier used for ACL checks. Telegram
    /// `chat_id` (string-encoded); Slack user ID (`U…`).
    pub sender_id: String,

    /// Transport-native thread token. `None` when the message is
    /// not in a thread (or `support_thread = false` for the
    /// transport).
    pub thread_id: Option<String>,

    /// Discriminator for thread semantics. Always paired with
    /// `thread_id`: `Some(_)` iff `thread_id.is_some()`.
    pub thread_kind: Option<ThreadKind>,

    /// Transport-native id of THIS message. Telegram `message_id`
    /// (string-encoded integer); Slack `event_ts` / `ts`
    /// (float-string).
    pub message_id: String,

    /// The message body.
    pub text: String,

    /// Original provider server timestamp; present when the
    /// transport supplies it (Telegram `Update.date`, Slack
    /// `event_ts`).  Stored as a string to keep transport-native
    /// formatting (Slack uses float-strings, Telegram uses unix
    /// integers).
    pub transport_timestamp: Option<String>,

    /// Makakoo's local-receive clock (NOT the transport server's
    /// reported timestamp). Captured at deserialization time in the
    /// Rust transport process so multi-transport slots have a
    /// consistent ordering.
    pub received_at: DateTime<Utc>,

    /// Transport-native extras for debugging.
    #[serde(default)]
    pub raw_metadata: BTreeMap<String, serde_json::Value>,
}

/// Outbound reply frame: Python gateway → Rust transport.
///
/// Cross-transport reply is forbidden in v1 — the router rejects an
/// outbound whose `transport_id` doesn't match an inbound from the
/// same slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakakooOutboundFrame {
    /// MUST equal an inbound frame's `transport_id` from the same
    /// slot. Cross-transport reply is forbidden in v1.
    pub transport_id: String,

    /// Type dispatch — adapter selection key.
    pub transport_kind: String,

    /// Where to send (NOT a user id). For Slack this is the
    /// channel/im id; for Telegram this is the chat_id.
    pub conversation_id: String,

    /// Only honored when `support_thread = true` for the matching
    /// transport. Otherwise dropped with a DEBUG log.
    pub thread_id: Option<String>,

    /// MUST match the inbound `thread_kind` if both are set;
    /// mismatch → adapter logs WARN and drops thread (still sends
    /// to the conversation root).
    pub thread_kind: Option<ThreadKind>,

    /// The reply body.
    pub text: String,

    /// Transport-native reply target. The demux layer COERCES this
    /// to the transport's expected type:
    ///   - Telegram: parse as integer; if parse fails → drop with WARN
    ///   - Slack: pass through as `thread_ts` float-string; if string
    ///     doesn't look like a Slack timestamp → drop with WARN
    /// Mismatched format never crashes — the message still sends,
    /// just without the thread anchor.
    pub reply_to_message_id: Option<String>,
}

/// IPC envelope. One JSON object per line over the Unix socket.
///
/// `kind` tags which variant `frame` carries so the receiving side
/// can `serde_json` deserialize into the right struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "frame", rename_all = "snake_case")]
pub enum MakakooFrame {
    Inbound(MakakooInboundFrame),
    Outbound(MakakooOutboundFrame),
}

impl MakakooFrame {
    /// Encode as a single line of newline-delimited JSON (trailing
    /// `\n` included).
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Decode a single line (trailing `\n` already stripped by the
    /// caller's line reader).
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_inbound() -> MakakooInboundFrame {
        MakakooInboundFrame {
            agent_slot_id: "secretary".into(),
            transport_id: "telegram-main".into(),
            transport_kind: "telegram".into(),
            account_id: "12345678".into(),
            conversation_id: "746496145".into(),
            sender_id: "746496145".into(),
            thread_id: None,
            thread_kind: None,
            message_id: "42".into(),
            text: "hello".into(),
            transport_timestamp: Some("1714123456".into()),
            received_at: Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap(),
            raw_metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn inbound_roundtrip() {
        let frame = sample_inbound();
        let env = MakakooFrame::Inbound(frame.clone());
        let line = env.to_line().unwrap();
        assert!(line.ends_with('\n'));
        let decoded = MakakooFrame::from_line(line.trim_end_matches('\n')).unwrap();
        match decoded {
            MakakooFrame::Inbound(d) => {
                assert_eq!(d.agent_slot_id, frame.agent_slot_id);
                assert_eq!(d.message_id, frame.message_id);
                assert_eq!(d.transport_timestamp, frame.transport_timestamp);
            }
            _ => panic!("expected inbound"),
        }
    }

    #[test]
    fn outbound_roundtrip() {
        let frame = MakakooOutboundFrame {
            transport_id: "slack-main".into(),
            transport_kind: "slack".into(),
            conversation_id: "D0123ABCD".into(),
            thread_id: Some("1714123456.000200".into()),
            thread_kind: Some(ThreadKind::SlackThread),
            text: "ack".into(),
            reply_to_message_id: Some("1714123456.000100".into()),
        };
        let env = MakakooFrame::Outbound(frame.clone());
        let line = env.to_line().unwrap();
        let decoded = MakakooFrame::from_line(line.trim_end_matches('\n')).unwrap();
        match decoded {
            MakakooFrame::Outbound(d) => {
                assert_eq!(d.transport_id, frame.transport_id);
                assert_eq!(d.thread_kind, Some(ThreadKind::SlackThread));
            }
            _ => panic!("expected outbound"),
        }
    }

    #[test]
    fn thread_kind_serializes_snake_case() {
        let line = serde_json::to_string(&ThreadKind::TelegramForum).unwrap();
        assert_eq!(line, r#""telegram_forum""#);
        let line = serde_json::to_string(&ThreadKind::SlackThread).unwrap();
        assert_eq!(line, r#""slack_thread""#);
    }

    #[test]
    fn rust_emits_exact_bytes_of_python_contract_fixture() {
        // Cross-language contract anchor: the JSON Rust emits for
        // `sample_inbound()` MUST match the bytes in
        // `plugins-core/agent-harveychat/python/tests/fixtures/sample_inbound.json`
        // (newline-stripped). The Python contract test parses that
        // same fixture; if either side drifts, both fail in lockstep.
        let frame = sample_inbound();
        let env = MakakooFrame::Inbound(frame);
        let line = env.to_line().unwrap();
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let fixture = std::path::PathBuf::from(manifest)
            .parent()
            .unwrap()
            .join("plugins-core/agent-harveychat/python/tests/fixtures/sample_inbound.json");
        let golden = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let golden = golden.trim_end_matches('\n');
        let actual = line.trim_end_matches('\n');
        // Parse both as JSON values and compare structurally — this
        // tolerates field-order differences (serde_json emits in
        // struct order, hand-edited fixture might reorder).
        let v_actual: serde_json::Value = serde_json::from_str(actual).unwrap();
        let v_golden: serde_json::Value = serde_json::from_str(golden).unwrap();
        assert_eq!(
            v_actual, v_golden,
            "Rust frame.rs serialization drifted from Python contract fixture.\n\
             Actual: {actual}\nGolden: {golden}"
        );
    }

    #[test]
    fn sender_username_field_is_absent_in_v1_frame() {
        // Negative test: confirm the deferred field name is not
        // accidentally introduced. If a future commit adds
        // `sender_username` to MakakooInboundFrame, this test fails
        // — and the SPRINT.md "v1 directory adapter deferred"
        // contract must be re-evaluated first.
        let frame = sample_inbound();
        let json = serde_json::to_value(&frame).unwrap();
        assert!(
            json.get("sender_username").is_none(),
            "sender_username must NOT be in v1 frame (Q12 deferred adapter)"
        );
    }
}
