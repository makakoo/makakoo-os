# SPRINT-OLIBIA-REMOTE-OPERATOR-V2-2026-04-30

## Goal

Turn Olibia/HarveyChat into a real remote operator for Sebastian's Mac without weakening Makakoo's permission boundary.

Olibia must be able to help when Sebastian is physically away from the computer, but risky actions must be explicit, auditable, revocable, and short-lived.

## Current facts from live audit

- Olibia runs through `agent-harveychat` / `core.chat` / `core.agent.harvey_agent`.
- Telegram and Discord are configured.
- Cortex Memory is enabled and shipped inside `makakoo-os`; no separate Cortex repository is needed.
- Existing permission management is real but write-oriented:
  - store: `$MAKAKOO_HOME/config/user_grants.json`
  - audit: `$MAKAKOO_HOME/logs/audit.jsonl`
  - tools: `grant_write_access`, `revoke_write_access`, `list_write_grants`
  - CLI: `makakoo perms {list,grant,revoke,purge,audit,show}`
- Existing `run_command` is intentionally safe/read-only and rejects non-whitelisted actions.
- Therefore: remote operator needs an action-grant layer, not a bypass.

## Locked decisions

### LD-1 — Reuse Makakoo grants, do not invent a second permission store

Remote action grants use the same `user_grants.json` file and the same sidecar lock, expiry, owner, rate-limit, and audit patterns.

### LD-2 — Exact action grants only

A v1 action grant authorizes one exact normalized target. No wildcards. No "open shell for one hour" grant.

Example:

```text
action:shell/run:<sha256-prefix-16>
```

### LD-3 — Permanent remote action grants require explicit hard confirmation

`duration="permanent"` requires `confirm="yes-really"`, even inside `$MAKAKOO_HOME`.

### LD-4 — Hard-block dangerous command shapes even with grants

Credential exfiltration, privilege escalation, shell pipe installers, root deletion, disk erase, and shell metacharacter injection are denied before grant matching.

### LD-5 — Origin-turn binding is runtime-owned

The model should not invent `origin_turn_id`. `HarveyAgent._tool_calling_loop()` injects the trusted task id into grant tool args when available.

## Implementation delivered in this sprint

### Phase 1 — Action grant core

- New module: `plugins-core/lib-harvey-core/src/core/capability/action_perms.py`
- New scope family: `action:<kind>:<target-hash>`
- Supported actions:
  - `shell/run`
  - `browser/control` (reserved for browser-harness follow-up)
  - `process/control` (reserved)
  - `app/control` (reserved)
- Exact target hashing: SHA-256 prefix 16.
- Audit verbs:
  - `perms/action_grant`
  - `action/shell_run`

### Phase 2 — HarveyChat tools

Added tools to `HARVEY_TOOLS`:

- `grant_action_access(action, target, duration?, label?, confirm?, user_turn_id?)`
- `list_action_grants(include_expired?)`
- `operator_run_command(command, timeout_seconds?)`

`operator_run_command` refuses until a matching exact `shell/run` action grant exists.

### Phase 3 — Prompt and docs

Updated:

- `core/chat/bridge.py` system prompt
- `docs/agents/agent-harveychat.md`
- `docs/user-manual/makakoo-perms.md`
- `spec/USER_GRANTS.md`

### Phase 4 — Tests

Added:

- `plugins-core/lib-harvey-core/src/core/capability/test_action_perms.py`

Coverage:

- action grants require `origin_turn_id` on conversational plugins
- shell commands require exact action grant
- command mismatch is denied
- hard-blocked command denied even before grant
- action grant listing works

## Non-goals for this sprint

- Full browser-harness control from Telegram. Permission scope exists; execution tool is deferred until Chrome CDP setup is reliable.
- GUI automation / AppleScript omnipotence. Needs separate risk review.
- `makakoo perms grant-action` Rust CLI creation command. Rust CLI can list/revoke action grants by id; creation remains HarveyChat Python for v1.

## Verification commands

```bash
cd /Users/sebastian/makakoo-os
PYTHONPATH=plugins-core/lib-harvey-core/src python3 -m pytest -q plugins-core/lib-harvey-core/src/core/capability/test_action_perms.py
PYTHONPATH=plugins-core/lib-harvey-core/src python3 -m pytest -q plugins-core/lib-harvey-core/src/core/cortex/tests
(cd plugins-core/agent-harveychat/python && python3 -m pytest -q)
```

## Done criteria

- [x] Remote actions are behind explicit grants.
- [x] Grants share Makakoo permission store.
- [x] Tool docs instruct Olibia to ask before granting.
- [x] Tests cover grant and denial paths.
- [x] Live Olibia restarted on patched code.
- [x] Repo committed and pushed.
