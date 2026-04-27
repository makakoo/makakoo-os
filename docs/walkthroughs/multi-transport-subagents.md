# Multi-bot subagents — end-to-end walkthrough

This walkthrough takes you from a clean Makakoo install through three
live subagents — one of them reachable from both Telegram AND Slack
simultaneously. By the end you will have:

- `harveychat` — the legacy Olibia Telegram bot, migrated to a slot.
- `secretary` — a NEW slot reachable on @SecretaryBot (Telegram) AND
  in Slack via Socket Mode.
- `career` — a Telegram-only career-manager slot.

Each slot has its own persona, tools, paths, and per-transport
allowlist. They share one Brain, one MCP server, and one user-grant
store but each grant is bound to its issuing slot (Phase 3
`bound_to_agent`).

---

## 0. Prerequisites

| Need | How to get it |
|---|---|
| Two Telegram bot tokens | `@BotFather` → `/newbot` × 2 (one for Secretary, one for Career; the Olibia bot already exists) |
| One Slack app + bot token + app token | `https://api.slack.com/apps` → Create New App → Socket Mode → Install to Workspace |
| Sebastian's chat_id | `@userinfobot` in Telegram → reply with your numeric id |
| Sebastian's Slack user id | Click your name in any Slack workspace → "More" → "Copy member ID" → `U…` |

You do NOT need a public webhook endpoint — Slack Socket Mode dials out
from your laptop to `wss.slack.com`, so it works behind NAT.

---

## 1. Migrate the legacy HarveyChat bot

```sh
makakoo agent migrate-harveychat
```

Expected output:

```
harveychat migrated: ~/MAKAKOO/config/agents/harveychat.toml ← data/chat/config.json
  legacy conversations.db archived at ~/MAKAKOO/data/agents/harveychat/conversations.db.bak
  legacy config.json archived at ~/MAKAKOO/data/agents/harveychat/config.json.bak
  fresh conversations.db seeded at ~/MAKAKOO/data/agents/harveychat/conversations.db
```

The original `data/chat/config.json` is preserved (not deleted) for
rollback safety. Re-running the command is idempotent — it backfills
any missing artifacts and reports `already migrated`.

Verify the migration:

```sh
makakoo agent list
```

```
SLOT                    NAME                    STATUS        TRANSPORTS
harveychat              Olibia                  OK            telegram-main(telegram)
```

---

## 2. Create the secretary slot (Telegram + Slack)

Stash the secrets in the makakoo keyring first so the TOML doesn't
carry them inline:

```sh
makakoo secret set agent/secretary/telegram-main/bot_token  '<paste-bot-token>'
makakoo secret set agent/secretary/slack-main/bot_token     'xoxb-…'
makakoo secret set agent/secretary/slack-main/app_token     'xapp-…'
```

Build a TOML file (multi-transport slots aren't constructible from
flags alone — `--telegram-token` + `--slack-bot-token` together still
go through `--from-toml` for this case).

**Shortcut:** [`templates/agents/secretary-freelance.toml`](../../templates/agents/secretary-freelance.toml)
is exactly this archetype — copy it, swap the `<PLACEHOLDER>` chat IDs,
done. The full TOML is shown below for reference:

```toml
# ~/secretary.toml
slot_id = "secretary"
name    = "Secretary"
persona = "Sharp professional secretary for Sebastian's freelance office. Drafts emails, schedules meetings, drafts invoices. Never auto-sends — always confirms first."
inherit_baseline = false

allowed_paths   = ["~/MAKAKOO/data/secretary/", "~/Office/"]
forbidden_paths = ["~/CV/", "~/MAKAKOO/data/career/"]
tools           = ["brain_search", "write_file", "gmail", "google-calendar"]

process_mode = "supervised_pair"

[[transport]]
id      = "telegram-main"
kind    = "telegram"
enabled = true
secret_ref = "agent/secretary/telegram-main/bot_token"
secret_env = "SECRETARY_TELEGRAM_MAIN_TOKEN"
allowed_users = ["746496145"]   # your Telegram chat_id

[transport.config]
polling_timeout_seconds = 30
support_thread = true

[[transport]]
id      = "slack-main"
kind    = "slack"
enabled = true
secret_ref     = "agent/secretary/slack-main/bot_token"
app_token_ref  = "agent/secretary/slack-main/app_token"
allowed_users  = ["U0123ABCD"]   # your Slack user id

[transport.config]
team_id = "T0123ABCD"
mode    = "socket"
dm_only = true
support_thread = true
```

