# Consuming Makakoo from external agentic apps

Makakoo OS ships 6 portable tool families via MCP — `browse`,
`multimodal`, `pi`, `agents`, `wiki`, `skill_discover` — each
documented in its own `SKILL.md` (see
[`docs/plugins/skills-inventory.md`](../plugins/skills-inventory.md)).

This guide shows **four** ways to consume those tools from outside
Makakoo's own infected CLIs. Pick one based on your runtime. Every
route takes under 10 minutes; the LangChain snippet in §2 is
copy-pasteable and was dogfooded end-to-end before this doc shipped.

**Prereqs (all routes):** you have `makakoo-mcp` on `PATH` (output of
`cargo install --path makakoo-mcp` from the workspace, or whichever
release channel Makakoo ships). Verify with:

```bash
$ makakoo-mcp --help 2>&1 | head -2
Usage: makakoo-mcp [OPTIONS]
```

> **Private repo note:** Makakoo's source is private until the v0.6
> public-repo gate flips. Until then, external consumers must receive
> a distribution tarball or a read-granted clone from Sebastian — no
> `curl` one-liner install exists yet. Once public, this section will
> link the install-from-GitHub path.

## 1. MCP-native CLI hosts (claude-code, gemini-cli, codex, opencode, vibe, cursor, qwen-code, pi)

If you are on a CLI Makakoo already infects, the tools appear
automatically after `makakoo infect --global`. No additional wiring.
See `docs/plugins/browser-harness.md` for the end-to-end flow and
`harvey_browse` triggers. This guide is mostly for runtimes Makakoo
doesn't infect.

## 2. LangChain (or LangGraph) via `langchain-mcp-adapters`

LangChain ships a first-class MCP client that speaks stdio. Install
the adapter, point it at `makakoo-mcp`, and every MCP tool becomes a
LangChain `StructuredTool`.

### Fresh venv dogfood recipe

```bash
python3.13 -m venv /tmp/makakoo-langchain
source /tmp/makakoo-langchain/bin/activate
pip install --quiet langchain-mcp-adapters mcp
```

### Runnable snippet (40 lines)

```python
# save as try_makakoo.py
import asyncio
import os
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client, get_default_environment


async def main():
    # MCP's stdio_client sanitizes env to a hardcoded allowlist
    # (HOME/PATH/SHELL/etc). MAKAKOO_HOME is NOT in that list, so the
    # spawned makakoo-mcp would fall back to ~/.makakoo/ and find
    # nothing. Forward MAKAKOO_HOME explicitly. See §5 troubleshooting.
    env = get_default_environment()
    if "MAKAKOO_HOME" in os.environ:
        env["MAKAKOO_HOME"] = os.environ["MAKAKOO_HOME"]
    params = StdioServerParameters(
        command="makakoo-mcp",
        args=[],
        env=env,
    )
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()

            # 1. Discover what Makakoo exposes.
            tools = await session.list_tools()
            names = [t.name for t in tools.tools]
            print(f"tools available: {len(names)}")
            assert "skill_discover" in names, names

            # 2. Walk the skill tree — agents should always do this
            #    before claiming a capability.
            sd = await session.call_tool(
                "skill_discover",
                {"query": "browse", "limit": 5},
            )
            print("skill_discover(browse) →", sd.content[0].text[:200])

            # 3. Drive harvey_browse end-to-end. Prereq: Chrome with
            #    --remote-debugging-port=9222 + agent-browser-harness
            #    daemon running (BU_CDP_WS set).
            result = await session.call_tool(
                "harvey_browse",
                {"code": "goto('https://example.com'); print(page_info())"},
            )
            print("harvey_browse →", result.content[0].text[:200])


asyncio.run(main())
```

Run it:

```bash
python try_makakoo.py
```

Expected output:

```
tools available: 53
skill_discover(browse) → [{"name": "agent-browser-harness", ...}, ...]
harvey_browse → {"browser":"default","exit_code":0,"stderr":"",...,"title": "Example Domain", ...}
```

