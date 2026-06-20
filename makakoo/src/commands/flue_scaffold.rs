//! `makakoo agent create --runtime flue` — scaffold a Flue (TypeScript) agent
//! project wired to Makakoo's MCP server + the `@flue/telegram` channel.
//!
//! Makakoo owns the control plane (identity, scope, secrets, registry — the
//! slot); Flue runs the data plane (the agent loop + the Telegram webhook). The
//! two are bridged by `mcp-proxy.mjs`, which exposes the local `makakoo-mcp`
//! stdio server over StreamableHTTP so the agent's `connectMcpServer()` can
//! consume every Makakoo tool (`mcp__harvey__*`). This bridge was proven
//! end-to-end — all of Makakoo's MCP tools were callable from Flue and a tool
//! executed against the live Brain.

use std::path::Path;

use makakoo_core::agents::AgentSlot;

/// Write a runnable Flue agent project into `out_dir`. Refuses to clobber a
/// non-empty directory.
pub fn scaffold_flue_project(slot: &AgentSlot, out_dir: &Path) -> anyhow::Result<()> {
    if out_dir.exists()
        && out_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        anyhow::bail!(
            "flue output dir {} already exists and is non-empty — refusing to overwrite",
            out_dir.display()
        );
    }

    let project = format!("{}-flue-agent", slot.slot_id);
    let instructions = slot.persona.clone().unwrap_or_else(default_instructions);

    write_file(out_dir, "package.json", &package_json(&project))?;
    write_file(out_dir, "mcp-proxy.mjs", MCP_PROXY)?;
    write_file(out_dir, "src/agents/assistant.ts", ASSISTANT_TS)?;
    write_file(out_dir, "src/channels/telegram.ts", TELEGRAM_TS)?;
    write_file(out_dir, "instructions.txt", &instructions)?;
    write_file(out_dir, ".env.example", ENV_EXAMPLE)?;
    write_file(out_dir, ".gitignore", GITIGNORE)?;
    write_file(out_dir, "README.md", &readme(&slot.slot_id, &project))?;
    Ok(())
}

fn write_file(root: &Path, rel: &str, content: &str) -> anyhow::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(())
}

fn default_instructions() -> String {
    "You are a helpful assistant reachable over Telegram. Reply concisely. You \
have Makakoo's tools available as `mcp__harvey__*` — use them to search the \
Brain, run skills, and act on the user's behalf."
        .to_string()
}

fn package_json(project: &str) -> String {
    format!(
        r##"{{
  "name": "{project}",
  "private": true,
  "type": "module",
  "scripts": {{
    "proxy": "node mcp-proxy.mjs",
    "dev": "flue dev",
    "build": "flue build --target node"
  }},
  "dependencies": {{
    "@flue/runtime": "^1.0.0-beta.2",
    "@flue/telegram": "^1.0.0-beta.1",
    "@modelcontextprotocol/sdk": "^1.29.0",
    "grammy": "^1.0.0"
  }},
  "devDependencies": {{
    "@flue/cli": "^1.0.0-beta.1"
  }}
}}
"##
    )
}

fn readme(slot: &str, project: &str) -> String {
    format!(
        r##"# {project}

A [Flue](https://flueframework.com) agent scaffolded by
`makakoo agent create {slot} --runtime flue`.

Makakoo owns identity, scope, secrets and the registry (the `{slot}` slot); this
project is the runnable channel agent. It reaches Makakoo's Brain, skills and
tools over **MCP** through a local stdio→HTTP proxy.

## Run

```sh
npm install
cp .env.example .env          # then fill in the values:
#   TELEGRAM_BOT_TOKEN            — your bot token (@BotFather, or `makakoo secret`)
#   TELEGRAM_WEBHOOK_SECRET_TOKEN — any random string; use the same on setWebhook
npm run proxy                 # terminal 1: makakoo-mcp over http://127.0.0.1:8808/mcp
npx flue dev                  # terminal 2: runs the agent + webhook locally
```

Point Telegram's webhook at `POST /channels/telegram/webhook` (use a tunnel such
as cloudflared/ngrok in dev, or deploy to a Flue target). The agent replies in
the bound conversation and can call any `mcp__harvey__*` Makakoo tool.

## Files

- `src/agents/assistant.ts` — the agent: model + instructions + Makakoo MCP tools.
- `src/channels/telegram.ts` — verified Telegram webhook → dispatch to the agent.
- `mcp-proxy.mjs` — stdio→StreamableHTTP bridge to the local `makakoo-mcp` binary.
- `instructions.txt` — the agent's system instructions (from the slot persona).
"##
    )
}

const MCP_PROXY: &str = r##"// stdio -> StreamableHTTP MCP proxy.
//
// Flue's MCP client speaks StreamableHTTP; Makakoo's `makakoo-mcp` speaks stdio.
// This bridges them: it spawns `makakoo-mcp` once and re-exposes its tools over
// http://127.0.0.1:8808/mcp so the agent's connectMcpServer() can consume them.
//
//   node mcp-proxy.mjs
//
// Env: MAKAKOO_MCP_BIN (default "makakoo-mcp"), MAKAKOO_MCP_PORT (default 8808).
import http from 'node:http';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { ListToolsRequestSchema, CallToolRequestSchema } from '@modelcontextprotocol/sdk/types.js';

