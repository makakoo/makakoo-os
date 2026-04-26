# Troubleshooting subagents

Common failure modes when running multi-bot subagents and how to
remediate them.

## "Agent slot 'X' not found"

```
Agent slot 'secretary' not found at ~/MAKAKOO/config/agents/secretary.toml.
Run 'makakoo agent create secretary' to create it.
```

**Cause:** The supervisor (LaunchAgent / systemd) launched a
gateway with `MAKAKOO_AGENT_SLOT=secretary` but no matching TOML
exists.

**Fix:** Either run `makakoo agent create secretary …` or remove
the supervisor unit pointing at the missing slot.

The exit code is `64` (UNIX `EX_USAGE`). Supervisors should treat
this as a permanent failure and stop restarting — it won't fix
itself.

---

## Telegram: `Unauthorized` on getMe

`makakoo agent validate harveychat` reports:

```
  ✗ telegram-main (telegram): config error: telegram getMe failed: Unauthorized
```

**Cause:** Bot token revoked / regenerated via `@BotFather`.

**Fix:**

1. `/revoke` and `/token` in `@BotFather` to mint a fresh token.
2. `makakoo secret set agent/<slot>/telegram-main/bot_token <new-token>`
3. `makakoo agent validate <slot>` — should now pass.
4. `makakoo agent restart <slot>` to pick up the new token.

---

## Slack: `team_id mismatch`

```
slack team_id mismatch: TOML='T0123ABCD' but auth.test returned 'T9999OTHER'
```

**Cause:** The bot token was issued by a different Slack workspace
than the TOML claims.

**Fix:** Either update the TOML to the correct `team_id` (most
likely you copy-pasted the wrong token) or rotate the token from
the right workspace.

---

## Slack: WebSocket reconnect storm

`agent status secretary` shows Slack `state = reconnecting` and
`errors_1h` climbing every few minutes.

**Cause:** Either:
- The app token (`xapp-…`) is wrong / revoked / rate-limited.
- Slack-side outage (rare).
- The `apps.connections.open` rate limit (10 connections / minute
  per app) tripped because something else is dialling the same
  app token.

**Fix:**

1. Run `makakoo agent validate secretary` — if `apps.connections.open`
   fails, rotate the app token under `Basic Information → App-Level
   Tokens` in `api.slack.com/apps`.
2. If validate passes but the loop still flaps, check
   `~/MAKAKOO/data/agents/secretary/slack-main.log` for the per-
   reconnect error message; persistent network errors point at
   firewall / DNS issues for `wss.slack.com`.

The reconnect backoff caps at 60s, so even a fully broken app
token only burns ~1 connection / minute (well under the rate
limit).

---

## IPC: `gateway_unavailable` drops in transport log

```
{"event":"ipc.gateway_unavailable","transport_id":"telegram-main","drop":true}
```

**Cause:** The Python gateway process died or hasn't started yet.
Phase 1 IPC is at-most-once: in-flight inbound frames are dropped
during gateway downtime, not buffered.

**Fix:**

1. `makakoo agent status <slot>` — if `gateway: dead`, the
   supervisor will restart it within ~5s.
2. If it keeps dying, check `~/MAKAKOO/data/agents/<slot>/agent.log`
   for the Python traceback.
3. Frames dropped during the downtime are NOT re-played. The
   user will need to resend the message. Phase 4 dogfood will
   tell us whether at-most-once is acceptable in practice; if not,
   a follow-on sprint adds at-least-once with idempotency keys.

---

## bound_to_agent: grant invisible to the wrong slot

User reports: "I granted Olibia write access to `~/Shared/`, but
Career can't write there either."

**Cause:** Phase 3 grants are bound to the issuing slot. A grant
issued by Olibia (slot `harveychat`) is invisible to Career
(slot `career`). This is the locked behavior — agents shouldn't
inherit each other's elevated permissions.

**Fix:** Issue the grant from each slot that needs it, or revoke
and re-issue as a machine-global grant via the CLI:

```sh
makakoo perms revoke <grant-id>
makakoo perms grant fs/write:~/Shared/ --label shared --plugin cli
```

CLI grants are `bound_to_agent: None` (machine-global) and visible
to every slot.

---

## "duplicate transport.id" on agent create

```
duplicate transport.id 'telegram-main' in slot — every [[transport]]
must have a slot-unique id
```

**Cause:** The slot has two `[[transport]]` blocks with the same
`id`. Phase 1 `transport_id` is the PRIMARY routing key, so two
adapters with the same `id` would alias each other on outbound
demux.

**Fix:** Rename one of them (e.g. `telegram-main` and
`telegram-secondary` if you really do have two Telegram bots
attached to the same slot — yes, this is supported, just give them
distinct ids).

---

## "duplicate bot identity" on agent create

```
two transports of kind 'telegram' resolve to the same identity
(account_id='12345678', tenant=None); transport 'telegram-secondary'
is the duplicate.
```

