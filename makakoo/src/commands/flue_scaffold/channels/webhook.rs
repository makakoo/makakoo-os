//! Webhook channel renderer — Hono-based HTTP server. Inbound only.
//! Spec's `path` becomes the POST endpoint, `secret_env` is the HMAC
//! secret for signature verification (X-Signature header).

pub fn render(path: &str, secret_env: &str) -> anyhow::Result<String> {
    Ok(format!(
        r##"/* AUTO-GENERATED. Do not edit by hand. */
import {{ defineTool, dispatch }} from '@flue/runtime';
import {{ createWebhookChannel, type WebhookConversationRef }} from '@flue/runtime';
import {{ Hono }} from 'hono';
import {{ createHmac, timingSafeEqual }} from 'node:crypto';
import assistant from '../../agents/assistant.ts';

function requiredEnv(name: string): string {{
  const value = process.env[name];
  if (!value) throw new Error(`${{name}} is required. Add it to .env from .env.example.`);
  return value;
}}

const SECRET = requiredEnv('{secret_env}');
const PATH = '{path}';

export const channel = createWebhookChannel({{
  path: PATH,
  verifySignature(rawBody: string, signature: string | null): boolean {{
    if (!signature) return false;
    const expected = createHmac('sha256', SECRET).update(rawBody).digest('hex');
    const a = Buffer.from(expected, 'hex');
    const b = Buffer.from(signature, 'hex');
    return a.length === b.length && timingSafeEqual(a, b);
  }},

  async handler({{ payload, headers }}) {{
    const conv: WebhookConversationRef = {{
      type: 'webhook',
      endpointId: PATH,
      requestId: (headers['x-request-id'] as string) ?? crypto.randomUUID(),
    }};
    await dispatch(assistant, {{
      id: channel.conversationKey(conv),
      input: {{ type: 'webhook.payload', payload, headers }},
    }});
  }},
}});

export function tool(id: string) {{
  return defineTool({{
    name: 'respond_webhook',
    description: 'Return a JSON response to the inbound webhook that triggered this agent turn.',
    parameters: {{
      type: 'object',
      properties: {{
        status: {{ type: 'integer', minimum: 100, maximum: 599, default: 200 }},
        body: {{ type: 'object', additionalProperties: true }},
      }},
      required: ['status', 'body'],
      additionalProperties: false,
    }},
    async execute({{ status, body }}) {{
      // The webhook response is recorded for the bound request and
      // returned by the channel when the response channel is read.
      channel.recordResponse(id, {{ status, body }});
      return JSON.stringify({{ status, body }});
    }},
  }});
}}
"##,
        path = path,
        secret_env = secret_env,
    ))
}
