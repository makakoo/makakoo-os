# Migrating from slot TOML to agent spec

If you have an existing slot TOML at
`$MAKAKOO_HOME/config/agents/<slot>.toml`, here's how to convert it
to a spec.

## Quick conversion (hand)

| slot TOML field | spec field |
|-----------------|------------|
| `slot_id` | `name` |
| `name` | `description` |
| `persona` | `instructions` |
| `tools` | `tools` |
| `allowed_paths` | `scope.allowed_paths` |
| `forbidden_paths` | `scope.forbidden_paths` |
| `[[transport]]` (kind=telegram) | `channels[]` (kind=telegram) |
| `[[transport]]` (kind=slack) | `channels[]` (kind=slack) |
| `[[transport]]` (kind=discord) | `channels[]` (kind=discord) |
| `[[transport]]` (kind=web) | `channels[]` (kind=webhook) |
| `[[transport]]` (kind=email) | `channels[]` (kind=email) |
| `[[transport]]` (kind=voice_twilio) | `channels[]` (kind=voice) |
| `[llm.override].model` | `model` |
| (no equivalent) | `triggers` |

## Steps

1. **Copy the slot TOML aside.** Don't delete it yet — you'll
   verify the new spec produces the same result.

2. **Write a new spec file** at e.g. `~/projects/agents/<slot>.yaml`
   using the slot fields above. Example:

   ```yaml
   # From slot_id = "harveychat"
   name: harveychat
   description: "Harvey chat agent"

   # From [llm.override].model = "anthropic/claude-sonnet-4-6"
   model: anthropic/claude-sonnet-4-6

   # From persona = "Sharp professional secretary"
   instructions: "Sharp professional secretary"

   # From tools = ["email", "calendar"]
   tools: [email, calendar]

   # From [[transport]] (kind=telegram, secret_env="HARVEYCHAT_TELEGRAM_TOKEN")
   channels:
     - kind: telegram
       token_env: HARVEYCHAT_TELEGRAM_TOKEN
       allowed_users: ["746496145"]

   scope:
     allowed_paths: ["~/MAKAKOO/data/harveychat/"]
     forbidden_paths: ["~/.ssh/", "~/.aws/"]
   ```

3. **Validate without creating:**
   ```bash
   makakoo agent validate-spec ~/projects/agents/harveychat.yaml
   ```

4. **Compare with the existing slot:**
   ```bash
   makakoo agent show harveychat
   # ... check the redacted TOML matches your spec
   ```

5. **Create the new slot from the spec:**
   ```bash
   makakoo agent create --specs ~/projects/agents/harveychat.yaml
   ```

6. **Verify the Flue project is correct:**
   ```bash
   cd $MAKAKOO_HOME/agents-flue/harveychat
   ls -la
   cat spec.yaml           # the spec verbatim
   cat package.json        # deps match your channels/triggers
   cat .env.example        # all env vars listed
   cat README.md           # channels/triggers documented
   ```

7. **Test the agent end-to-end** (trigger a channel message, verify
   the agent responds).

8. **Remove the old slot only after verification:**
   ```bash
   makakoo agent destroy harveychat
   ```

## Field mapping details

### Channels

slot TOML `[[transport]]` blocks map to spec `channels[]` entries.
The kind names differ slightly:

| slot kind | spec kind |
|-----------|-----------|
| `telegram` | `telegram` |
| `slack` | `slack` |
| `discord` | `discord` |
| `web` | `webhook` (spec adds the "hook" suffix for clarity) |
| `email` | `email` |
| `voice_twilio` | `voice` (spec drops the implementation detail) |

The `secret_env` / `app_token_env` / `team_id_env` / `secret_ref` /
`inline_secret_dev` / etc. fields all become env-var references in
the spec. The spec only stores the env var **name**; the actual
secret value lives in the operator's environment or secret store.

### LLM override

slot TOML `[llm.override].model` becomes spec `model`. The spec is
simpler — only the model identifier. Other LLM knobs (max_tokens,
temperature, reasoning_effort) are not yet represented in the spec
schema; they're deferred to V2.

### What the slot TOML has that the spec doesn't

- `process_mode` — always `"supervised_pair"` in V1. Slot-only field.
- `inherit_baseline` — always `true` in V1. Slot-only field.
- Per-transport config (`polling_timeout_seconds`, `allowed_chat_ids`,
  `team_id`, `mode`, `dm_only`, etc.) — V1 specs don't expose these.
  The Flue project uses sensible defaults; operators can edit the
  generated TS file post-scaffold for advanced tuning.

### What the spec has that the slot TOML doesn't

- `description` — shown in `makakoo agent list` and the Flue README
- `triggers` — cron + webhook trigger sources, not in the slot TOML
  at all
- Markdown in `instructions` — slot TOML stores as a plain string

## V1 limitations for migrated slots

- **Email/voice channels**: migrated from a working slot TOML will
  fail at scaffold time (deferred to V2). Workaround: rewrite as a
  `webhook` channel + custom `defineTool`, or keep the existing
  slot TOML and skip the migration.
- **Advanced per-transport config**: any non-default values (custom
  polling timeouts, OAuth2 vs app_password, etc.) are lost in the
  spec → slot round-trip. Operators must edit the generated TS file
  or the slot TOML directly to preserve them.

## See also

- `docs/agents/spec.md` — full spec schema reference
- `docs/walkthroughs/create-agent-from-spec.md` — end-to-end walkthrough
- `docs/user-manual/agent.md` — `makakoo agent` CLI reference
