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
