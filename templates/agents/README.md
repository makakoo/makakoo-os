# Agent slot templates

Copy-paste-ready TOML files for the most common subagent archetypes.

## How to use

```sh
# 1. Pick a template that matches your goal
cp ~/makakoo-os/templates/agents/secretary-freelance.toml ~/secretary.toml

# 2. Open it and replace every <PLACEHOLDER> — the file fails fast if you miss one
${EDITOR:-vim} ~/secretary.toml

# 3. Stash the secrets it references
makakoo secret set agent/secretary/telegram-main/bot_token  '<paste>'
makakoo secret set agent/secretary/slack-main/bot_token     'xoxb-…'
makakoo secret set agent/secretary/slack-main/app_token     'xapp-…'

# 4. Create the slot — credentials are verified BEFORE any file is written
makakoo agent create secretary --from-toml ~/secretary.toml

# 5. Start it
makakoo agent start secretary
makakoo agent status secretary
```

## The gallery

| Template | Persona | Transports | Reach for it when… |
|---|---|---|---|
| [`secretary-freelance.toml`](./secretary-freelance.toml) | Freelance office secretary | Telegram + Slack | You want a chat-driven assistant for client management, contracts, invoices. Pairs with the `skill-freelance-office` plugin. |
| [`career-manager.toml`](./career-manager.toml) | Career / recruiter conversations | Telegram only | You want an inbox for recruiter pings + job-lead tracking, separate from the rest of your work. |
| [`alerts-bot.toml`](./alerts-bot.toml) | One-way ops/alerts | Slack only | You want infra/build/deploy notifications routed into a Slack channel. Receive-only — replies are ignored. |
| [`support-inbox.toml`](./support-inbox.toml) | Customer support agent | Email + Web chat | You want a public-facing slot that takes long-form email AND in-page chat. SMTP outbound today, IMAP IDLE in v2.1. |
| [`community-bot.toml`](./community-bot.toml) | Community / Discord guild | Discord | You want a single-guild Discord presence. MESSAGE_CONTENT off by default. |

## Picking a transport

| Need | Use |
|---|---|
| Reach Sebastian on his phone, no public URL | Telegram |
| Reach Sebastian inside his work Slack, no public URL | Slack (Socket Mode) |
| Reach a community / friend group | Discord |
| Reach a phone number over Meta | WhatsApp (needs public webhook) |
| Take phone calls | Voice (Twilio, needs public webhook) |
| Long-form correspondence with strangers | Email |
| Drop-in widget on your website | Web chat (HMAC visitor cookies) |

Multiple `[[transport]]` blocks combine — one slot can be reachable
on Telegram + Slack + Email simultaneously. **Cross-transport reply
is forbidden:** the slot replies on the channel that delivered the
inbound message, never bridges across them.

## Field reference

Every template uses the same top-level shape:

| Field | Required | Notes |
|---|---|---|
| `slot_id` | yes | ASCII alphanumeric + `-_`, 1–64 chars, must match filename stem |
| `name` | yes | Human-readable display name |
| `persona` | yes | System prompt — the "who am I and what do I do" block |
| `inherit_baseline` | no | If `true`, slot inherits Harvey's baseline tools/paths. Default `false` (least privilege). |
| `allowed_paths` | yes | Filesystem paths the slot can write to (in addition to its own data dir) |
| `forbidden_paths` | no | Explicit deny — overrides allowed_paths on conflict |
| `tools` | yes | MCP tool names + plugin command names the slot can invoke |
| `process_mode` | no | `supervised_pair` (default) — supervisor + Python gateway as a process pair |
| `[[transport]]` | yes (≥1) | One block per chat channel. See per-template TOML for shape. |

## Reference docs

- [`docs/user-manual/agent.md`](../../docs/user-manual/agent.md) — full CLI surface for `makakoo agent`
- [`docs/walkthroughs/multi-transport-subagents.md`](../../docs/walkthroughs/multi-transport-subagents.md) — flagship end-to-end walkthrough
- [`docs/walkthroughs/`](../../docs/walkthroughs/) — per-transport recipes (Discord / WhatsApp / Voice / Email / Web)
- [`docs/troubleshooting/agents.md`](../../docs/troubleshooting/agents.md) — failure modes
- [`docs/specs/http-server-security.md`](../../docs/specs/http-server-security.md) — locked HTTP-server contract for webhook transports
