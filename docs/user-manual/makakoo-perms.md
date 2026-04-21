# `makakoo perms` — Runtime write-access grants

**Since:** v0.3 (baseline); hardened in v0.3.1 (rate-limit decrement, denial audits, `origin_turn_id` enforcement), v0.3.2 (Rust MCP handler parity), and v0.3.3 (grant ownership check on revoke, SANCHO idempotency, `--json` envelope).

By default, Makakoo agents can only write inside a small hardcoded *baseline*:

```
~/MAKAKOO/data/reports
~/MAKAKOO/data/drafts
~/MAKAKOO/tmp
/tmp
```

Everything else is refused by the handler. `makakoo perms` is the way to
extend that surface — at runtime, without editing code or restarting
anything.

Three equivalent surfaces share one backend (`$MAKAKOO_HOME/config/user_grants.json`):

| Surface | When to use |
|---|---|
| `makakoo perms` (this page) | scripted / CI workflows, manual admin |
| `grant_write_access` / `revoke_write_access` / `list_write_grants` MCP tools | conversational flow from any infected CLI |
| `perms_purge_tick` SANCHO task (every 900s) | automatic expiry cleanup; not user-facing |

All three write the same store, emit the same audit entries, and honor the
same guardrails.

---

## Synopsis

```bash
makakoo perms <subcommand> [options] [arguments]
```

## Subcommands

