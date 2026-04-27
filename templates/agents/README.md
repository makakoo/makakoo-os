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

### Tier 1 — set-up-once, payback-every-week

| Template | Persona | Transports | Reach for it when… |
|---|---|---|---|
| [`secretary-freelance.toml`](./secretary-freelance.toml) | Freelance office secretary | Telegram + Slack | You want a chat-driven assistant for client management, contracts, invoices. Pairs with the `skill-freelance-office` plugin. |
| [`invoice-chaser.toml`](./invoice-chaser.toml) | Polite escalating dunning | Email | You want the day-7 / 14 / 30 follow-up cadence on overdue invoices automated, with approval gate. Pairs with secretary. |
| [`expense-receipts.toml`](./expense-receipts.toml) | Tax-category receipt filer | Telegram + Email | You want to snap a receipt photo (or forward an e-receipt) and have it filed under the right tax bucket. Pairs with secretary. |
| [`meeting-prep.toml`](./meeting-prep.toml) | Brain × Calendar briefer | Telegram | You want a one-pager pushed 30min before every calendar event — attendees, last contact, talking points, open commitments. |
| [`lead-qualifier.toml`](./lead-qualifier.toml) | First-touch sales filter | Email + Web | You want website + sales@ inquiries triaged with 3 qualifying questions before they hit your calendar. Distinct from support-inbox. |

### Tier 2 — situational but high-leverage

| Template | Persona | Transports | Reach for it when… |
|---|---|---|---|
| [`client-boundary-bouncer.toml`](./client-boundary-bouncer.toml) | Calm "no / later / paid scope" voice | WhatsApp + Email | You want scope-creep requests filtered against the active contract, with three reply shapes (no / later / paid add-on) drafted for approval. |
| [`subscription-watch.toml`](./subscription-watch.toml) | Zombie-SaaS detector | Email + Telegram | You suspect you're hemorrhaging $200-800/mo on tools you don't use. Watches Gmail receipts, flags rate hikes / trial conversions / 60-day-idle vendors. |
| [`career-manager.toml`](./career-manager.toml) | Career / recruiter conversations | Telegram only | You want an inbox for recruiter pings + job-lead tracking, separate from the rest of your work. |
| [`support-inbox.toml`](./support-inbox.toml) | Customer support agent | Email + Web chat | You want a public-facing slot that takes long-form email AND in-page chat. SMTP outbound today, IMAP IDLE in v2.1. |

### Tier 3 — narrow but useful in context

| Template | Persona | Transports | Reach for it when… |
|---|---|---|---|
| [`alerts-bot.toml`](./alerts-bot.toml) | One-way ops/alerts | Slack only | You want infra/build/deploy notifications routed into a Slack channel. Receive-only — replies are ignored. |
| [`community-bot.toml`](./community-bot.toml) | Community / Discord guild | Discord | You want a single-guild Discord presence. MESSAGE_CONTENT off by default. |

### How the tiers were picked

Tier 1 + 2 templates were curated 2026-04-27 via cross-validator
review (`lope ask` against claude + gemini + opencode + pi). Two
validators (claude, gemini) independently picked the same five Tier-1
archetypes from a 10-candidate shortlist; `client-boundary-bouncer`
was codex's standalone pick; `subscription-watch` was claude's
write-in. Three candidates were cut as toy/duplicate by both
validators (`focus-guard`, `on-call-router`, `journal-companion`).

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