const MAKAKOO_MCP = process.env.MAKAKOO_MCP_BIN ?? 'makakoo-mcp';
const PORT = Number(process.env.MAKAKOO_MCP_PORT ?? 8808);

const upstream = new Client({ name: 'makakoo-proxy-upstream', version: '0.0.0' });
await upstream.connect(new StdioClientTransport({ command: MAKAKOO_MCP, args: [], env: { ...process.env } }));

function buildServer() {
  const s = new Server({ name: 'makakoo-proxy', version: '0.0.0' }, { capabilities: { tools: {} } });
  s.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: (await upstream.listTools()).tools }));
  s.setRequestHandler(CallToolRequestSchema, async (req) => await upstream.callTool(req.params));
  return s;
}

http
  .createServer(async (req, res) => {
    if (!req.url.startsWith('/mcp')) { res.writeHead(404).end(); return; }
    let body;
    if (req.method === 'POST') {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString('utf8')) : undefined;
    }
    const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined });
    res.on('close', () => transport.close());
    const server = buildServer();
    await server.connect(transport);
    await transport.handleRequest(req, res, body);
  })
  .listen(PORT, '127.0.0.1', () => console.log(`makakoo MCP proxy on http://127.0.0.1:${PORT}/mcp`));
"##;

const ASSISTANT_TS: &str = r##"import { readFileSync } from 'node:fs';
import { createAgent, connectMcpServer } from '@flue/runtime';
import { channel, postMessage } from '../channels/telegram.ts';

// Makakoo's MCP server, exposed over HTTP by `npm run proxy` (mcp-proxy.mjs).
// connectMcpServer adapts every Makakoo tool into a Flue tool (mcp__harvey__*).
const mcpUrl = process.env.MAKAKOO_MCP_URL ?? 'http://127.0.0.1:8808/mcp';
const makakoo = await connectMcpServer('harvey', { url: mcpUrl, transport: 'streamable-http' });

const instructions = readFileSync(new URL('../../instructions.txt', import.meta.url), 'utf8');

export default createAgent(({ id }) => ({
  model: process.env.AGENT_MODEL ?? 'anthropic/claude-sonnet-4-6',
  instructions,
  tools: [...makakoo.tools, postMessage(channel.parseConversationKey(id))],
}));
"##;

const TELEGRAM_TS: &str = r##"import { defineTool, dispatch } from '@flue/runtime';
import { createTelegramChannel, type TelegramConversationRef } from '@flue/telegram';
import { Api } from 'grammy';
import type { Message } from 'grammy/types';
import assistant from '../agents/assistant.ts';

export const client = new Api(requiredEnv('TELEGRAM_BOT_TOKEN'));

export const channel = createTelegramChannel({
  secretToken: requiredEnv('TELEGRAM_WEBHOOK_SECRET_TOKEN'),

  // Path: /channels/telegram/webhook
  async webhook({ update }) {
    const incoming = update.message ?? update.channel_post ?? update.business_message;
    if (!incoming) return;
    await dispatch(assistant, {
      id: channel.conversationKey(conversationFromMessage(incoming)),
      input: { type: 'telegram.message', updateId: update.update_id, message: incoming },
    });
  },
});

function conversationFromMessage(message: Message): TelegramConversationRef {
  return message.business_connection_id
    ? {
        type: 'business-chat',
        businessConnectionId: message.business_connection_id,
        chatId: message.chat.id,
      }
    : { type: 'chat', chatId: message.chat.id };
}

export function postMessage(ref: TelegramConversationRef) {
  return defineTool({
    name: 'post_telegram_message',
    description: 'Post a message to the Telegram conversation bound to this agent.',
    parameters: {
      type: 'object',
      properties: { text: { type: 'string', minLength: 1 } },
      required: ['text'],
      additionalProperties: false,
    },
    async execute({ text }) {
      const message = await client.sendMessage(ref.chatId, text, {
        ...(ref.type === 'business-chat'
          ? { business_connection_id: ref.businessConnectionId }
          : {}),
      });
      return JSON.stringify({ messageId: message.message_id });
    },
  });
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required.`);
  return value;
}
"##;

const ENV_EXAMPLE: &str =
    "# Telegram bot token (from @BotFather, or `makakoo secret get <name>`)\n\
TELEGRAM_BOT_TOKEN=123456:replace-with-your-bot-token\n\
# Any random string; set the SAME value when you call setWebhook\n\
TELEGRAM_WEBHOOK_SECRET_TOKEN=replace_with_a_random_secret\n\
# Makakoo MCP endpoint exposed by mcp-proxy.mjs (npm run proxy)\n\
MAKAKOO_MCP_URL=http://127.0.0.1:8808/mcp\n\
# Optional model override\n\
AGENT_MODEL=anthropic/claude-sonnet-4-6\n";

const GITIGNORE: &str = "node_modules/\n.env\ndist/\n.flue/\n";
