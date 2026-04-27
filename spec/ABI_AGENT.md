# ABI: Agent — v0.1

**Status:** v0.1 LOCKED — 2026-04-15
**Kind:** `agent`
**Owner:** Makakoo kernel, `crates/core/src/abi/agent.rs`
**Promotes to v1.0:** after Phase E dogfooding

---

## 0. What an agent is

An **agent** is a long-running process with a lifecycle managed by the
daemon. Agents keep state, react to events, and run proactively. Typical
agents: `arbitrage` (trading), `harveychat` (chat UI), `btc-sniper`
(market monitor), `career-manager` (job search), `knowledge-extractor`
(research).

Agents are **always subprocesses** owned by the daemon (D5). The daemon
handles start, stop, restart, health, and crash recovery.

## 1. Contract

An agent plugin is a directory containing:

- `plugin.toml` with `kind = "agent"`
- `AGENT.md` — human-readable description + configuration notes
- Entrypoint script with `start`, `stop`, `health` verbs
- Required `[state]` dir for persistent data
- Typically `[sancho.tasks]` + `[mcp.tools]` + `[infect.fragments]`
- `[capabilities]` declaring exactly which verbs the agent uses

## 2. Minimal manifest

```toml
[plugin]
name = "agent-example"
version = "0.1.0"
kind = "agent"
language = "python"
summary = "Minimal example agent"

[source]
path = "plugins-core/agent-example"

[abi]
agent = "^0.1"

[depends]
python = ">=3.11"

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
start = ".venv/bin/python -m example.main --start"
stop = ".venv/bin/python -m example.main --stop"
health = ".venv/bin/python -m example.main --health"

[capabilities]
grants = ["brain/read", "state/plugin"]

[state]
dir = "$MAKAKOO_HOME/state/agent-example"
retention = "keep"
```

## 3. Lifecycle

### 3.1 start

`[entrypoint].start` is invoked when:
- The daemon boots and the agent is installed
- The user runs `makakoo agent start <name>` explicitly
- The daemon restarts after a crash

**Contract:**
- Start must return within 30 seconds (configurable via `[entrypoint].
  start_timeout`)
- Non-zero exit = start failed, daemon logs the error and backs off
- Exit code 0 does NOT mean the agent is done — it means start
  succeeded. The agent is expected to fork/detach or enter its main
  loop after exit-0 from start
- Stdout/stderr during start are captured to
  `$MAKAKOO_HOME/logs/agents/<name>/start.log`

**Daemonization pattern:**
```python
# example/main.py
def start():
    if already_running():
        print("already running")
        return 0
    pid = os.fork()  # or asyncio.create_subprocess + detach
    if pid == 0:
        main_loop()
    return 0
```

**Alternative: supervised mode.** Agents can declare
`[entrypoint].supervised = true` to have the daemon itself run the
agent's main loop as a child process. In supervised mode, `start` is
the main loop — the daemon keeps it alive, and restart-on-crash is
handled by the SANCHO scheduler.

```toml
[entrypoint]
supervised = true
start = ".venv/bin/python -m example.main"
stop = "kill $MAKAKOO_PLUGIN_PID"      # daemon handles this automatically
```

Supervised mode is the recommended pattern for new agents (simpler, no
daemonization code in the plugin).

### 3.2 stop

`[entrypoint].stop` is invoked when:
- The daemon is shutting down
- The user runs `makakoo agent stop <name>` or `makakoo plugin
  uninstall <name>`
- The daemon is restarting the agent after a crash

**Contract:**
- Stop must complete within 30 seconds (configurable via
  `[entrypoint].stop_timeout`)
- Non-zero exit = agent refused to stop cleanly; daemon kills the
  process tree with SIGKILL after the timeout
- Stop should flush state and close network connections cleanly
- `MAKAKOO_PLUGIN_PID` env var is set to the agent's main process PID

### 3.3 health

`[entrypoint].health` is invoked periodically (default every 60s,
configurable via `[entrypoint].health_interval`) by the daemon.

