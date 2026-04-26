# Spec — HTTP server security

**Status:** Locked by SPRINT-MULTI-BOT-SUBAGENTS-V2.0-MEGA Phase 13.
Documents the contract every transport's webhook + WS handler MUST
honor. Auditors should treat this as ground truth; implementations
that diverge are bugs.

## Surface

`makakoo-mcp --http` exposes one bound listener:

```
/health                                                       — readiness probe
/transport/<slot_uuid>/<transport_uuid>/<kind>                — webhook (any HTTP method)
/transport/<slot_uuid>/<transport_uuid>/<kind>/ws             — WS upgrade
/rpc                                                          — Ed25519-signed JSON-RPC (existing)
```

`<slot_uuid>` and `<transport_uuid>` are 36-char hex-with-dashes
(UUID v4 wire shape, no `uuid` crate dep). Bad shape → 404 (not
401), so probe traffic doesn't enumerate the registry.

## Verify-before-parse contract

Every `WebhookHandler::verify` runs against the buffered raw body
BEFORE `WebhookHandler::handle` parses it. Parse errors after a
verify FAIL would leak signal about which fields the body had —
defeating the whole HMAC. The `webhook_dispatch` function in
`webhook_router.rs` reads the body once, hands the same `Bytes` to
both verify and handle.

## Per-handler signature requirements

| Transport | Algorithm | Header | Notes |
|---|---|---|---|
| Slack Events | HMAC-SHA256 | `X-Slack-Signature: v0=...` | 5-min replay window enforced |
| WhatsApp | HMAC-SHA256 | `X-Hub-Signature-256: sha256=...` | GET handshake bypasses HMAC; verify_token equality only |
| Twilio Voice | HMAC-SHA1 | `X-Twilio-Signature: ...` | Body=sorted form params concatenated to URL |
| Web chat WS | HMAC-SHA256 cookie | `Cookie: makakoo_web_visitor=...` | Verified at upgrade; expired/malformed cookies replaced via `Set-Cookie` |

## Origin allowlist (WS only)

Web chat enforces `WebConfig.allowed_origins` with one exception:
loopback origins (`localhost`, `127.0.0.1`, `::1`, `[::1]`) are
accepted in dev mode. `production_mode = true` requires a non-empty
`allowed_origins` and rejects loopback.

`Set-Cookie` drops the `Secure` attribute IFF the request was on
loopback. Production cookies always carry `Secure`.

## Cookie shape (Web chat)

```
makakoo_web_visitor=<visitor_id>.<exp_unix>.<hex(HMAC-SHA256)>; \
  Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=2592000
```

- `visitor_id`: 16-char hex (8 random bytes)
- `exp_unix`: visitor cookie expiry, default 30 days
- HMAC over `<visitor_id>.<exp_unix>` using the persisted key
- Key persisted to `$MAKAKOO_HOME/keys/web-chat-hmac` mode 0600
- Constant-time signature comparison

## Status codes

| Failure | Code | Audit kind |
|---|---|---|
| Unknown slot/transport UUID shape | 404 | (none — pre-registry) |
| No handler registered | 404 | (none) |
| Signature missing | 401 | `webhook.invalid_signature` |
| Signature mismatch | 401 | `webhook.invalid_signature` |
| Replay window exceeded | 401 | `webhook.invalid_signature` |
| Cookie missing/expired (WS) | 401 | `webhook.bad_cookie` |
| Origin not allowlisted (WS) | 403 | `webhook.bad_origin` |
| Generic 4xx (BadRequest) | 400 | `webhook.bad_request` |

The audit kind names are stable wire identifiers — surfaced through
`makakoo agent audit --kind <name>`.

## Graceful shutdown

`SHUTDOWN_DRAIN = 30s`. On SIGTERM, the dispatcher stops accepting
new connections and waits up to the drain for in-flight handlers.
Long-running WS sessions are dropped with a `WARN` log — Q10
trade-off: better than blocking shutdown indefinitely.

## Route isolation

The `/rpc` Ed25519-signed surface and the `/transport/...` HMAC
surface share the same axum process but **never delegate auth**:

- `/rpc` middleware verifies Ed25519 from the per-peer signing key
- `/transport/...` middleware is per-handler HMAC

A request that authenticates on `/rpc` does NOT inherit any
permission on `/transport/...`. The boundary is route-tree
separation; there is no cross-route auth shim.

## Redaction in audit log

Locked Q14:

- Secrets and tokens: NEVER logged. Audit writer redacts keys named
  `secret_value`, `password`, `token`, `bot_token`, `api_key`,
  `signing_secret`, `client_secret`, `body`, `text`.
- Actor and target identifiers (`alice@example.com`, `U001`,
  `+34600000001`): logged in full. Forensics need them.
- File paths in scope-violation events: logged in full.
- Raw HTTP bodies: NEVER logged. Hash + size only when a debug flag
  asks for it.

## Files

- `makakoo-mcp/src/webhook_router.rs` — dispatch, route shape
- `makakoo-mcp/src/slack_events.rs` — Slack handler
- `makakoo-mcp/src/whatsapp_webhook.rs` — WhatsApp handler
- `makakoo-mcp/src/twilio_voice_webhook.rs` — Twilio Voice handler
- `makakoo-mcp/src/web_chat_ws.rs` — Web chat WS handler
- `makakoo-core/src/transport/web.rs` — cookie sign/verify + origin checks
- `makakoo-core/src/agents/audit.rs` — JSONL writer + redaction
