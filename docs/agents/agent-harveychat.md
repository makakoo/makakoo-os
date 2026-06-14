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
makakoo plugin install --core agent-harveychat
makakoo plugin info agent-harveychat
makakoo plugin disable agent-harveychat
makakoo plugin enable agent-harveychat
makakoo daemon restart
```

Manual control:

```sh
cd ~/MAKAKOO/plugins/agent-harveychat
.venv/bin/python -u src/agent.py start --daemon
.venv/bin/python -u src/agent.py stop
```

For a VPS/systemd deployment, run the gateway in the foreground under a
systemd service instead of `--daemon`:

```ini
Environment=MAKAKOO_HOME=/home/makakoo/MAKAKOO
Environment=HARVEY_HOME=/home/makakoo/MAKAKOO
Environment=PYTHONPATH=/home/makakoo/MAKAKOO/plugins/lib-harvey-core/src:/home/makakoo/MAKAKOO/plugins/lib-hte/src
Environment=SWITCHAI_MODEL=ail-compound
Environment=HARVEYCHAT_WORKFLOWS=0
Environment=SWITCHAI_KEY=<switchAILocal API key>
Environment=TELEGRAM_ALLOWED_USERS=<telegram-user-id>
Environment=TELEGRAM_BOT_TOKEN=<bot-token>
ExecStart=/home/makakoo/MAKAKOO/plugins/agent-harveychat/.venv/bin/python -u /home/makakoo/MAKAKOO/plugins/agent-harveychat/src/agent.py start
```

## Cortex Memory

HarveyChat can run with native Cortex Memory enabled. Cortex stores durable, PII-scrubbed chat memories in the local HarveyChat SQLite database and retrieves relevant memories before each assistant turn. It also supports explicit Telegram/Discord aliases for cross-channel recall.

See [HarveyChat Cortex Memory](./harveychat-cortex-memory.md) for setup, alias commands, inspection, and rollback.

Recommended long-context Telegram config in `~/MAKAKOO/data/chat/config.json`:

```json
{
  "bridge": {
    "switchai_url": "http://localhost:18080/v1",
    "switchai_model": "ail-compound",
    "max_history_messages": 80,
    "max_tokens": 8192
  },
  "cortex": {
    "enabled": true,
    "memory_limit": 12,
    "min_confidence": 0.7,
    "min_importance": 0.4,
    "pii_scrubbing": true,
    "max_memory_chars": 1000,
    "max_prompt_memory_chars": 8000,
    "max_memory_age_days": 365,
    "app_id": "makakoo-harveychat"
  }
}
```

By default, Telegram uses the direct conversational/tool path; set `HARVEYCHAT_WORKFLOWS=1` only if you want experimental background research/image/archive workflows.

This gives HarveyChat three memory layers: recent chat history, Cortex
long-term chat memory, and Brain tool access via `brain_search` /
`brain_write`. If a bot uses a non-default persona (for example Donna on a
VPS), set `$MAKAKOO_HOME/config/persona.json`; HarveyChat injects that persona
after the global bootstrap so the channel does not regress to the factory
`Harvey` identity.

## Remote operator gates

HarveyChat/Olibia can manage the computer remotely, but only through the
same Makakoo permission system used by the CLI:

- Safe read-only diagnostics use `run_command` and stay whitelisted.
- Writes outside the default sandbox require a time-limited write grant via
  `grant_write_access`.
- Non-whitelisted shell commands require an exact action grant:
  `grant_action_access(action="shell/run", target="<exact command>")`, then
  `operator_run_command("<exact command>")`.
- Logged-in / JavaScript browser reads require an exact action grant:
  `grant_action_access(action="browser/control", target="<exact browser/read target>")`,
  then `operator_browser_read(url, query)`.
- One action grant authorizes one exact normalized target only. It does not
  create a broad shell session.
- Hard-blocked destructive or credential-exfiltration patterns stay blocked
  even if a grant exists.

Action grants live in `$MAKAKOO_HOME/config/user_grants.json` as
`action:*` scopes, emit audit entries to `$MAKAKOO_HOME/logs/audit.jsonl`,
and can be revoked from the CLI with `makakoo perms revoke <grant-id>`.

## Where it writes

- **State:** `~/MAKAKOO/state/agent-harveychat/` — last-seen message offsets, per-chat seen-set.
- **Data:** `~/MAKAKOO/data/chat/` — config, PID, logs, `conversations.db`.
- **Logs:** `~/MAKAKOO/data/chat/harveychat.log`

## Health signals

- `ps -ef | grep harveychat` — one running process.
- Recent `harveychat.log` entries showing Telegram polling.
- Sending `/status` to the bot returns gateway health within a few seconds.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| No reply when you message the bot | Your chat is not on the allowlist | Add your chat ID to `~/MAKAKOO/config/harveychat/config.toml` under `allowed_chats`. |
| `401 Unauthorized` in logs | Bot token missing or wrong | `makakoo secret set telegram.bot_token`; restart the daemon. |
| Bot silently stops responding after a reboot | `agent-harveychat` didn't auto-restart | Check `makakoo daemon status`; re-infect if needed via `makakoo daemon restart`. |
| Message flood loop (bot replies to its own messages) | Bot-ignore patch not applied | Update to the latest version of the plugin; the fix landed 2026-04-10. |
| Non-whitelisted command rejected | No exact `shell/run` action grant | Approve the exact command in chat, or revoke/inspect grants with `makakoo perms list --json`. |
| Logged-in page read rejected | No exact `browser/control` action grant | Approve the exact browser target in chat; if Chrome CDP is down, start Chrome with `--remote-debugging-port=9222`. |

## Capability surface

- `net/http:api.telegram.org` — Telegram bot API.
- `secret/read:telegram.*` — read the bot token.
- `fs/read:$MAKAKOO_HOME/plugins/agent-harveychat`
- `fs/write:$MAKAKOO_HOME/data/chat`
- `llm/chat` — answer synthesis.
- `action:shell/run:<hash>` — optional exact remote-operator shell actions,
  only after explicit user grant.
- `action:browser/control:<hash>` — optional exact real-browser reads,
  only after explicit user grant.

## Remove permanently

```sh
makakoo plugin uninstall agent-harveychat --purge
```
