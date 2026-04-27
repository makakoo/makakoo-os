# `agent-pi`

**Summary:** Pi ([badlogic/pi-mono](https://github.com/badlogic/pi-mono)) wrapped as a first-class Makakoo worker — subagent for code tasks + session tools.
**Kind:** Agent (plugin) · **Language:** Python · **Source:** `plugins-core/agent-pi/`

## When to use

- You want to delegate a bounded code task to pi and get the result back (e.g. *"write a throwaway script that parses this log format"*).
- You want to use pi as a third validator in `lope negotiate` alongside Claude and Gemini.
- You need to run a long task with token-caveman voice that keeps to strict completion criteria.

## Prerequisites

- `pi` binary on `$PATH`. If not present, install via the pi project's own instructions (not bundled in this plugin).
- For LLM routing, pi reads its own config; Makakoo does not tunnel pi through `switchAILocal` unless you configure it via [`pi-switchai-provider`](https://github.com/traylinx/pi-switchai-provider) separately.

## Start / stop

Pi is **not a long-lived process**. It runs per-request via `pi --rpc`. The agent entry in Makakoo exists so `AgentRunner` can invoke it like any other agent (dispatch, lope validator, etc.).

```sh
makakoo plugin info agent-pi
makakoo plugin disable agent-pi
makakoo plugin enable agent-pi
# daemon restart not needed — pi is per-request
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-pi/sessions/` — transcripts of each pi session.
- **Logs:** `~/MAKAKOO/data/logs/agent-pi.{out,err}.log`

## Health signals

- `which pi && pi --version` — returns a version.
- `pi --rpc --help` — the RPC mode is available.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `pi: command not found` | `pi` binary not on `$PATH` | Install pi (see the pi project's README — this plugin does not bundle it). |
| Pi times out during `lope negotiate` | Too many parallel pi sessions eating token quota | Reduce concurrent `lope negotiate` runs; or switch to a different validator for this negotiation. |
| Pi returns valid JSON but the MakakooAdapter rejects it | ABI mismatch between pi version and plugin expectations | Check plugin version vs pi version in the plugin's `README.md`. |

## Capability surface

- `exec/shell` — invoking `pi --rpc`.
- `fs/read` + `fs/write` — own state dir for sessions.

## Remove permanently

```sh
makakoo plugin uninstall agent-pi --purge
```

Removing this agent removes pi from the lope validator pool; lope ensembles fall back to whatever validators remain.
