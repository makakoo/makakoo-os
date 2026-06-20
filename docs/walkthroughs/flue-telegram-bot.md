# Build a Telegram bot with Flue (`agent create --runtime flue`)

**Time:** ~15 min · **Prereqs:** Walkthrough [01](./01-fresh-install-mac.md)
(a working `makakoo` + `makakoo-mcp` on `PATH`), Node 18+, and a Telegram
bot token from [@BotFather](https://t.me/BotFather).

This builds a **runnable** Telegram agent that answers messages using your
Makakoo Brain, skills, and every `mcp__harvey__*` tool — scaffolded in one
command. Unlike a `native` slot (which runs inside Makakoo's own gateway),
a **Flue** agent is a standalone TypeScript project you run and deploy
yourself, while Makakoo still owns identity, scope, and secrets.

## How it fits together

```
   Telegram  ──webhook──▶  Flue agent (TypeScript)  ──MCP──▶  makakoo-mcp
   (your bot)              src/agents/assistant.ts            (Brain, skills,
                           src/channels/telegram.ts            tools = mcp__harvey__*)
                                    │
                                    ▼
                            mcp-proxy.mjs  (stdio → StreamableHTTP, :8808)
```

- **Makakoo = control plane:** the `assistant` slot holds identity, scope,
  allowed paths/tools, and secret references.
- **Flue = data plane:** the agent loop + the verified Telegram webhook.
- **`mcp-proxy.mjs` = the bridge:** spawns `makakoo-mcp` once and re-exposes
  its stdio tools over HTTP so Flue's `connectMcpServer()` can call them.

## 1. Scaffold the agent

```sh
makakoo agent create assistant \
  --runtime flue \
  --persona "You are a helpful Telegram assistant. Reply concisely. Use Makakoo's Brain and tools." \
  --out ~/MAKAKOO/agents-flue/assistant
```

`--out` is optional (default `$MAKAKOO_HOME/agents-flue/<slot>`). The command
refuses to overwrite a non-empty directory. It writes a native slot config
**and** a runnable project:

```
~/MAKAKOO/agents-flue/assistant/
├── package.json            # @flue/runtime + @flue/telegram + MCP SDK + grammy
├── mcp-proxy.mjs           # stdio → StreamableHTTP bridge to makakoo-mcp
├── instructions.txt        # the agent's system prompt (from --persona)
├── src/agents/assistant.ts # model + instructions + Makakoo MCP tools
├── src/channels/telegram.ts# signature-verified webhook → dispatch to agent
├── .env.example
├── .gitignore
└── README.md
```

## 2. Configure secrets

```sh
cd ~/MAKAKOO/agents-flue/assistant
cp .env.example .env
```

Edit `.env`:

| Var | Value |
|---|---|
| `TELEGRAM_BOT_TOKEN` | The token from @BotFather (or pull from `makakoo secret get <name>`). |
| `TELEGRAM_WEBHOOK_SECRET_TOKEN` | Any random string — you'll pass the **same** value to `setWebhook`. |
| `MAKAKOO_MCP_URL` | Leave as `http://127.0.0.1:8808/mcp` (the proxy default). |
| `AGENT_MODEL` | Optional model override (default `anthropic/claude-sonnet-4-6`). |

> Keep tokens out of git — the generated `.gitignore` already excludes `.env`.

## 3. Run it (two terminals)

```sh
npm install

# terminal 1 — expose Makakoo's MCP tools over HTTP
npm run proxy        # → makakoo MCP proxy on http://127.0.0.1:8808/mcp

# terminal 2 — run the agent + the local webhook server
npx flue dev
```

`npm run proxy` starts `mcp-proxy.mjs`. It spawns `makakoo-mcp` (override
with `MAKAKOO_MCP_BIN`) and listens on port `8808` (override with
`MAKAKOO_MCP_PORT`). On boot the agent calls `connectMcpServer('harvey', …)`
and adapts every Makakoo tool into a Flue tool — so the bot can search the
Brain, run skills, and act, all gated by the slot's scope.

## 4. Point Telegram at the webhook

Telegram only delivers updates to a **public HTTPS** URL, so in development
put a tunnel in front of the Flue dev server (the URL it prints on start):

```sh
# example with cloudflared — any HTTPS tunnel works (ngrok, etc.)
cloudflared tunnel --url http://localhost:<flue-dev-port>
```

Register the webhook, reusing the **exact** secret from your `.env`:

```sh
curl -sS "https://api.telegram.org/bot$TELEGRAM_BOT_TOKEN/setWebhook" \
  -d "url=https://<your-tunnel-host>/channels/telegram/webhook" \
  -d "secret_token=$TELEGRAM_WEBHOOK_SECRET_TOKEN"
```

The agent rejects any update whose `X-Telegram-Bot-Api-Secret-Token` header
doesn't match — so a leaked URL alone can't drive your bot.

## 5. Talk to it

Message your bot on Telegram. It dispatches each message to the agent, which
replies in the same conversation via the generated `post_telegram_message`
tool. Ask it something only your Brain knows — e.g. *"what did I journal about
the OpenAI Codex application?"* — to confirm the MCP bridge is live.

## 6. Deploy (optional)

`flue dev` is for local iteration. For a long-running bot, build and host the
project on any Flue target:

```sh
npm run build        # flue build --target node
```

The `mcp-proxy.mjs` + `makakoo-mcp` pair must run wherever the agent runs (the
agent needs a reachable `MAKAKOO_MCP_URL`).

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `refusing to overwrite` on create | `--out` dir is non-empty | Pick an empty dir, or delete the old scaffold. |
| Agent starts but has no `mcp__harvey__*` tools | Proxy not running / wrong `MAKAKOO_MCP_URL` | Start `npm run proxy` first; confirm port `8808` matches `.env`. |
| `makakoo-mcp` not found in proxy | Binary not on `PATH` | Set `MAKAKOO_MCP_BIN=/usr/local/bin/makakoo-mcp` before `npm run proxy`. |
| Telegram delivers nothing | Webhook URL/secret mismatch | Re-run `setWebhook`; the URL must be public HTTPS and end `/channels/telegram/webhook`. |
| `TELEGRAM_BOT_TOKEN is required` | `.env` not loaded / empty | Copy `.env.example` → `.env` and fill both Telegram values. |

## See also

- [`user-manual/agent.md`](../user-manual/agent.md) — the full `agent` CLI
  reference, including `native` vs `flue` runtime.
- [`multi-transport-subagents.md`](./multi-transport-subagents.md) — native
  multi-transport slots supervised by Makakoo's own gateway.
- [Flue framework](https://flueframework.com) — the TypeScript agent harness
  this scaffold targets.
