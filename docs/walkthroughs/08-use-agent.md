# Walkthrough 08 — Use an agent

## What you'll do

Understand the **agents-as-plugins** model, find every agent your install shipped with, inspect one in detail, look at the health of its background process, and find its logs. Reuse the lifecycle controls from [Walkthrough 03](./03-install-plugin.md) to disable / re-enable an agent you don't need right now.

**Time:** about 5 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md), [Walkthrough 03](./03-install-plugin.md) (for plugin vocabulary).

## Agents = plugins with `kind = "agent"`

In Makakoo there is no separate "agent runtime" and no `makakoo agent` subcommand. Instead, an agent is just a **plugin whose `plugin.toml` declares `kind = "agent"`**. Agent plugins ship with:

- A `[entrypoint]` section declaring `start` / `stop` / `health` scripts.
- A `[capabilities]` section declaring what it's allowed to touch.
- Usually an MCP-tool export or a SANCHO task that's the agent's external surface.

When the Makakoo daemon starts (from `makakoo daemon install`, part of walkthrough 01), it walks every installed plugin, finds the agents, and spawns their `start` entrypoints. When you `makakoo plugin disable <agent-name>`, the daemon notices on next load and doesn't respawn it.

## Steps

### 1. List every installed agent

```sh
makakoo plugin list | grep -E "Agent\b"
```

Expected output (truncated — your distro shipped a subset):

```text
agent-arbitrage-agent            1.0.0    Agent    Python   yes    path:/.../plugins-core/agent-arbitrage-agent
agent-browser-harness            0.1.0    Agent    Python   yes    path:/.../plugins-core/agent-browser-harness
agent-career-manager             1.0.0    Agent    Python   yes    path:/.../plugins-core/agent-career-manager
agent-harveychat                 1.0.0    Agent    Python   yes    path:/.../plugins-core/agent-harveychat
agent-knowledge-extractor        1.0.0    Agent    Python   yes    path:/.../plugins-core/agent-knowledge-extractor
agent-marketing-blog             1.0.0    Agent    Shell    yes    path:/.../plugins-core/agent-marketing-blog
agent-marketing-linkedin         1.0.0    Agent    Shell    yes    path:/.../plugins-core/agent-marketing-linkedin
agent-marketing-twitter          1.0.0    Agent    Shell    yes    path:/.../plugins-core/agent-marketing-twitter
agent-meta-harness-agent         0.1.0    Agent    Python   yes    path:/.../plugins-core/agent-meta-harness-agent
agent-multimodal-knowledge       1.0.0    Agent    Python   yes    path:/.../plugins-core/agent-multimodal-knowledge
agent-pg-watchdog                1.0.0    Agent    Python   yes    path:/.../plugins-core/agent-pg-watchdog
agent-pi                         0.1.0    Agent    Python   yes    path:/.../plugins-core/agent-pi
```

Short tour of what each does — full manual pages land under `docs/agents/` in Phase 2 of the docs sprint:

| Agent | Purpose (one line) |
|---|---|
| `agent-arbitrage-agent` | Polymarket arbitrage signal loop |
| `agent-browser-harness` | Real-Chrome CDP driver for `harvey_browse` (Walkthrough 07) |
| `agent-career-manager` | Inbound/outbound recruiter workflow |
| `agent-harveychat` | Telegram + local chat gateway to Harvey |
| `agent-knowledge-extractor` | Auto-ingest URLs, PDFs, audio, video mentioned in the Brain |
| `agent-marketing-*` | Per-channel draft generators (blog, LinkedIn, Twitter) |
| `agent-meta-harness-agent` | Self-improvement loop (watches SANCHO failures → proposes fixes) |
| `agent-multimodal-knowledge` | Omni image/audio/video understanding + `knowledge_ingest` |
| `agent-pg-watchdog` | Postgres health watchdog |
| `agent-pi` | Pi CLI bridge (makes `pi` a usable worker from every infected CLI) |

### 2. Inspect one agent

Let's pick `agent-knowledge-extractor` — it's what runs behind the scenes when you ingest a document (walkthrough 09).

```sh
makakoo plugin info agent-knowledge-extractor
```

Expected output:

```text
agent-knowledge-extractor v1.0.0
  summary: Auto-ingest URLs, PDFs, audio, video mentioned in the Brain
  kind:     Agent
  language: Python
  enabled:  yes
  root:     /Users/you/MAKAKOO/plugins/agent-knowledge-extractor
  license:  MIT

  effective grants (incl. auto-defaults):
    - exec/shell
    - fs/read:$MAKAKOO_HOME/plugins/agent-knowledge-extractor
    - fs/write:$MAKAKOO_HOME/data/knowledge
    - mcp/register:knowledge_ingest
    - net/http:*
    - state/plugin:$MAKAKOO_HOME/state/agent-knowledge-extractor

  mcp tools:
    - knowledge_ingest

  lock entry:
    ...
```

