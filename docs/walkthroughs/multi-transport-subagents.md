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

## What DSH V1 does not do

DSH V1 does not start Telegram, Slack, Discord, WhatsApp, email, voice,
webhook, or cron listeners. A spec with those declarations emits a warning at
creation. `makakoo agent start` supervises the DSH runtime, not a channel
gateway.

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
