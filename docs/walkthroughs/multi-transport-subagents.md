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
go through `--from-toml` for this case):

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
