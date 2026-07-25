//! mcp-proxy.mjs — stdio → StreamableHTTP bridge to the local
//! `makakoo-mcp` binary. Unchanged from the original Phase 1/2/3
//! scaffold; kept verbatim so existing Flue projects keep working.

pub const MCP_PROXY: &str = r##"// stdio -> StreamableHTTP MCP proxy.
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
