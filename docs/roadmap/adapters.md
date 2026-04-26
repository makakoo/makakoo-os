# Transport adapter roadmap

Status of every transport adapter Makakoo's `transport::Transport`
trait can hold, plus the design boundaries for the deferred
OpenClaw-parity adapters.

## v1 (shipped — SPRINT-MULTI-BOT-SUBAGENTS)

| Adapter | Status | Notes |
|---|---|---|
| **Telegram** (long-poll) | ✅ shipped | `getMe` verifier, `getUpdates` poller, `sendMessage` outbound, reply_to_message_id i64 coercion, self-loop suppression, per-transport `allowed_users` |
| **Slack** (Socket Mode) | ✅ shipped | `auth.test` + `apps.connections.open` verifier, WebSocket loop with 1s→60s exponential reconnect + `status.reconnecting` log, 5-min `(channel, ts)` dedup window, subtype suppression, self-loop via cached `auth.test.user_id`, per-transport `allowed_users`, dm_only / channels allowlist |

## Follow-on adapters

These are deferred to follow-up sprints. The `Transport` trait is
designed so each can ship as an isolated PR — no Makakoo core
changes required beyond adding the `kind = "<name>"` discriminator
to the schema validator.

### Slack Events API (webhook production path)

**Status:** post-v1.

v1 ships Socket Mode only — adequate for laptop / single-server
operator, inadequate for HA / multi-instance deployments where
each Makakoo instance would compete to receive the same WebSocket
event.

The webhook variant requires:

- A public HTTPS endpoint (cloudflare tunnel, ngrok, or a real
  load balancer in front of `makakoo-mcp serve`).
- A new `mode = "webhook"` discriminator on `SlackConfig`.
- A second axum route on `makakoo-mcp` that verifies Slack's
  signing-secret header (HMAC-SHA256 over the raw body), then
  dispatches to the same `MakakooInboundFrame` constructor the
  Socket Mode loop already calls.
- The Slack signing secret added as a third secret slot on the
  `[[transport]]` block:
  `signing_secret_ref` / `signing_secret_env`.

Estimated cost: ~150 LOC + 8 tests. No risk to the existing
Socket Mode path (mutually exclusive `mode` discriminator).

### Discord

**Status:** post-v1.

Discord's gateway is a WebSocket much like Slack's Socket Mode but
with heartbeat semantics and a different envelope schema. The
`tokio-tungstenite` dep is already vendored for Slack — reusing it
is cheap. The Discord-specific concerns:

- Sharding: a single bot in a large server ecosystem may need
  multiple gateway connections. v2 of this adapter can ship
  single-shard; multi-shard is a v3 concern.
- `serenity` crate is mature and adds ~3MB to the binary; rolling
  our own using `tokio-tungstenite` keeps the binary lean but
  doubles the LOC. v1 of the Discord adapter should pick one and
  document the trade-off.

### WhatsApp

**Status:** post-v1.

WhatsApp Cloud API + Meta's developer onboarding is the
realistic path. Pairing is OAuth-via-business-portal (heavyweight
compared to Slack's app-token). The adapter shape mirrors Slack
Events API (HTTPS webhooks with HMAC verification) more than
Telegram (polling). Defer until the Slack webhook path is
established — they share most of the routing layer.

### Email

**Status:** post-v1.

IMAP IDLE for inbound, SMTP for outbound. Largely outside the
real-time messaging idiom — replies are minutes-to-hours, not
seconds — so the gateway / queue model needs adjustment (e.g.
batched inbound). Useful for the secretary slot's
"forward-this-to-me-when-an-email-arrives" workflows.

### Voice

**Status:** post-v1.

Twilio Voice or LiveKit. Inbound is audio → STT → text frame;
outbound is text → TTS → audio. The `transport_kind = "voice"`
fits the existing schema; the new concern is the audio payload
which doesn't fit `MakakooInboundFrame.text: String`. Either:

- Add a `MakakooInboundFrame::Audio { url: String, transcript:
  Option<String> }` variant, OR
- Force STT to happen at the transport layer so the LLM sees
  text only.

The latter is simpler; the former preserves audio for tools that
might want it (sentiment analysis, voice fingerprinting).

### Web chat

**Status:** post-v1.

A self-hosted chat widget that talks to `makakoo-mcp` over a
WebSocket. Useful for "embed Olibia on a webpage" use cases.
Schema-wise it looks like Slack Socket Mode without the
multi-tenant `team_id` constraint.

## Deferred OpenClaw-parity seams

Phase 1 implements 6 of the 10 OpenClaw `ChannelPlugin` adapter
traits. The remaining 4 are deferred — each warrants its own
follow-on sprint:

| Adapter | Purpose | Deferral rationale |
|---|---|---|
| `ChannelDirectoryAdapter` | Resolve `sender_username` from `sender_id` | v1 frames omit `sender_username` entirely; downstream code uses `sender_id` for both ACL and display. Adding the directory adapter unlocks "tag the agent's response with the sender's display name" features. |
| `ChannelApprovalAdapter` | Approval flows for high-risk actions ("approve this transfer") | The user_grants three-layer model already covers approval-on-write. Approval-on-arbitrary-action is a v2+ concern. |
| `ChannelMessagingAdapter` | Format-aware outbound (rich blocks, buttons, attachments) | v1 outbound is plain text only. Slack Block Kit / Telegram inline keyboards land here. |
| `ChannelThreadingAdapter` | First-class thread lifecycle (create/move/merge threads) | v1 supports `thread_id` propagation but doesn't let agents create new threads programmatically. |

## Per-agent LLM model override

**Status:** deferred (Phase 3 non-goal).

Per-spec, all subagents currently share the machine-level
`switchai_model` and `max_tokens` config from `BridgeConfig`. The
case for per-agent override:

- Career-manager could use a cheaper model (`mimo-v2-lite`) for
  recruiter-reply drafting.
- Arbitrage-agent could use a higher-context model
  (`opus-4.7-1m`) for multi-instrument analysis.
- Olibia-on-Telegram could use a faster model (`haiku-4.5`) for
  conversational latency.

The schema slot is reserved: future TOML can add
`[llm.override]` with `model`, `max_tokens`, `temperature`,
`reasoning_effort`. The dispatcher then layers per-agent over
machine-level. Tracking ticket: tbd.
