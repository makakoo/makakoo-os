# Multi-transport subagents: current support boundary

This page used to be a runnable Telegram + Slack walkthrough for Makakoo's
legacy Python gateway. It is now a compatibility note so new users do not
deploy a configuration the default DeepSeek Harness runtime cannot serve.

## What works now

- AgentSpec accepts multiple channel declarations.
- Makakoo validates and preserves those declarations in slot metadata.
- DSH runs the scoped model/tool loop and accepts authenticated local prompts.
- Existing legacy gateway slots without a generated runtime keep their old
  lifecycle path.
- The explicit Flue compatibility renderer can generate its older channel
  modules, but it must be run manually.

## What the supervisor starts

`makakoo agent start` hosts the slot's **telegram** transports alongside the
runtime: allowlisted inbound messages reach `/v1/run` under a stable per-chat
session, and replies go back through `sendMessage`. See
[the spec reference](../agents/spec.md#channels-and-triggers) for the
allowlist and token rules.

Slack, Discord, WhatsApp, email, voice, webhook, and cron declarations are
still preserved without a listener or scheduler — `agent start` reports each
one it will not start, so a slot never looks connected when it is not.

## Safe path for a new agent

Create and test the local runtime first:

```sh
makakoo agent validate-spec ./agent.yaml
makakoo agent create --specs ./agent.yaml
cd "$MAKAKOO_HOME/agents-dsh/<slot>"
npm install
makakoo agent start <slot>
makakoo agent prompt <slot> "hello" --session smoke
```

Use the [DSH runtime walkthrough](./dsh-agent-runtime.md) for the full flow.

## Legacy Flue compatibility

If a Telegram or Slack listener is required today and the operator accepts the
legacy manual runtime:

```sh
MAKAKOO_AGENT_ENGINE=flue makakoo agent create --specs ./agent.yaml
cd "$MAKAKOO_HOME/agents-flue/<slot>"
npm install
npm run proxy
# separate terminal
npx flue dev
```

This is not supervised by `makakoo agent start`. Read
[`flue-telegram-bot.md`](./flue-telegram-bot.md) before using it.

## Adapter work still required

The intended channel adapter will authenticate inbound events, map each
conversation to a stable DSH session id, call the runtime's loopback `/v1/run`
endpoint, and return the reply on the originating channel. Until that slice is
implemented and tested, a "multi-transport DSH bot" claim is false.