| Subcommand | Purpose |
|---|---|
| [`list`](#makakoo-perms-list) | Show baseline + active grants |
| [`grant`](#makakoo-perms-grant) | Create a new grant |
| [`revoke`](#makakoo-perms-revoke) | Revoke by id or by path |
| [`purge`](#makakoo-perms-purge) | Drop expired grants (SANCHO does this automatically) |
| [`audit`](#makakoo-perms-audit) | Query `logs/audit.jsonl` for perms events |
| [`show`](#makakoo-perms-show) | Show one grant record |

---

## `makakoo perms list`

```bash
makakoo perms list [--all] [--json]
```

| Flag | Meaning |
|---|---|
| `--all` | Also show today's expired grants |
| `--json` | Machine-readable envelope (v0.3.3) |

Prints a one-line summary:

```
Baseline: ~/MAKAKOO/data/reports, ~/MAKAKOO/data/drafts, ~/MAKAKOO/tmp, /tmp.
2 active grants: /Users/sebastian/sprint/** until 17:00 CEST,
/Users/sebastian/notes/** permanent. 1 expired today.
```

This is the same string the conversational `list_write_grants` tool returns —
agents quote it verbatim.

**`--json` envelope (v0.3.3):**

```json
{
  "schema_version": 1,
  "baseline": [
    "/Users/you/MAKAKOO/data/reports",
    "/Users/you/MAKAKOO/data/drafts",
    "/Users/you/MAKAKOO/tmp",
    "/tmp"
  ],
  "active": [
    {
      "id": "g_20260421_a3f21c4b",
      "scope": "fs/write:/Users/you/code/**",
      "created_at": "2026-04-21T14:00:00Z",
      "expires_at": "2026-04-21T15:00:00Z",
      "label": "debug session",
      "granted_by": "sebastian",
      "plugin": "cli",
      "origin_turn_id": "",
      "owner": "cli"
    }
  ],
  "expired_today_count": 0,
  "all": false
}
```

Same shape as the MCP `list_write_grants` response — callers use one
parser across both surfaces.

---

## `makakoo perms grant`

```bash
makakoo perms grant <path> [--for <duration>] [--label <text>]
                          [--plugin <name>] [--mkdir] [--yes-really]
```

| Flag | Default | Meaning |
|---|---|---|
| `<path>` | *(required)* | Directory to grant. Absolute, relative, or `~`-expanded |
| `--for` | `1h` | Duration — `30m`, `1h`, `24h`, `7d`, or `permanent` |
| `--label` | `""` | Short free-text tag for audit-log search. Truncated to 80 chars |
| `--plugin` | `cli` | Caller attribution — see [Plugin strings](#plugin-strings) below |
| `--mkdir` | off | Create the target directory if it doesn't exist |
| `--yes-really` | off | Required for `permanent` grants outside `$MAKAKOO_HOME` |

**Duration grammar is strict.** `30m | 1h | 24h | 7d | permanent`.
Anything else (`"for an hour"`, `"until tomorrow"`, `2h`, `45m`, `30s`) is
refused. Rationale: a loose grammar is a prompt-injection attack surface on
LLM-supplied durations.

**Scope refusals** (fire at the handler, not the LLM):

```
/          ~          ~/         $HOME
*          **         .          ./
$HOME/     ~/**       $HOME/**
```

These all return `scope too broad — grant a specific subdirectory`. The
glob grammar allows explicit paths with globs, e.g. `~/code/**/*.md`.

**Permanent-outside-HOME:** a `permanent` grant whose expanded path does
not start with `$MAKAKOO_HOME` requires `--yes-really`. Otherwise it's
rejected with a prompt to add the flag. Rationale: permanent grants have
no expiry, which makes them both risky and easy to forget.

**Example:**

```bash
makakoo perms grant ~/code/scratch/ --for 1h --label "debug session"
# → g_20260421_a3f21c4b

makakoo perms grant ~/long-project/ --for 7d
makakoo perms grant ~/MAKAKOO/notes/ --for permanent
makakoo perms grant /opt/things/ --for permanent --yes-really
```

---

## `makakoo perms revoke`

```bash
makakoo perms revoke <grant-id>
makakoo perms revoke --path <path>
makakoo perms revoke --path last          # newest active grant
```

Revoke by id is exact; revoke by `--path` finds the grant covering that
path (error if zero or ≥2 matches). The alias `last` (also `latest`)
resolves to the most-recently-created active grant — useful for "oops,
undo that grant I just made."

Revoke also **decrements the per-hour create counter** (v0.3.1) so a
cycle of `grant → revoke → grant` doesn't eat through the 50/hour cap.
Purge (expiry) does *not* decrement — that path is deliberately not a
capacity return.

**Ownership gate (v0.3.3).** A caller may revoke a grant only if:

- The caller's `plugin` string matches the grant's `owner` field,
  OR
- The caller is on the **admin bypass list**: `cli` or
  `sancho-native`.

`owner` defaults to the `plugin` that created the grant. So
cross-plugin revokes are refused unless you call from the CLI
(`plugin=cli` → admin bypass). Refusals emit an audit entry with
`correlation_id="reason:not_owner"` — grep
`logs/audit.jsonl` for it to track cross-plugin revoke attempts.

In practice the CLI and SANCHO purge always succeed; a compromised
conversational plugin can no longer wipe another agent's grants.

---

## `makakoo perms purge`

```bash
makakoo perms purge [--json]
```

Drops every grant whose `expires_at < now`. Emits one `perms/revoke`
audit entry per removed grant with
`correlation_id="reason:expired"`, `plugin="sancho-native"` or `cli` depending on who called.

The SANCHO task `perms_purge_tick` runs this every 900 seconds
automatically — you almost never need to call it manually. Use
`makakoo perms purge` only if you want an immediate cleanup.

**Idempotency gate (v0.3.3).** The SANCHO tick consults a 60s
cooldown stored at `state/perms_purge_last.json`. If a restart /
clock skew re-fires the tick within that window, the second run
returns `skipped (within Ns cooldown since last tick)` — no double
audit entries for the same revocations. The CLI
`makakoo perms purge` **bypasses** the gate (admin action).

---

## `makakoo perms audit`

```bash
makakoo perms audit [--since <duration>] [--plugin <name>] [--grant <id>] [--json]
```

Filters `$MAKAKOO_HOME/logs/audit.jsonl` to the perms-subsystem
entries (`verb=perms/grant` or `perms/revoke`).

| Flag | Meaning |
|---|---|
| `--since` | Only entries in the last `<duration>`. Grammar: `10m`, `1h`, `2d` |
| `--plugin` | Filter by caller attribution (`cli`, `claude-code`, `harveychat`, …) |
| `--grant` | Only entries for one grant id |
| `--json` | Emit the raw JSONL entries instead of the rendered table |

**Every grant event lands here.** As of v0.3.1, every **refusal** does too,
with a taxonomy tag on `correlation_id`:

| `correlation_id` | Meaning |
|---|---|
| `reason:too_broad` | Scope refused — `/`, `~`, bare `*`/`**`, etc. |
| `reason:bad_duration` | Duration outside `30m\|1h\|24h\|7d\|permanent` |
| `reason:permanent_outside_home_unconfirmed` | `permanent` outside `$MAKAKOO_HOME` without `yes-really` |
| `reason:rate_limit_active` | 20-active-grants cap hit |
| `reason:rate_limit_hourly` | 50/hour create cap hit |
| `reason:missing_origin_turn_id` | Conversational tool called without the host's turn-id (prompt-injection signature) |
| `reason:not_owner` | v0.3.3 — revoke refused because caller's plugin doesn't match the grant's `owner` |
| `reason:expired` | Grant rolled off by purge |

Intrusion detection on the grant subsystem is now possible post-incident —
an attacker hammering `grant_write_access` leaves a forensic trail.

---

## `makakoo perms show`

```bash
makakoo perms show <grant-id> [--json]
```

Prints one grant record. Useful for scripted verification.

---

## Rate limits

Global, per-user, enforced at the handler:

| Limit | Default | Counter file |
|---|---|---|
| Max active grants | 20 | — *(counted on read)* |
| Max create-ops per rolling hour | 50 | `$MAKAKOO_HOME/state/perms_rate_limit.json` |

When you hit either, the refusal reads:

```
rate limit: 20 active grants (max 20); revoke some or wait
rate limit: 50 grants created in the last hour (max 50); wait a bit
```

The counter is **sidecar-locked** (`<file>.json.lock`) — concurrent
grants / revokes / purges never corrupt it.

**v0.3.1 change:** revoking a grant now releases one slot from the
per-hour counter. A cycle of 50 `grant → revoke` pairs no longer locks
you out for an hour. Purge expiry deliberately does *not* release slots
(see `spec/USER_GRANTS.md §7`).

---

## Plugin strings

The `plugin` attribution on every audit entry is populated from
`$HARVEY_PLUGIN` — an env var the infected bootstrap sets for each CLI
host. Known values (see `spec/USER_GRANTS.md §11.1`):

| Value | Set by |
|---|---|
| `cli` | `makakoo perms` directly |
| `sancho-native` | `perms_purge_tick` SANCHO task |
| `harveychat`, `harveychat-telegram`, `harveychat-web` | HarveyChat bridge (by channel) |
| `claude-code`, `gemini-cli`, `codex`, `opencode`, `vibe`, `cursor`, `qwen`, `pi` | Per-CLI shell-rc exports installed by `makakoo infect --global` |
| `mcp` | Default fallback for MCP callers with no `HARVEY_PLUGIN` set |

Everything in the set `{claude-code, gemini-cli, codex, opencode, vibe,
cursor, qwen, pi, harveychat, harveychat-telegram, harveychat-web}` is a
**conversational channel**. Grants from these surfaces MUST carry an
`origin_turn_id` (v0.3.1 for the Python path, v0.3.2 for the Rust MCP
handler). A grant call from one of these plugins with an empty
`origin_turn_id` is rejected as a prompt-injection signature.

`cli` and `sancho-native` are NOT conversational — they don't carry a
human turn, and they're allowed to call without `origin_turn_id`.

---

## Three-layer resolver

When `write_file` is called on a path, the handler checks three layers
in order (all additive; a match anywhere permits the write):

1. **Baseline roots** — hardcoded. `~/MAKAKOO/data/reports`,
   `~/MAKAKOO/data/drafts`, `~/MAKAKOO/tmp`, `/tmp`.
2. **Plugin-manifest grants** — declarative grants in
   `plugins-core/<plugin>/plugin.toml` under `[capabilities].grants`.
3. **User grants** — everything `makakoo perms grant` or the
   conversational tools created.

If none match, you get:

```
write_file rejected: '/Users/sebastian/foo/bar.md' is outside the
allowed baseline roots and active grants
```

The canonical agent response to this rejection is to ask you whether
to call `grant_write_access(path, "1h")`. Never fabricate a grant
call — always quote the rejection verbatim and wait for a clear yes.

Full model: `spec/CAPABILITIES.md §1.11`. Threat model: `spec/USER_GRANTS_THREAT_MODEL.md`.

---

## See also

- [`spec/USER_GRANTS.md`](../../spec/USER_GRANTS.md) — authoritative schema, lock protocol, CLI + MCP contracts
- [`spec/USER_GRANTS_THREAT_MODEL.md`](../../spec/USER_GRANTS_THREAT_MODEL.md) — adversary register, residual risks
- [`spec/CAPABILITIES.md §1.11`](../../spec/CAPABILITIES.md) — three-layer additive model
- [`CHANGELOG.md`](../../CHANGELOG.md) — v0.3.1 + v0.3.2 hardening entries
