//! Slack channel renderer — uses `@flue/slack`.
//!
//! V1: per codex review, `@flue/slack` takes `signingSecret` (not
//! `botToken`/`appToken`/`teamId`), and the handler names are
//! `events` / `interactions` / `commands` (not `message`). There
//! is no first-party outbound API in `@flue/slack` — operators
//! wire their own `defineTool` for posting back, or use the
//! Slack Web API directly.

pub fn render(
    _token_env: &str,
    _app_token_env: &str,
    _team_id_env: &str,
    _allowed_users: &[String],
) -> anyhow::Result<String> {
    // The Flue slack channel doesn't take tokens (it only signs
    // inbound requests). The operator's bot token (used to POST
    // messages back) is read directly by their outbound tool.
    Ok(r##"/* AUTO-GENERATED. Do not edit by hand.
 *
 * NOTE: per codex review of Phase 4, @flue/slack does NOT take
 * botToken / appToken / teamId. The channel only verifies
 * inbound request signatures. To post messages back, the agent
 * must use a separate defineTool that calls the Slack Web API
 * with an env-var-held bot token. See the slack-postback tool
 * below for a starting point.
 */
import { defineTool, dispatch } from '@flue/runtime';
import {
  createSlackChannel,
  type SlackThreadRef,
} from '@flue/slack';
import * as v from 'valibot';
import assistant from '../agents/assistant.ts';

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required. Add it to .env from .env.example.`);
  return value;
}

const SIGNING_SECRET = requiredEnv('SLACK_SIGNING_SECRET');
const BOT_TOKEN = requiredEnv('SLACK_BOT_TOKEN');

export const channel = createSlackChannel({
  signingSecret: SIGNING_SECRET,

  // Slack Events API payloads (message, app_mention, etc.).
  async events({ event }) {
    await dispatch(assistant, {
      id: channel.conversationKey({ teamId: 'tbd', channelId: event.channel, threadTs: '' }),
      input: { type: 'slack.event', event },
    });
  },

  // Interactivity payloads (button clicks, modal submissions).
  async interactions({ payload }) {
    await dispatch(assistant, {
      id: channel.conversationKey({ teamId: 'tbd', channelId: payload.channel?.id ?? '', threadTs: '' }),
      input: { type: 'slack.interaction', payload },
    });
  },

  // Slash-command payloads.
  async commands({ command }) {
    await dispatch(assistant, {
      id: channel.conversationKey({ teamId: 'tbd', channelId: command.channel_id, threadTs: '' }),
      input: { type: 'slack.command', command },
    });
  },
});

/**
 * Operator-supplied outbound tool. The Flue channel itself does
 * not expose a posting API, so this tool calls the Slack Web API
 * directly with an env-var-held bot token. The Valibot schema is
 * the source of truth for what the LLM can call this tool with.
 */
export function tool(id: string) {
  return defineTool({
    name: 'post_slack_message',
    description: 'Post a message to a Slack channel via the Web API.',
    input: v.object({
      channel: v.string(),
      text: v.pipe(v.string(), v.minLength(1)),
      thread_ts: v.optional(v.string()),
    }),
    async run(ctx) {
      const { channel, text, thread_ts } = ctx.input;
      const body: Record<string, unknown> = { channel, text };
      if (thread_ts) body.thread_ts = thread_ts;
      const res = await fetch('https://slack.com/api/chat.postMessage', {
        method: 'POST',
        headers: {
          'Authorization': `Bearer ${BOT_TOKEN}`,
          'Content-Type': 'application/json; charset=utf-8',
        },
        body: JSON.stringify(body),
      });
      const out = await res.json() as { ok: boolean; ts?: string; error?: string };
      if (!out.ok) throw new Error(`slack postMessage failed: ${out.error ?? 'unknown'}`);
      return { ts: out.ts };
    },
  });
}
"##.to_string())
}
