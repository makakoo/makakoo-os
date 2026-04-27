# `agent-meta-harness-agent`

**Summary:** Experimental meta-agent — spawns and supervises ad-hoc task agents on demand.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-meta-harness-agent/`

## When to use

Advanced pattern: when a single task needs a **bespoke, short-lived agent** that doesn't fit the existing catalog (e.g., a one-off data-pipeline sweep, a custom research scrape). Meta-harness generates the agent, runs it to completion, and tears it down.

**Most users never need this directly.** It's invoked by other agents (notably `agent-meta-harness` in the GYM flywheel) to auto-fix surfaced issues.

## Stability caveat

The plugin's own summary says "experimental". Interfaces may change between v0.1 releases. Don't build production flows on top of this until it stabilizes — use a specific-purpose agent instead.

## Start / stop

Managed by the daemon:

```sh
makakoo plugin info agent-meta-harness-agent
makakoo plugin disable agent-meta-harness-agent
makakoo plugin enable agent-meta-harness-agent
makakoo daemon restart
```

Most users can disable this until they have a concrete use.

## Where it writes

- **State:** `~/MAKAKOO/state/agent-meta-harness-agent/` — spawned-agent transcripts and teardown logs.
- **Logs:** `~/MAKAKOO/data/logs/agent-meta-harness-agent.{out,err}.log`

## Health signals

- `ps -ef | grep meta-harness-agent` — one supervisor process when enabled.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| Supervisor spawns a child agent that never exits | Infinite-loop detection not wired yet | Manual `pkill` on the child; file an issue. |
| Meta-harness can't find capability template | Template directory moved between refactors | Reinstall the plugin from `--core` to pick up current layout. |

## Capability surface

- `exec/shell` — spawning child agents.
- `fs/read` + `fs/write` — own state dir.
- `llm/chat` — generating the child agent's prompt.

## Remove permanently

```sh
makakoo plugin uninstall agent-meta-harness-agent --purge
```
