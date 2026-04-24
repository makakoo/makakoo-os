# `agent-harveychat`

**Summary:** Harvey's external messaging gateway — Telegram today, WhatsApp and Slack planned.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-harveychat/`

## When to use

When you want to talk to Harvey from **outside the terminal** — a Telegram chat on your phone, a Slack DM, etc. Harvey receives the message, routes it through the Brain, optionally calls an LLM, and replies in the same channel.

Not needed for in-terminal usage — that's what the infected AI CLIs cover.

## Prerequisites

- A Telegram bot token (from `@BotFather`) stored in the Makakoo keyring:

  ```sh
  makakoo secret set telegram.bot_token
  ```

- Your Telegram user ID and (optional) an allowlist of chats the bot will respond in — stored at `~/MAKAKOO/config/harveychat/config.toml`.

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-harveychat
makakoo plugin disable agent-harveychat
makakoo plugin enable agent-harveychat
makakoo daemon restart
```

Manual control:

```sh
cd ~/MAKAKOO/plugins/agent-harveychat
python3.11 -u src/agent.py start --daemon
python3.11 -u src/agent.py stop
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-harveychat/` — last-seen message offsets, per-chat seen-set.
- **Data:** `~/MAKAKOO/data/harveychat/` — message archive (per-chat JSONL).
- **Logs:** `~/MAKAKOO/data/logs/agent-harveychat.{out,err}.log`

## Health signals

- `ps -ef | grep harveychat` — one running process.
- Recent `stdout.log` entries showing `polled` or `pushed` lines.
- Sending `/ping` to the bot returns a reply within a few seconds.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| No reply when you message the bot | Your chat is not on the allowlist | Add your chat ID to `~/MAKAKOO/config/harveychat/config.toml` under `allowed_chats`. |
| `401 Unauthorized` in logs | Bot token missing or wrong | `makakoo secret set telegram.bot_token`; restart the daemon. |
| Bot silently stops responding after a reboot | `agent-harveychat` didn't auto-restart | Check `makakoo daemon status`; re-infect if needed via `makakoo daemon restart`. |
| Message flood loop (bot replies to its own messages) | Bot-ignore patch not applied | Update to the latest version of the plugin; the fix landed 2026-04-10. |

## Capability surface

- `net/http:api.telegram.org` — Telegram bot API.
- `secret/read:telegram.*` — read the bot token.
- `fs/read:$MAKAKOO_HOME/plugins/agent-harveychat`
- `fs/write:$MAKAKOO_HOME/data/harveychat`
- `llm/chat` — answer synthesis.

## Remove permanently

```sh
makakoo plugin uninstall agent-harveychat --purge
```
