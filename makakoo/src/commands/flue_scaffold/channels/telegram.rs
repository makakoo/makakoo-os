//! Telegram channel renderer — uses `@flue/telegram` + `grammy`.
//!
//! V1: per codex review of Phase 4, `@flue/telegram` has NO
//! `allowedUsers` option. Allowlist enforcement happens inside
//! the `webhook` handler before `dispatch()`. The actual
//! webhook input shape is `{ c, update }`, not just `{ update }`.
//! `defineTool` uses `input` (a JSON schema) + `run(ctx)`, NOT
//! `parameters` + `execute`.

pub fn render(token_env: &str, allowed_users: &[String]) -> anyhow::Result<String> {
    let allowed_users_ts = render_user_list(allowed_users);
    Ok(format!(
        r##"/* AUTO-GENERATED. Do not edit by hand. */
import {{ defineTool, dispatch }} from '@flue/runtime';
import {{ createTelegramChannel, type TelegramConversationRef }} from '@flue/telegram';
import * as v from 'valibot';
import {{ Api }} from 'grammy';
import type {{ Update }} from '@grammyjs/types';
import assistant from '../agents/assistant.ts';

function requiredEnv(name: string): string {{
  const value = process.env[name];
  if (!value) throw new Error(`${{name}} is required. Add it to .env from .env.example.`);
  return value;
}}

const TOKEN = requiredEnv('{token_env}');
const SECRET = requiredEnv('TELEGRAM_WEBHOOK_SECRET_TOKEN');
const ALLOWED_USERS: ReadonlyArray<string> = {allowed_users_ts};

export const channel = createTelegramChannel({{
  secretToken: SECRET,

  // Path: /channels/telegram/webhook
  async webhook({{ c, update }}) {{
    const senderId = telegramUserId(update);
    if (senderId === null) return c.text('no sender', 400);
    if (ALLOWED_USERS.length > 0 && !ALLOWED_USERS.includes(String(senderId))) {{
      return c.text('not allowed', 403);
    }}
    const conv: TelegramConversationRef = {{
      type: 'chat',
      chatId: update.message?.chat.id ?? update.channel_post?.chat.id ?? 0,
    }};
    await dispatch(assistant, {{
      id: channel.conversationKey(conv),
      input: {{ type: 'telegram.update', update }},
    }});
    return c.text('ok');
  }},
}});

function telegramUserId(update: Update): number | null {{
  // Numeric user ID only. We skip `channel_post?.author_signature`
  // because that's a display-name string, not a user ID.
  return (
    update.message?.from?.id ??
    update.edited_message?.from?.id ??
    update.callback_query?.from.id ??
    update.inline_query?.from.id ??
    null
  );
}}

export function tool(id: string) {{
  // Flue's `defineTool`: `input` is a Valibot schema; `run(ctx)`
  // gets typed `ctx.input`. The schema is the source of truth for
  // what the LLM can call this tool with.
  return defineTool({{
    name: 'post_telegram_message',
    description: 'Post a message back to the Telegram conversation bound to this agent turn.',
    input: v.object({{
      text: v.pipe(v.string(), v.minLength(1)),
    }}),
    async run(ctx) {{
      const {{ text }} = ctx.input;
      const client = new Api(TOKEN);
      const ref = channel.parseConversationKey(id) as TelegramConversationRef;
      const message = await client.sendMessage(ref.chatId, text);
      return {{ messageId: message.message_id }};
    }},
  }});
}}
"##,
        token_env = token_env,
        allowed_users_ts = allowed_users_ts,
    ))
}

fn render_user_list(users: &[String]) -> String {
    if users.is_empty() {
        "[]".to_string()
    } else {
        let parts: Vec<String> = users.iter().map(|u| format!("'{}'", u)).collect();
        format!("[{}]", parts.join(", "))
    }
}
