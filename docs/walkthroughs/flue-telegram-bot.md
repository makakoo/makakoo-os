# Legacy compatibility: build a Telegram bot with Flue

**Status:** operator-only compatibility path. New Makakoo agents use the
[supervised DeepSeek Harness runtime](./dsh-agent-runtime.md). Flue remains
useful when you specifically need the older generated Telegram listener, but
Makakoo does not supervise Flue projects with `makakoo agent start`.

**Time:** about 15 minutes. **Prerequisites:** Makakoo OS, Node.js 18 or newer,
a Telegram bot token, and a switchAILocal or supported legacy Flue provider.

## 1. Write an AgentSpec

```yaml
# assistant.yaml
name: assistant
description: Telegram assistant on the legacy Flue compatibility runtime
model: switchailocal/ail-compound
instructions: |
  You are a concise Telegram assistant. Use only the allowed Makakoo tools.
tools:
  - brain_search
channels:
  - kind: telegram
    token_env: TELEGRAM_BOT_TOKEN
    allowed_users: ["YOUR_TELEGRAM_CHAT_ID"]
triggers: []
scope:
  allowed_paths:
    - "~/MAKAKOO/data/Brain/**"
  forbidden_paths:
    - "~/.ssh/**"
```

Validate the canonical spec first:

```sh
makakoo agent validate-spec ./assistant.yaml
```

## 2. Select Flue explicitly

```sh
MAKAKOO_AGENT_ENGINE=flue \
  makakoo agent create --specs ./assistant.yaml
```

Without that environment variable, Makakoo generates the default DSH runtime.
There is no runtime-selection CLI flag.

The Flue project is written under
`$MAKAKOO_HOME/agents-flue/assistant/`. Makakoo also records the slot policy
under `$MAKAKOO_HOME/config/agents/assistant.toml`.

## 3. Configure and run manually

```sh
cd "$MAKAKOO_HOME/agents-flue/assistant"
cp .env.example .env
```

Set `TELEGRAM_BOT_TOKEN` and `TELEGRAM_WEBHOOK_SECRET_TOKEN` in `.env`. Keep
that file out of git.

Then use two terminals:

```sh
# terminal 1
npm install
npm run proxy

# terminal 2
npx flue dev
```

The generated MCP proxy starts `makakoo-mcp` and exposes its scoped tools to
the Flue agent. The generated channel module verifies Telegram's webhook
secret before dispatch.

## 4. Register the Telegram webhook

Telegram needs a public HTTPS URL. Put a trusted tunnel in front of the local
Flue server, then register the generated webhook path:

```sh
curl -sS "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/setWebhook" \
  -d "url=https://<your-host>/channels/telegram/webhook" \
  -d "secret_token=$TELEGRAM_WEBHOOK_SECRET_TOKEN"
```

## Lifecycle caveat

Do not run `makakoo agent start assistant`. The supervisor rejects Flue slots
and tells you to run the generated proxy/dev scripts. Stop Flue by stopping
those processes.

To remove the slot and archive its managed files:

```sh
makakoo agent destroy assistant
```

## Troubleshooting

| Symptom | Fix |
|---|---|
| A DSH project was generated | Recreate after setting `MAKAKOO_AGENT_ENGINE=flue`. |
| `slot ... uses the legacy Flue engine` | Expected. Run `npm run proxy` and `npx flue dev` manually. |
| No Makakoo tools | Start `npm run proxy`; verify `MAKAKOO_MCP_BIN` and `MAKAKOO_MCP_URL`. |
| Telegram sends nothing | Verify the public HTTPS URL and webhook secret match `.env`. |

Full agent reference: [`../user-manual/agent.md`](../user-manual/agent.md).