The `mcp tools:` section lists what the agent exposes to every infected CLI. `effective grants` says exactly what the agent is allowed to touch — read this block to understand blast radius before enabling a new agent.

### 3. Confirm the agent is actually running

The daemon spawns agents from their `start` entrypoint. Find the running process:

```sh
ps -ef | grep -v grep | grep agent-knowledge-extractor
```

Expected output (your PID and path differ):

```text
sebastian   12345  python /Users/you/MAKAKOO/plugins/agent-knowledge-extractor/main.py
```

If there's no match and the plugin is enabled, the daemon probably can't spawn it — jump to troubleshooting.

### 4. Find the agent's logs

Every agent writes to `~/MAKAKOO/data/logs/`:

```sh
ls -la ~/MAKAKOO/data/logs/ | grep knowledge-extractor
```

Expected output:

```text
-rw-r--r--   1 you  staff   12345   Apr 24 15:42   agent-knowledge-extractor.out.log
-rw-r--r--   1 you  staff     234   Apr 24 15:42   agent-knowledge-extractor.err.log
```

Tail the stdout log to see what the agent is doing right now:

```sh
tail -20 ~/MAKAKOO/data/logs/agent-knowledge-extractor.out.log
```

### 5. Disable an agent you don't need

Say you don't do Polymarket trading and want to silence the arbitrage-agent:

```sh
makakoo plugin disable agent-arbitrage-agent
```

Expected output:

```text
agent-arbitrage-agent disabled
restart the daemon (or next sancho tick) to deregister tasks
```

Then:

```sh
makakoo daemon restart
```

The daemon reloads, notices `enabled = false`, does not respawn the agent. The agent's Python process (if any) gets cleaned up.

### 6. Re-enable when you need it back

```sh
makakoo plugin enable agent-arbitrage-agent
makakoo daemon restart
```

Back to normal.

## Manual control (advanced — bypass the daemon)

If the daemon is mis-managing a specific agent and you need to drive it directly, every agent plugin ships with entrypoint scripts you can call by hand. For `agent-browser-harness`:

```sh
cd ~/MAKAKOO/plugins/agent-browser-harness
.venv/bin/python daemon_admin.py start
.venv/bin/python daemon_admin.py health   # "OK" (exit 0) or "DOWN" (exit 1)
.venv/bin/python daemon_admin.py stop
```

The exact script name (`daemon_admin.py` here) is declared in the plugin's `plugin.toml` under `[entrypoint]`. Read the manifest to find it:

```sh
grep -A3 "^\[entrypoint\]" ~/MAKAKOO/plugins/<agent-name>/plugin.toml
```

> **Known doc gotcha (DOGFOOD-FINDINGS F-008):** some SKILL.md files refer to `makakoo agent start <name>` — that subcommand does not exist today. Use `makakoo daemon restart` + plugin `enabled` toggle, or call the entrypoint script directly.

## What just happened?

- **Agents ≠ a separate runtime.** They're plugins with a `kind = "agent"` label and a lifecycle. The daemon is the lifecycle manager.
- **You already have ~11 agents running** from the `core` distro. Most of them do nothing unless triggered (a SANCHO interval, an MCP tool call). A few (`pg-watchdog`, `arbitrage-agent`) do active background work — check logs before assuming an agent is idle.
- **Every agent has a bounded capability surface.** `plugin info` shows the grants. If you see something you don't want an agent to have, the only correct response is to disable the plugin or fork it and edit its manifest — you cannot narrow grants at runtime.
- **The daemon is a supervisor**, not a magic substrate. You can replace it with a direct script call (`python daemon_admin.py start`) and the agent still works. The daemon just makes start-on-boot + crash-recovery + log aggregation one layer.

## If something went wrong

| Symptom | Fix |
|---|---|
| `plugin list` shows an agent as `yes` but no process running | Check `ls ~/MAKAKOO/data/logs/<agent>.err.log` for the startup error, usually a missing dependency. If it's Python, the agent's `install.sh` probably didn't build the venv — rerun `makakoo plugin install --core <agent-name>`. |
| Daemon log full of `entrypoint not found` | The plugin's `[entrypoint].start` path is relative to the plugin dir — check the manifest; the script may have been deleted or renamed. |
| `makakoo daemon restart` doesn't kill a stuck Python agent | That agent's start script probably detaches (double-fork). Kill it by PID: `pkill -f '<agent-name>'`, then restart the daemon. |
| Every agent shows as disabled after a Makakoo upgrade | A schema migration probably reset `plugins.lock`. Run `makakoo plugin sync` (re-registers everything in `plugins-core/`) then `makakoo daemon restart`. |

## Next

- [Walkthrough 09 — Ingest a document](./09-ingest-document.md) — uses `agent-knowledge-extractor` + `agent-multimodal-knowledge`, which you just met.
- [Walkthrough 10 — Mascot mission](./10-mascot-mission.md) — mascots are a specialization of agents; see one fire a scheduled mission.