If `harvey_browse` returns a daemon-down error, see §5 troubleshooting.

### Wrapping the session for LangGraph

`langchain-mcp-adapters` exposes `load_mcp_tools(session)` to return
`StructuredTool` objects ready to drop into a LangGraph `ToolNode`:

```python
from langchain_mcp_adapters.tools import load_mcp_tools
tools = await load_mcp_tools(session)  # every MCP tool as a StructuredTool
```

## 3. OpenAI Assistants SDK

The Assistants SDK accepts arbitrary function tools. Convert each
Makakoo MCP tool's input schema into an Assistants function schema;
dispatch to `makakoo-mcp` over stdio in your tool handler.

```python
import json, subprocess

def makakoo_call(tool_name: str, args: dict) -> dict:
    """Invoke a single Makakoo MCP tool via one-shot stdio."""
    payload = json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05",
                    "capabilities": {}, "clientInfo": {"name": "oai-assistant", "version": "0"}},
    }) + "\n" + json.dumps({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": tool_name, "arguments": args},
    })
    proc = subprocess.run(
        ["makakoo-mcp"], input=payload, capture_output=True, text=True, timeout=30
    )
    # Parse the id=2 response line.
    for line in proc.stdout.splitlines():
        if '"id":2' in line:
            frame = json.loads(line)
            return frame.get("result")
    raise RuntimeError(proc.stderr)
```

Register each tool you want to surface as an Assistants function with
a schema derived from the MCP `tools/list` response. The handler
function routes to `makakoo_call`.

## 4. Cursor rules + ChatGPT custom instructions

Both runtimes accept plain-text "system rule" files. The good news:
each Makakoo `SKILL.md` body is **already** a portable rule — copy
the body (minus the "Prereqs for the agent runtime" section, which is
Makakoo-specific) into your `.cursor/rules/*.mdc` file or your
ChatGPT custom instructions box.

