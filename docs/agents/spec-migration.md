# Migrate a legacy slot TOML to a DSH AgentSpec

Legacy slot TOML files under `$MAKAKOO_HOME/config/agents/` are runtime policy
records, not accepted `agent create` input. New agents start from YAML or TOML
**AgentSpec** files and compile to a supervised DeepSeek Harness (DSH) project.
There is no automatic slot-TOML importer.

The safest migration is parallel: create a new DSH slot with a temporary id,
verify it, then archive the legacy slot.

## 1. Map the policy fields

| legacy slot TOML | AgentSpec |
|---|---|
| `slot_id` | `name` |
| `name` | `description` |
| `persona` | `instructions` |
| `tools` | `tools` |
| `allowed_paths` | `scope.allowed_paths` |
| `forbidden_paths` | `scope.forbidden_paths` |
| `[llm.override].model` | `model` |
| `[[transport]]` | `channels[]` declaration only; see boundary below |

Use a new id while both slots exist:

```yaml
name: harveychat-dsh
description: Local DSH replacement for the legacy HarveyChat slot
model: switchailocal/ail-compound
instructions: |
  Sharp professional secretary. Draft external messages and wait for approval.
tools:
  - brain_search
channels: []
triggers: []
scope:
  allowed_paths:
    - "~/MAKAKOO/data/harveychat/**"
  forbidden_paths:
    - "~/.ssh/**"
    - "~/.aws/**"
```

DSH model routes are switchAILocal-only. Replace legacy provider prefixes such
as `anthropic/...` with `switchailocal/<model>` or an unprefixed model id served
by switchAILocal.

## 2. Validate before writing state

```sh
makakoo agent validate-spec ./harveychat-dsh.yaml
makakoo agent show harveychat       # inspect the old policy for comparison
```

Validation does not create files, install packages, start processes, or call a
channel.

## 3. Create and install

```sh
makakoo agent create --specs ./harveychat-dsh.yaml
cd "$MAKAKOO_HOME/agents-dsh/harveychat-dsh"
npm install
npm run check
makakoo agent validate harveychat-dsh
```

The generated project contains `runner.mjs`, `cordis.yml`, `spec.yaml`, and an
`.env.example`. Runtime state lives under
`$MAKAKOO_HOME/agents-dsh/harveychat-dsh/`, not `agents-flue/`.

## 4. Verify the local runtime

```sh
makakoo agent start harveychat-dsh
makakoo agent health harveychat-dsh
makakoo agent prompt harveychat-dsh "State your role and allowed tools" --session migration
makakoo agent stop harveychat-dsh
```

Confirm model behavior, tool scope, filesystem scope, restart, and stop before
retiring anything.

## Channel and trigger boundary

DSH V1 preserves channel and trigger declarations in the spec and slot
metadata, but does **not** start Telegram, Slack, Discord, WhatsApp, email,
voice, webhook, or cron listeners. Do not migrate a working channel bot and
assume message delivery survived. Keep the legacy runtime until a Makakoo
channel adapter is available, or explicitly use the manually operated Flue
compatibility path documented in
[`../walkthroughs/flue-telegram-bot.md`](../walkthroughs/flue-telegram-bot.md).

## 5. Archive the old slot only after verification

```sh
makakoo agent destroy harveychat
```

`destroy` stops the service and archives managed configuration, data, and
runtime files. It does not revoke secrets unless `--revoke-secrets` is
explicit. Keep the new temporary id unless preserving the old id is important;
renaming requires a fresh spec and a second verified create/destroy cycle.

## Known mapping limits

- `description` is retained in generated `spec.yaml`; AgentSlot has no dedicated
  description field.
- `inherit_baseline` is not carried forward. AgentSpec tool lists are explicit;
  `tools: []` exposes no model-facing tools.
- Per-transport tuning has no lossless AgentSpec round trip.
- Cron and webhook triggers are not represented in slot TOML and are not
  scheduled by DSH V1.
- Filesystem overlap validation is exact-string in V1; retain explicit deny
  paths.

## See also

- [`spec.md`](spec.md) — AgentSpec schema
- [`../walkthroughs/dsh-agent-runtime.md`](../walkthroughs/dsh-agent-runtime.md) — end-to-end DSH walkthrough
- [`../user-manual/agent.md`](../user-manual/agent.md) — authoritative CLI reference