Then create the slot:

```sh
makakoo agent create secretary --from-toml ~/secretary.toml
```

`agent create` runs `getMe` (Telegram) and `auth.test` +
`apps.connections.open` (Slack) BEFORE writing any files. If any
credential fails, no TOML is written and the command exits non-zero
with the per-transport failure.

Verify:

```sh
makakoo agent show secretary       # secrets are redacted
makakoo agent validate secretary   # re-runs the credential check
```

---

## 3. Create the career slot (Telegram-only)

```sh
makakoo secret set agent/career/telegram-main/bot_token '<paste-token>'

makakoo agent create career \
  --name "Career Manager" \
  --persona "Tracks job leads, recruiter conversations, contract negotiations. Drafts replies to recruiters; never auto-sends." \
  --allowed-paths "~/CV/,~/MAKAKOO/data/career/" \
  --tools "brain_search,write_file,linkedin,gmail" \
  --telegram-token '<paste-token>' \
  --telegram-allowed "746496145"
```

(Single-transport flag mode — no `--from-toml` needed.)

---

## 4. Start everything

```sh
makakoo agent start harveychat
makakoo agent start secretary
makakoo agent start career
```

In another terminal:

```sh
makakoo agent status secretary
```

Expected layout (Phase 4 locked):

```
secretary
  gateway:   alive   pid=12345     last_frame=2s ago
  transport slack-main:     connected     last_inbound=3m ago    errors_1h=0  queue_depth=0
  transport telegram-main:  connected     last_inbound=8s ago    errors_1h=0  queue_depth=0
```

---

## 5. Live dual-transport test

1. Send `@SecretaryBot hello` in Telegram → secretary replies in
   Telegram.
2. Open a DM with `@SecretaryBot` in Slack → secretary replies in
   Slack.
3. Send `@SecretaryBot what did I just say?` in Slack — secretary's
   reply should reference the Telegram turn (one conversation per
   slot, transports interleave).
4. Check `makakoo agent status secretary` again — both transports
   show recent `last_inbound`, both show `errors_1h=0`.

---

## 6. Cross-transport reply is forbidden

Slack inbound → Telegram outbound is rejected at the router. The LLM
cannot work around this. Phase 1 IPC contract: an outbound reply
MUST go to the **originating** `transport_id` of the current inbound
turn. Even when the same slot has received messages on both
Telegram and Slack, the secretary can never switch transports
mid-turn — replies are pinned to the channel that delivered the
inbound message they're answering. If you ask the secretary to
"send the reply via Telegram instead", it will tell you it can
only reply on the channel you addressed.

---

## 7. Cleanup

```sh
makakoo agent stop secretary
makakoo agent stop career
```

To fully decommission a slot today, manually:

```sh
rm ~/MAKAKOO/config/agents/<slot>.toml
mv ~/MAKAKOO/data/agents/<slot> ~/MAKAKOO/archive/<slot>-$(date +%s)
makakoo secret delete agent/<slot>/telegram-main/bot_token
# …delete any other secret refs the slot used
```

A first-class `makakoo agent destroy <slot>` (interactive
teardown — stops the process, archives TOML + DB to
`~/.makakoo/archive/agents/<slot>-<ts>/`, optionally revokes the
bot token) is a Phase 5 follow-up.

`harveychat` is preserved indefinitely — it carries the legacy
Olibia conversation history.