**Cause:** Two Telegram `[[transport]]` blocks have different
`transport.id` but resolve (via `getMe.id`) to the same bot.
Polling the same bot twice would race on the `getUpdates` offset.

**Fix:** Remove one of the duplicate transport blocks, OR mint a
genuinely separate bot token.

---

## Slack `dm_only = false` requires `channels` list

```
transport 'slack-main' kind=slack: channels list is required when dm_only = false
```

**Cause:** You enabled channel events but didn't restrict which
channels. Phase 2 schema validation rejects this (would otherwise
spam-route every channel the bot is invited to).

**Fix:** Add `channels = ["C0123DEFG"]` to `[transport.config]`
(the Slack channel id, copy from the channel's "About" panel).

---

## Slack: `invalid Socket Mode app token`

`makakoo agent validate <slot>` reports:

```
slack apps.connections.open (Socket Mode probe) failed: not_allowed_token_type
```

**Cause:** The `app_token` slot was populated with a regular bot
token (`xoxb-…`) or a user token (`xoxp-…`) instead of an
app-level token (`xapp-…`). Socket Mode requires the app-level
variant generated under `Basic Information → App-Level Tokens`.

**Fix:** Mint a fresh app token with the `connections:write` scope,
then `makakoo secret set agent/<slot>/slack-main/app_token <xapp-…>`.

---

## Cross-transport outbound rejected

Tool call returns:

```
RouterError::UnknownTransport { slot_id: "secretary", transport_id: "slack-main" }
```

Or a logged WARN:

```
outbound transport_id 'slack-main' has no matching transport on slot 'secretary' — cross-transport reply is forbidden in v1
```

**Cause:** The Python gateway tried to send a reply on a
`transport_id` that doesn't match the inbound turn's originating
transport. v1 forbids cross-transport replies — every reply MUST
go back to the channel the inbound message arrived on.

**Fix:** This is a contract violation by the gateway, not a user
error. If you see this in production, it's a bug in the dispatch
layer: file an issue with the matching inbound + outbound frame
JSON. The router never invokes the adapter when this trips, so no
message goes out.

---

## Per-slot queue overflow (`queue.overflow`)

```
{"event":"queue.overflow","transport_id":"telegram-main","action":"drop_newest"}
```

**Cause:** The per-slot asyncio queue (locked at 100 frames) is
full. Either:
- The LLM dispatcher is wedged (slow tool call, infinite loop in
  the model).
- A transport is hosing the slot with messages faster than the
  LLM can process them.

The newest frame is dropped (NOT the oldest) so already-queued
messages still get a reply — this favors fairness over recency.

**Fix:** Check `makakoo agent status <slot>` for `queue_depth`. If
it's stuck at 100, restart the slot's gateway:
`makakoo agent restart <slot>`. If it climbs again, the LLM
backend is the bottleneck — investigate the gateway log for
slow tool calls.

---

## Tool not in scope (`ToolNotInScope`)

LLM response surfaces:

```
tool 'run_command' is not in scope for slot 'career'; allowed: brain_search, write_file, linkedin, gmail
```

**Cause:** The LLM tried to invoke a tool not on the slot's
`tools` whitelist. Phase 3 enforces least-privilege.

**Fix:** Either grant the tool by editing the slot's TOML and
re-validating:

```sh
# in ~/MAKAKOO/config/agents/career.toml:
tools = ["brain_search", "write_file", "linkedin", "gmail", "run_command"]
makakoo agent restart career
```

Or — preferably — leave the whitelist tight and rephrase the user
request so the agent uses an allowed tool.

---

## Path not in scope (`PathNotInScope`)

LLM response surfaces:

```
path '/etc/passwd' is not in scope for slot 'career'; allowed: ~/CV/, ~/MAKAKOO/data/career/; forbidden: (none)
```

Or the least-privilege variant:

```
path '/etc/passwd' is not in scope for slot 'career'; allowed: (none — least-privilege default); forbidden: (none)
```

**Cause:** The path the LLM tried to read or write is outside the
slot's `allowed_paths`, OR `allowed_paths` is empty (which denies
everything by default).

**Fix:** Add the path to `allowed_paths` in the slot's TOML, OR
issue a runtime grant via `makakoo perms grant` (CLI grants are
machine-global; MCP-issued grants bind to the calling slot per
Phase 3).

---

## "Inline secret value used (dev-only fallback)"

WARNING in the agent log on startup:

```
WARN inline secret value used (dev-only fallback) — move to env var
or makakoo secret store before production
```

**Cause:** The TOML's `inline_secret_dev` field is populated
because neither `secret_env` (process env var) nor `secret_ref`
(makakoo keyring) resolved.

**Fix:** Run `makakoo secret set <ref> <value>`, then either
remove the inline value from the TOML or leave it as a fallback
(the keyring entry takes precedence anyway). For production
deployments, populate `secret_env` and inject via the
LaunchAgent / systemd unit's environment block.
