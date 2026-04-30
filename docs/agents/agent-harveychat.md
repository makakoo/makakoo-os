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

## Cortex Memory

HarveyChat can run with native Cortex Memory enabled. Cortex stores durable, PII-scrubbed chat memories in the local HarveyChat SQLite database and retrieves relevant memories before each assistant turn. It also supports explicit Telegram/Discord aliases for cross-channel recall.

See [HarveyChat Cortex Memory](./harveychat-cortex-memory.md) for setup, alias commands, inspection, and rollback.

## Remote operator gates

HarveyChat/Olibia can manage the computer remotely, but only through the
same Makakoo permission system used by the CLI:

- Safe read-only diagnostics use `run_command` and stay whitelisted.
- Writes outside the default sandbox require a time-limited write grant via
  `grant_write_access`.
- Non-whitelisted shell commands require an exact action grant:
  `grant_action_access(action="shell/run", target="<exact command>")`, then
  `operator_run_command("<exact command>")`.
- One action grant authorizes one exact normalized target only. It does not
  create a broad shell session.
- Hard-blocked destructive or credential-exfiltration patterns stay blocked
  even if a grant exists.

Action grants live in `$MAKAKOO_HOME/config/user_grants.json` as
`action:*` scopes, emit audit entries to `$MAKAKOO_HOME/logs/audit.jsonl`,
and can be revoked from the CLI with `makakoo perms revoke <grant-id>`.

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
| Non-whitelisted command rejected | No exact `shell/run` action grant | Approve the exact command in chat, or revoke/inspect grants with `makakoo perms list --json`. |

## Capability surface

- `net/http:api.telegram.org` — Telegram bot API.
- `secret/read:telegram.*` — read the bot token.
- `fs/read:$MAKAKOO_HOME/plugins/agent-harveychat`
- `fs/write:$MAKAKOO_HOME/data/harveychat`
- `llm/chat` — answer synthesis.
- `action:shell/run:<hash>` — optional exact remote-operator shell actions,
  only after explicit user grant.

## Remove permanently

```sh
makakoo plugin uninstall agent-harveychat --purge
```
