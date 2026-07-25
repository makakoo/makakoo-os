//! Discord channel renderer — uses `@flue/discord`.
//!
//! V1: per codex review, `@flue/discord` takes `publicKey` (not
//! a discord.js `Client`), and the handler is `interactions`
//! (not `messageCreate`). Discord interactions are slash
//! commands / component clicks / modals — NOT free-form
//! messages. Outbound responses are returned synchronously
//! from the handler.

pub fn render(_token_env: &str, _allowed_users: &[String]) -> anyhow::Result<String> {
    Ok(r##"/* AUTO-GENERATED. Do not edit by hand.
 *
 * NOTE: per codex review of Phase 4, @flue/discord does NOT
 * take a discord.js Client. The channel only verifies
 * inbound interactions using a public key. Discord bots
 * built on @flue/discord receive Interactions (slash
 * commands, components, modals), NOT message events.
 */
import { dispatch } from '@flue/runtime';
import {
  createDiscordChannel,
  type DiscordDestinationRef,
} from '@flue/discord';
import assistant from '../agents/assistant.ts';

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required. Add it to .env from .env.example.`);
  return value;
}

const PUBLIC_KEY = requiredEnv('DISCORD_PUBLIC_KEY');

export const channel = createDiscordChannel({
  publicKey: PUBLIC_KEY,

  // Discord interactions (slash commands, components, modals).
  async interactions({ payload }) {
    const conv: DiscordDestinationRef = {
      type: 'guild',
      guildId: payload.guild_id ?? 'dm',
      channelId: payload.channel_id ?? 'dm',
    };
    await dispatch(assistant, {
      id: channel.conversationKey(conv),
      input: { type: 'discord.interaction', payload },
    });
    // Return a PONG-style response (operators customize this).
    return { type: 1 };
  },
});
"##.to_string())
}
