# Walkthrough — Discord bot subagent

End-to-end recipe: stand up a Discord-backed subagent slot under
Makakoo OS. ~10 minutes, no Docker, no hosted service.

## What you'll have at the end

- A Discord application with a bot user
- A Makakoo slot that receives DMs + guild messages and replies via the LLM
- Inbound auto-rejected from any guild not on the allowlist
- The slot supervised by `launchd` (macOS) or `systemd-user` (Linux)

## Prerequisites

- Makakoo OS installed (`curl -sSL get.makakoo.com | sh`)
- A Discord account with permission to create applications
- (Optional) A guild you control, for testing

## 1. Create the Discord application

1. Open <https://discord.com/developers/applications>.
2. Click **New Application**, name it something memorable
   (`Makakoo Secretary` works).
3. In the **Bot** tab, click **Reset Token** and copy the bot token —
   this is what Makakoo uses as the credential. **Don't paste it
   into source control.**
4. Under **Privileged Gateway Intents**, decide:
   - **MESSAGE_CONTENT** — leave OFF unless you genuinely need the
     bot to read every message in a guild. With it OFF, the bot sees
     DMs in full and only mentions/replies in guild channels (which
     is usually what you want).
   - **GUILD_MEMBERS** — leave OFF in v1. The current adapter
     returns `Unsupported` for `list_users`; that's by design.

## 2. Invite the bot to a guild

In the **OAuth2 → URL Generator** tab:

- Scopes: `bot`
- Bot Permissions: `Send Messages`, `Read Message History`,
  `Use Slash Commands` (optional)

Copy the generated URL and paste into a browser. Approve into your
test guild. Note the guild's numeric ID (right-click guild icon
with **Developer Mode** on → **Copy Server ID**).

## 3. Stash the token in Makakoo's secret store

```bash
makakoo secret set agent/secretary/discord-main/bot_token
# pastes the token interactively; never echoes
```

`agent/<slot>/<transport_id>/bot_token` is the convention; the
adapter uses `secret_ref` to resolve it at startup.

## 4. Create the slot

```bash
makakoo agent create secretary \
  --name "Secretary" \
  --persona "You are Sebastian's secretary. Be concise." \
  --allowed-paths "$MAKAKOO_HOME/data/secretary" \
  --tools "brain_search,brain_write_journal" \
  --skip-credential-check
```

This writes `~/MAKAKOO/config/agents/secretary.toml`. Open it and
add the Discord transport block (`makakoo agent create` does the
single-Telegram and single-Slack shapes natively today; Discord
goes through `--from-toml` or hand-editing the slot file):

```toml
[[transport]]
id = "discord-main"
kind = "discord"
secret_ref = "agent/secretary/discord-main/bot_token"
allowed_users = ["YOUR_DISCORD_USER_ID"]

[transport.config]
message_content = false       # leave OFF unless you truly need it
guild_ids       = [123456789012345678]    # your guild ID
support_thread  = true
```

Save the file.

## 5. Validate + start

```bash
makakoo agent validate secretary
# Verifies the token via GET /users/@me, dry-runs intent compute.

makakoo agent start secretary
# Hands the slot to launchd / systemd-user; supervisor + Python
# gateway come up.
```

Within ~5 seconds the bot should appear online in your guild.

## 6. Talk to it

- DM the bot from your account → expect a reply within a few seconds.
- In the guild, mention the bot (`@Secretary hi`) → expect a reply
  in-channel.
- Try a message from a guild **not** in `guild_ids` → expect silence
  (the adapter drops it before the LLM ever sees it).

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| "BadOrigin" / 401 in audit log | Token mismatch | Re-fetch via `secret set`, restart |
| Empty content in guild messages | MESSAGE_CONTENT intent off (expected) | Mention the bot, or DM it |
| Bot not online after `agent start` | Supervisor crashloop | `makakoo agent audit secretary --kind gateway_crash --last 20` |
| Rate limited | 60 msg/5min per sender | Wait or bump `[rate_limit] per_sender` in slot TOML |

## Stopping + destroying

```bash
makakoo agent stop secretary
makakoo agent destroy secretary --revoke-secrets
# Archives ~/MAKAKOO/archive/agents/secretary-<unix_ts>/, deletes
# the slot, revokes the keyring entry.
```

`--revoke-secrets` is opt-in. The default leaves keys in place so a
re-create can reuse them.