---

## Troubleshooting

See `docs/troubleshooting/agents.md` for common failure modes:
- Slack `team_id mismatch`
- Telegram `Unauthorized` (token revoked)
- IPC `gateway_unavailable` drops
- `bound_to_agent` grant invisibility
- Webhook 401 (Slack / WhatsApp / Twilio signature mismatch)
- Discord intent gating (MESSAGE_CONTENT off behavior)
- Web chat Origin rejection in production

---

## 8. Beyond Telegram + Slack — the v2-MEGA transports

v2.0 adds five more transport kinds. Each ships with a per-transport
walkthrough in `docs/walkthroughs/`; this section is the index +
trade-off summary so you know which to reach for.

| Kind | When to use | Walkthrough |
|---|---|---|
| `discord` | Reach users where they already hang out (gaming, communities). Per-guild allowlist. MESSAGE_CONTENT default OFF (privileged Discord intent — leave off unless you genuinely need to read every guild message). | `discord-bot.md` |
| `whatsapp` | Reach phones over Meta's network. Cloud API only. Inbound media → polite drop-reply (no STT in v1). | `whatsapp-business.md` |
| `voice_twilio` | Inbound phone calls. Push-to-talk (caller leaves a message, slot processes it). Real-time streaming + LLM-driven `<Play>` deferred to v2.1. | `voice-quickstart.md` |
| `email` | Long-form correspondence with proper Message-ID threading. v2.0 ships SMTP outbound + a parser for inbound; full IMAP IDLE listener lands in v2.1. | `email-secretary.md` |
| `web` | Drop-in chat widget for your website / dashboard. HMAC-SHA256 visitor cookies; Origin allowlist required in production. | `web-chat-demo.html` (static client) |

### Mixing transports on one slot

Same rules as Section 6: cross-transport reply is forbidden. A
secretary slot reachable on Telegram + Discord + Email replies on
the same channel that delivered the inbound — the LLM cannot
"send the reply via email instead". Multi-transport slots are
about reach, not about cross-channel routing.

### Webhook-vs-listener model

| Model | Transports | What runs |
|---|---|---|
| Per-task listener | `telegram` (long-poll), `slack` (Socket Mode), `discord` (gateway WS) | One async task per slot+transport; supervisor restarts on crash. |
| Shared webhook router | `whatsapp`, `voice_twilio` | One axum HTTP server hosts all webhooks at `/transport/<slot_uuid>/<transport_uuid>/<kind>`; HMAC verify before parse. |
| Shared WS upgrade | `web` | Same router, `/ws` suffix; cookies issued at upgrade. |
| Outbound only (v2.0) | `email` | SMTP via lettre; the IMAP IDLE listener is v2.1. |

The Q15 contract says webhook handlers MUST verify the signature
against the **buffered raw body** before parsing — see
`docs/specs/http-server-security.md` for the locked status-code
matrix + redaction rules.

### Common ops once a slot is running

```sh
makakoo agent status secretary           # per-transport health + RSS
makakoo agent audit secretary --last 30  # recent audit events
makakoo agent restart secretary          # graceful supervisor restart
makakoo agent destroy secretary --revoke-secrets  # clean teardown
```

---

## 9. Section 7 superseded — `agent destroy` is a first-class command

The "manually rm + mv + secret delete" footnote at the end of
Section 7 was a Phase-5 placeholder. v2-MEGA ships
`makakoo agent destroy <slot>` which:

- Stops the supervisor (if running)
- Archives the TOML + data dir to `$MAKAKOO_HOME/archive/agents/<slot>-<unix_ts>/`
- Lists detected secret refs from the TOML and prompts whether
  to revoke them
- Refuses to destroy `harveychat` without `--really-destroy-harveychat`
  (legacy Olibia conversation history protection)

Secrets are PRESERVED by default unless `--revoke-secrets` is
passed; this lets you re-create the slot tomorrow and reuse the
same keys.
