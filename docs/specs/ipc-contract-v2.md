# IPC contract v2 — Rust supervisor ↔ Python gateway

**Status:** locked, Phase 3 of v2-mega.
**Mirror:** `makakoo-core/src/transport/frame.rs`.

This document is the source of truth for any alternate-language
gateway that wants to participate in the per-slot supervisor
lifecycle. The Python reference gateway at
`plugins-core/agent-harveychat/python/` is the canonical
implementation.

---

## Transport

- Unix-domain socket at `~/MAKAKOO/run/agents/<slot_id>/ipc.sock`
- Newline-delimited JSON (one frame per line)
- At-most-once delivery — frames dropped during gateway downtime are
  NOT replayed
- Per-stream tokio `Mutex` on the Rust side serializes writes
- Gateway connects as client; supervisor binds as server
- On disconnect, gateway must reconnect with exponential backoff
  (500ms → 30s, jittered)

## Frame envelope

Every line is one JSON object with two top-level fields:

```json
{"kind": "inbound", "frame": { ... }}
{"kind": "outbound", "frame": { ... }}
```

`kind` is `"inbound"` for supervisor→gateway frames and
`"outbound"` for gateway→supervisor frames.

## Inbound (supervisor → gateway)

```json
{
  "kind": "inbound",
  "frame": {
    "agent_slot_id": "secretary",
    "transport_id": "telegram-main",
    "transport_kind": "telegram",
    "account_id": "12345678",
    "conversation_id": "746496145",
    "sender_id": "746496145",
    "thread_id": null,
    "thread_kind": null,
    "message_id": "42",
    "text": "hello",
    "transport_timestamp": "1700000000",
    "received_at": "2026-04-26T12:00:00.000000000Z",
    "raw_metadata": {}
  }
}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `agent_slot_id` | string | yes | the slot id that owns this transport |
| `transport_id` | string | yes | PRIMARY routing key — outbound MUST echo this |
| `transport_kind` | string | yes | `"telegram"`, `"slack"`, `"discord"`, `"whatsapp"`, `"email"`, `"voice_twilio"`, `"web"` |
| `account_id` | string | yes | bot identity (Telegram getMe.id, Slack U…, etc.) |
| `conversation_id` | string | yes | where to reply (chat_id, channel id, IM id, ...) |
| `sender_id` | string | yes | canonical user identifier for ACL checks |
| `thread_id` | string \| null | yes | transport-native thread token |
| `thread_kind` | enum \| null | yes | `"telegram_forum"` or `"slack_thread"` |
| `message_id` | string | yes | provider id of THIS message |
| `text` | string | yes | message body |
| `transport_timestamp` | string \| null | yes | provider server timestamp |
| `received_at` | string (RFC3339) | yes | Makakoo's local-receive clock |
| `raw_metadata` | object | yes | transport-native extras |

## Outbound (gateway → supervisor)

```json
{
  "kind": "outbound",
  "frame": {
    "transport_id": "telegram-main",
    "transport_kind": "telegram",
    "conversation_id": "746496145",
    "thread_id": null,
    "thread_kind": null,
    "text": "hi back",
    "reply_to_message_id": "42"
  }
}
```

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `transport_id` | string | yes | MUST equal an inbound from the same slot — cross-transport reply forbidden |
| `transport_kind` | string | yes | adapter selection key |
| `conversation_id` | string | yes | reply target (NOT a user id) |
| `thread_id` | string \| null | yes | only honored when transport supports threads |
| `thread_kind` | enum \| null | yes | must match the inbound's thread_kind if set |
| `text` | string | yes | reply body |
| `reply_to_message_id` | string \| null | yes | transport-native reply target |

## Identity block (gateway-side rendering)

The Python reference gateway prefixes every LLM call with an
identity block derived from the inbound frame + the slot's `slot.toml`:

```
[agent: secretary]
[transport: telegram-main (telegram)]
[user: 746496145]
[scope.tools: brain_search, write_file, gmail, google-calendar]
[scope.paths.allowed: ~/MAKAKOO/data/secretary/, ~/Office/]
[scope.paths.forbidden: ~/CV/, ~/MAKAKOO/data/career/]
```

The identity block is locked by Phase 3 of v1
(`agents::identity::render_identity_block`). The Python
implementation MUST produce the same block bytes for the same input.

## Scope enforcement (defense in depth)

The Rust MCP/grant layer is the **authoritative** scope enforcer.
The Python gateway preflight-checks `tools` and `allowed_paths` /
`forbidden_paths` purely as a UX optimization — the LLM sees a
friendlier error than a 403 from the MCP tool. All scope violations
write an audit log entry from the Rust side regardless of whether the
preflight caught them.

## Brain attribution

The Python gateway prefixes every Brain journal line it writes with
`[agent:<slot_id>]`. The supervisor pre-issues a write grant for the
slot's Brain journal directory with `bound_to_agent = Some(slot_id)`
so attribution is enforced even if the gateway forgets the prefix.

## Cross-transport reply forbidden

An outbound frame whose `transport_id` does not match an inbound from
the same slot is rejected by the Rust router with a WARN log. The
gateway must always echo `transport_id` from the inbound frame it is
answering — a slot may receive on Telegram AND Slack AND Discord, but
each reply pins to the channel of the inbound it answers.

## Versioning

This contract is `v2`. The transport-kind enum will grow as Phases
7-11 ship Discord / WhatsApp / Email / Voice / Web. Frame schema
changes carry a coordinated bump (Rust `Cargo.toml` version + Python
`__version__` constant).