**Contract:**
- Health must return within 5 seconds
- Exit code 0 = healthy, non-zero = unhealthy
- Stdout on exit 0: JSON object with agent-reported metrics (optional)
- Three consecutive unhealthy returns → daemon restarts the agent

**Example healthy response:**
```json
{"status": "ok", "uptime_s": 3600, "pending_trades": 2, "last_tick": "2026-04-15T17:42:01Z"}
```

## 4. Crash recovery

If the agent process exits unexpectedly (not via `stop`):
1. Daemon detects the exit (SIGCHLD or poll)
2. Logs the crash to `$MAKAKOO_HOME/logs/agents/<name>/crashes.jsonl`
3. Restarts with exponential backoff: 1s, 2s, 5s, 15s, 60s, 300s,
   max 600s
4. After 5 consecutive crashes within 15 minutes, agent is marked
   **quarantined** and not restarted until the user runs
   `makakoo agent unquarantine <name>`

**State integrity:** the agent is responsible for leaving its state
dir in a consistent state across crashes. Recommended pattern:
write-temp + fsync + rename for every state update.

## 5. SANCHO tasks + MCP tools

Agents typically declare `[sancho.tasks]` for scheduled work and
`[mcp.tools]` for tools exposed through the gateway. These are
registered at agent start and unregistered at stop.

See `PLUGIN_MANIFEST.md §9` and `§10` for the declaration format.

## 6. Bootstrap fragments

Agents can contribute fragments via `[infect.fragments]` to teach
infected hosts about themselves. Example:

```toml
[infect.fragments]
default = "fragments/arbitrage.md"
```

Fragment content:
```markdown
<!-- makakoo:fragment:arbitrage -->

An `arbitrage` agent is running in the background on this machine.
It trades Polymarket BTC momentum with a 5-minute tick. You can
query its status via `harvey_superbrain_query "arbitrage status"`
or call the `arbitrage_status` MCP tool directly.

<!-- makakoo:fragment:arbitrage-end -->
```

## 7. Capabilities typically needed

Most agents declare some combination of:

```toml
[capabilities]
grants = [
  "brain/read",
  "brain/write",             # for journaling
  "llm/chat",                # scope to specific models for safety
  "llm/embed",               # for memory search
  "net/http:<endpoint-glob>", # for external APIs
  "state/plugin",            # own state dir
  "secrets/read:<key-name>", # for API keys
  "sancho/register:<task>",  # auto-granted via [sancho.tasks]
  "mcp/register:<tool>",     # auto-granted via [mcp.tools]
]
```

## 8. Forbidden for agents at v0.1

- **Multiple main processes.** An agent has exactly one main process
  (supervised mode) or exactly one detached process (daemonized mode).
  Agents that need to manage multiple workers should spawn them as
  children and supervise them internally.
- **Binding to external network interfaces.** Agents listen on
  localhost only (if they listen at all). Cross-machine communication
  goes through explicit `net/http` capabilities.
- **Using `exec/shell`.** Combined with persistent state, this is
  currently refused at install time. Use scoped `exec/binary` with an
  allowlist instead.

## 9. Versioning

Same semver rules as all ABIs (see `PLUGIN_MANIFEST.md §18`).

**v0.1 → v1.0 promotion:** after Phase E when at least one agent plugin
has survived a full install → run → stop → update → reinstall cycle
without schema changes.

## 10. Example: `agent-arbitrage`

**Full manifest:** `PLUGIN_MANIFEST.md §16.2`

**Lifecycle:**
```sh
$ makakoo agent start arbitrage
starting agent arbitrage...
  pid: 82411
  listening on: localhost:0 (no external)
  sancho tasks: arbitrage_tick, arbitrage_evening_report
  mcp tools: arbitrage_status, arbitrage_tick_now
  health: ok
started

$ makakoo agent health arbitrage
{
  "status": "ok",
  "uptime_s": 3600,
  "pending_trades": 2,
  "last_tick": "2026-04-15T17:42:01Z"
}

$ makakoo agent stop arbitrage
stopping agent arbitrage...
  flushing state...
  closing polymarket connection...
stopped
```

---

**Status:** v0.1 LOCKED.
