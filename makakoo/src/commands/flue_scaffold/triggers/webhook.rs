//! Webhook trigger renderer — Hono-based HTTP server.
//!
//! V1: per codex review, no `defineTrigger` in @flue/runtime.
//! A trigger webhook is just an HTTP endpoint that dispatches
//! into the agent. The module starts a Hono server on a
//! dedicated port (8809 by convention — MCP proxy is on 8808)
//! and is loaded as a side effect from assistant.ts.

pub fn render(path: &str, secret_env: &str) -> anyhow::Result<String> {
    let port = std::env::var("MAKAKOO_TRIGGER_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8809);
    Ok(format!(
        r##"/* AUTO-GENERATED. Do not edit by hand.
 *
 * NOTE: per codex review of Phase 4, no defineTrigger. This
 * module is a standalone Hono server that dispatches into
 * the agent. Loaded as a side effect from assistant.ts.
 */
import {{ Hono }} from 'hono';
import {{ createHmac, timingSafeEqual }} from 'node:crypto';
import {{ dispatch }} from '@flue/runtime';
import assistant from '../agents/assistant.ts';

function requiredEnv(name: string): string {{
  const value = process.env[name];
  if (!value) throw new Error(`${{name}} is required. Add it to .env from .env.example.`);
  return value;
}}

const SECRET = requiredEnv('{secret_env}');
const PATH = '{path}';
const PORT = Number(process.env.MAKAKOO_TRIGGER_PORT ?? {port});

const app = new Hono();

app.post(PATH, async (c) => {{
  const rawBody = await c.req.text();
  const signature = c.req.header('x-signature');
  if (!signature) return c.text('missing signature', 401);
  const expected = createHmac('sha256', SECRET).update(rawBody).digest('hex');
  const a = Buffer.from(expected, 'hex');
  const b = Buffer.from(signature, 'hex');
  if (a.length !== b.length || !timingSafeEqual(a, b)) {{
    return c.text('signature mismatch', 401);
  }}
  let payload: unknown;
  try {{ payload = JSON.parse(rawBody); }} catch {{ payload = rawBody; }}

  const id = `webhook-${{c.req.header('x-request-id') ?? crypto.randomUUID()}}`;
  await dispatch(assistant, {{
    id,
    input: {{
      type: 'webhook.trigger',
      path: PATH,
      payload,
      headers: Object.fromEntries(c.req.raw.headers),
    }},
  }});
  return c.json({{ ok: true, id }});
}});

// @hono/node-server is already a transitive dep of @flue/runtime.
import('@hono/node-server').then(({{ serve }}) => {{
  serve({{ fetch: app.fetch, port: PORT }}, (info) => {{
    console.log(`trigger webhook listening on http://127.0.0.1:${{info.port}}${{PATH}}`);
  }});
}});
"##,
        path = path,
        secret_env = secret_env,
        port = port,
    ))
}