The agent won't be able to *call* `harvey_browse` from those runtimes
without a separate MCP hook (Cursor has one; ChatGPT doesn't), but it
**will** understand the decision tree + trigger patterns + hard rules
so it gracefully tells the user "this needs the Makakoo CLI locally"
instead of hallucinating browsing output.

Recommended for bootstrap portability:

- Copy the trigger patterns section (`When to reach for …`) verbatim.
- Copy the call shape JSON blocks — they document the tool interface.
- Skip the Python Rust implementation pointers.
- Paraphrase the "Hard rule" blocks if space is tight.

## 5. Troubleshooting matrix

| Symptom | Likely cause | Fix |
|---|---|---|
| `makakoo-mcp: command not found` | binary not on `PATH` | `cargo install --path makakoo-mcp` from the workspace checkout, or add `~/.cargo/bin` to `PATH` |
| `tools/list` returns nothing | stdio handshake swallowed by wrong framing | send `initialize` first, then `tools/list`. Don't skip the initialize call; MCP servers reject `tools/call` before it |
| `harvey_browse` returns `daemon not running` | agent-browser-harness daemon is down | start Chrome with CDP, resolve the WebSocket (`curl http://localhost:9222/json/version`), then `BU_CDP_WS=<ws-url>` + `daemon_admin.py start` |
| `harvey_browse` returns `DevToolsActivePort not found` | Chrome launched with a custom user-data-dir that the harness's built-in profile scan doesn't know about | set `BU_CDP_WS` to the `webSocketDebuggerUrl` field from `/json/version` — explicitly overrides the scan |
| install `agent-browser-harness` errors with "Package 'harness' requires a different Python" | stock `python3` on the system is older than 3.11 | `MAKAKOO_VENV_PYTHON=python3.13 makakoo plugin install --core agent-browser-harness` |
| `harvey_describe_*` returns 429 | rate-limited by the upstream vision model | the handler already retries with exponential backoff; if it still fails, tell the user and ask how to proceed — **do not** substitute a `WebFetch` / journal write (see `knowledge_ingest_vs_describe` rule) |
| `skill_discover` returns an empty array | EITHER `$MAKAKOO_HOME/plugins/` is unpopulated OR `MAKAKOO_HOME` isn't forwarded to the subprocess — MCP's `stdio_client` sanitizes env to a hardcoded allowlist that omits it | Forward `MAKAKOO_HOME` explicitly: `params = StdioServerParameters(..., env={**get_default_environment(), "MAKAKOO_HOME": os.environ["MAKAKOO_HOME"]})`. If `MAKAKOO_HOME` is unset in the parent shell, the binary falls back to `~/.makakoo/` (empty on most machines). |
| `harvey_browse` returns `agent-browser-harness venv python missing at ~/.makakoo/plugins/...` | same env-forwarding issue as above — the binary resolved the wrong `$MAKAKOO_HOME` | same fix — forward `MAKAKOO_HOME` through `stdio_client`'s env allowlist |
| session loses tool responses mid-conversation | stdio child killed by session timeout | wrap each call in its own `stdio_client` (§2 pattern) or pin `ClientSession` lifetime to the enclosing agent loop |

## 6. What NOT to assume

- **Internal-only tools are not surfaced.** `brain_*`, `harvey_superbrain_*`,
  `harvey_telegram_send`, `harvey_olibia_speak`, `harvey_swarm_run`,
  `sancho_*`, `chat_*`, `nursery_*`, `buddy_*`, `outbound_draft`,
  `costs_summary`, `grant_*`, `dream` — these exist in `tools/list`
  but mean nothing outside a running Makakoo. Filter them out or
  surface them as "requires Makakoo install". The skills inventory
  (`docs/plugins/skills-inventory.md`) lists exactly which are
  portable vs internal.
- **No free SaaS.** Makakoo has no managed endpoint; every install is
  local. Your external agent runs `makakoo-mcp` as a subprocess —
  there's no network hop, no auth header, no remote API.
- **Real Chrome only.** `harvey_browse` is Chrome-via-CDP; it won't
  drive Firefox or headless mode unless upstream browser-harness
  grows that support. Use other browser automation for non-Chrome
  targets.
- **`harvey_browse` is stateless per call.** Cross-call session state
  is planned for v0.6+ (see the v0.4 git-sourced-plugins memory's
  "Deferred" list). For now, chain calls with care: the daemon
  remembers a Chrome context, but each tool invocation spawns a fresh
  upstream `run.py` subprocess.

## 7. Going deeper

- Browser automation specifics: `docs/plugins/browser-harness.md`.
- Plugin install / update / outdated workflow: `docs/plugins/update-workflow.md`.
- Git-sourced plugin refs: `docs/plugins/git-sources.md`.
- Every portable tool's decision tree: the SKILL.md files under
  `plugins-core/<plugin>/SKILL.md` (or nested
  `plugins-core/lib-harvey-core/skills/<name>/SKILL.md`).
- Internal architecture: `docs/index.md`.

## Reverse direction

You have an agent you want **Makakoo to consume** (instead of the
other way around). See [`bring-your-own-agent.md`](bring-your-own-agent.md) —
one adapter.toml file, four template shapes (OpenAI-compat / subprocess
/ MCP stdio / peer-Makakoo-over-HTTP), scaffolded via
`makakoo adapter gen`.

## Changelog

- **2026-04-21 (v0.5 Phase D)** — First ship. LangChain snippet
  dogfooded in a fresh `/tmp/makakoo-langchain` venv; Chrome CDP
  + agent-browser-harness validated against `https://example.com`.
- **2026-04-21 (v0.6 Phase D)** — Cross-linked to
  `bring-your-own-agent.md`. Companion of this guide: how to make
  your agent a first-class swarm participant Makakoo can reach.
