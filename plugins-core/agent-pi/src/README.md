# agent-pi

Wraps `badlogic/pi-mono` as a first-class Makakoo worker. Pi is a lightweight
coding-agent CLI with its own session format, fork/rewind/label semantics, and
a JSONL RPC mode. `agent-pi` exposes pi to the rest of the Makakoo platform:

- SANCHO routes code-task subagent dispatches through pi via `pi_run`.
- MCP tools `pi_run`, `pi_session_fork`, `pi_session_label`,
  `pi_session_export`, `pi_set_model`, `pi_steer` speak to `pi --rpc` over
  stdio. (v0.2 Phase B.3/B.4 — handlers live in `makakoo-mcp`.)
- The SANCHO task `pi_session_sync` walks `~/.pi/agent/sessions/*.jsonl`
  every 10 min and writes Brain pages under
  `data/Brain/pages/pi-sessions/<id>.md`.

## Install prereqs

`agent-pi` does NOT install pi for you — that's a user action:

```bash
npm install -g @badlogic/pi-mono              # or pi's preferred install
pi extension install @traylinx/pi-switchai-provider
makakoo infect --slots pi                     # writes ~/.pi/AGENTS.md + memory symlink
makakoo plugin install agent-pi               # installs this plugin
```

## Doctor

```bash
makakoo plugin health agent-pi
# or directly:
python3 $MAKAKOO_HOME/plugins/agent-pi/src/doctor.py
```

Green requires:
1. `pi` binary on PATH (`pi --version` works)
2. `~/.pi/AGENTS.md` with a `harvey:infect-global v10+` marker
3. `@traylinx/pi-switchai-provider` extension installed
4. `~/.pi/memory` symlinked to `$MAKAKOO_HOME/data/auto-memory`

## Scope

This plugin is intentionally stateless. Pi sessions live under `~/.pi/agent/`,
owned by pi itself. Makakoo drives pi per request; long-lived state is
pi's responsibility. `start`/`stop` are no-ops — there is no daemon.

## Capability grants

```toml
grants = [
    "exec/binary:pi",                     # spawn the pi subprocess
    "fs/read:~/.pi",                      # read sessions for sync
    "fs/write:~/.pi/agent/sessions",      # fork/label writes
    "brain/write",                        # pi_session_sync writes Brain pages
]
```

No network grants — pi's own HTTP traffic goes through pi-switchai-provider,
which has its own grants in its extension manifest.
