# Reference — OpenClaw multi-channel pattern

Sebastian (2026-04-26): *"We should be able to do it using telegram or
slack or discord etc.. we should be able to connect different chats apps
to the subagents... check how OpenClaw is managing this."*

OpenClaw (third-party, located at
`/Users/sebastian/projects/makakoo/agents/sample_apps/openclaw`) is the
reference for how ONE agent connects to MULTIPLE chat platforms
simultaneously. This sprint should adopt — not reinvent — its pattern.

## TL;DR pattern

OpenClaw's plugin SDK exposes a uniform `ChannelPlugin<ResolvedAccount>`
interface that every transport (Telegram, Slack, Discord, Mattermost,
Teams, WhatsApp, Matrix, …) implements. A single Node.js process spawns
N concurrent `gateway.startAccount()` listeners — one per attached
transport — and routes inbound messages to a common LLM dispatcher
tagged with `{channel, account, thread, target}` metadata. Outbound
replies hit `outbound.sendPayload()` for the matching transport.

## Where the abstractions live

```
openclaw/
├── src/
│   ├── plugin-sdk/
│   │   ├── channel-entry-contract.ts     # entry-point loader
│   │   ├── gateway-runtime.ts            # gateway helpers
│   │   └── routing.ts                    # (channel, account, thread) → agent
│   ├── channels/plugins/
│   │   ├── types.plugin.ts               # ChannelPlugin<ResolvedAccount>
│   │   ├── types.adapters.ts             # Gateway/Outbound/Messaging/...
│   │   └── outbound.types.ts             # ChannelOutboundContext
│   └── secrets/
│       └── channel-contract-api.ts       # env > .env > config precedence
└── extensions/
    ├── telegram/src/channel.ts
    ├── discord/src/channel.ts
    ├── slack/src/channel.ts
    ├── mattermost/src/channel.ts
    ├── teams/src/channel.ts
    ├── whatsapp/src/channel.ts
    └── matrix/src/channel.ts
```

## The interface (paraphrased)

```typescript
type ChannelPlugin<ResolvedAccount = any> = {
  id: ChannelId;
  meta: ChannelMeta;
  capabilities: ChannelCapabilities;
  config:    ChannelConfigAdapter<ResolvedAccount>;     // load creds
  outbound?: ChannelOutboundAdapter;                     // send a reply
  gateway?:  ChannelGatewayAdapter<ResolvedAccount>;     // receive messages
  messaging?: ChannelMessagingAdapter;                   // format/route
  pairing?:   ChannelPairingAdapter;                     // user allowlist
  security?:  ChannelSecurityAdapter<ResolvedAccount>;
  status?:    ChannelStatusAdapter<ResolvedAccount>;
  // …~15 more optional adapters (approval, groups, commands, etc.)
};
```

Key methods every transport must implement:

- `gateway.startAccount(ctx)` — spawn webhook/poller for this account.
- `gateway.stopAccount(ctx)` — tear it down.
- `outbound.sendPayload(ctx, payload)` — deliver a formatted reply.
- `messaging.resolveTarget(req)` — normalise "send to X" requests.
- `config.resolveAccount(raw)` — decrypt/load creds.

## How metadata propagates

Every inbound message carries the channel context all the way to the
LLM, and every outbound reply receives the same context back so it
knows where to send:

```typescript
type ChannelOutboundContext = {
  cfg: OpenClawConfig;
  to: string;                     // user/chat/channel id
  accountId?: string | null;      // which Telegram/Discord/Slack account
  threadId?: string | number | null; // thread/topic
  identity?: OutboundIdentity;    // sender override
};
```

The LLM thus always knows: "this came from Slack channel #general via
account 'work-slack', thread 12345" — vs "this came from Telegram DM
with user 746496145".

## Secrets layering

Precedence (highest → lowest):

1. Process env (`TELEGRAM_BOT_TOKEN`, `DISCORD_BOT_TOKEN`, …).
2. Repo `.env`.
3. `~/.openclaw/.env`.
4. `openclaw.json` config block (last resort).

Per-transport secret descriptors are declared in
`ChannelSecretsAdapter.secretTargetRegistryEntries`, so `openclaw
doctor` can validate each transport's creds independently.

## What this means for Makakoo subagents

The multi-bot-subagents sprint should:

1. **Adopt the `ChannelPlugin` shape** as the contract for any
   chat-app attachment. Don't reinvent it; consider whether to fork
   the OpenClaw SDK or rebuild a Rust equivalent.
2. **Schema**: per-agent config has a list of `[[transport]]` blocks,
   each with `kind = "telegram" | "slack" | "discord" | ...` and
   transport-specific fields, mirroring OpenClaw's plugin descriptors.
3. **Process model**: one subagent process can multiplex N transports
   internally (OpenClaw model), OR one process per transport with a
   shared LLM session — Phase 0 must lock this.
4. **Routing layer**: needs `(transport, account, thread) → agent`
   resolution as a first-class concern, not a Telegram-only assumption.
5. **Secrets via `makakoo secret`** keyring (already exists), with the
   same precedence ladder OpenClaw uses but rooted in Makakoo's
   keyring rather than dotfiles.
6. **Don't re-attribute OpenClaw to Sebastian** in any external docs —
   it's third-party (per `openclaw_attribution` memory).

## Concrete acceptance addition

Original VISION.md acceptance criterion:

> *"3 subagents simultaneously running on Sebastian's machine, each
> responding only on its own bot."*

Updated for multi-transport:

> *"At least one subagent is reachable via TWO different chat
> platforms simultaneously (e.g. @SecretaryBot on Telegram AND in
> Slack channel #secretary), with metadata propagation proving the
> agent knows which transport delivered each message."*
