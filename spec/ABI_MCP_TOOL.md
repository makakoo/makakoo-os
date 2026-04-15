# ABI: MCP-Tool — v0.1

**Status:** v0.1 LOCKED — 2026-04-15
**Kind:** `mcp-tool` (or any plugin kind declaring `[mcp.tools]`)
**Owner:** Makakoo kernel, `crates/mcp/`
**Promotes to v1.0:** after Phase E dogfooding

---

## 0. What an MCP tool is

An **MCP tool** is a JSON-RPC-exposed capability available to every
infected host through the Makakoo MCP gateway. Hosts like Claude Code,
Cursor, and Gemini connect to the Makakoo MCP server as a stdio JSON-RPC
client. The gateway fans tool calls out to the right handler.

Tools can be:
- **In-process Rust handlers** living inside `crates/mcp/src/handlers/`
  (fastest, used for kernel-provided tools like `brain_search`,
  `brain_write_journal`, `sancho_status`)
- **Out-of-process subprocess handlers** living inside a plugin (any
  language, used for everything else)

Both shapes follow the same ABI from the client's perspective.

## 1. Contract

An MCP tool is declared in a plugin manifest:

```toml
[mcp]
tools = [
  { name = "arbitrage_status",   handler = "arbitrage.mcp:status",     schema = "schemas/status.json" },
  { name = "arbitrage_tick_now", handler = "arbitrage.mcp:tick_now",   schema = "schemas/tick.json" },
]
```

## 2. Tool declaration fields

| Field | Required | Type | Meaning |
|---|---|---|---|
| `name` | yes | string | MCP tool name exposed to clients |
| `handler` | yes | string | Language-specific handler reference |
| `schema` | no | path | Optional JSON schema file for input/output validation |
| `description` | no | string | Shown in host's tool-list UI |
| `deprecated` | no | bool | Hide from tool list; still responds to calls |

**Naming rules:** snake_case, globally unique across all plugins,
prefixed with the plugin's domain (`arbitrage_*`, `gym_*`, `brain_*`).
Reserved prefixes: `makakoo_*` (kernel-only tools).

## 3. Handler reference syntax

The `handler` field varies by plugin language:

### 3.1 Python

```toml
handler = "arbitrage.mcp:status"
```

Format: `<module>:<function>`. The kernel imports the module via the
plugin's venv Python and calls the function. Function signature:

```python
def status(params: dict) -> dict:
    """Handle the arbitrage_status MCP tool call."""
    # params = input dict from the MCP client
    # return = output dict
    return {"status": "ok", "pnl": 42.1}
```

### 3.2 Rust (in-process, kernel-shipped only)

```toml
handler = "crate::tier_a::brain::brain_search"
```

Rust handlers are compiled into the MCP gateway binary and run
in-process (no subprocess spawn). Only plugins shipped inside the
makakoo-os monorepo can use this form — external plugins must use
subprocess handlers.

### 3.3 Node

```toml
handler = "./mcp-handlers/status.js:default"
```

Format: `<file>:<export-name>`. Kernel loads via the plugin's
`node_modules`.

### 3.4 Shell/binary

```toml
handler = "./bin/mcp-handler --tool status"
```

Kernel spawns the binary with declared args. Stdin = JSON params.
Stdout = JSON result. Non-zero exit = error.

## 4. JSON-RPC protocol

The Makakoo MCP gateway speaks standard Model Context Protocol over
stdio. Tool calls arrive as:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "arbitrage_status",
    "arguments": {}
  }
}
```

And responses:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [{
      "type": "text",
      "text": "{\"status\": \"ok\", \"pnl\": 42.1}"
    }]
  }
}
```

The gateway:
1. Parses incoming requests
2. Routes by tool name to the declared handler
3. Calls the handler (in-process or subprocess)
4. Wraps the handler's output in the MCP `content` array
5. Returns the response

## 5. Schema validation

If `[mcp.tools].schema` points to a JSON schema file, the gateway
validates inputs against it before calling the handler. Invalid inputs
return an error without spawning the handler.

Schemas follow JSON Schema Draft 2020-12.

**Example** (`schemas/status.json`):
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "verbose": { "type": "boolean", "default": false }
  }
}
```

## 6. Tool registration lifecycle

1. At plugin install, kernel parses `[mcp.tools]` from manifest
2. Registers each tool's name → handler mapping in the MCP gateway's
   tool table
3. On infected host reconnect, the new tool appears in the host's
   available tools list (`tools/list` MCP method)
4. At plugin uninstall, tools are removed from the table
5. Calls to uninstalled tool names return a "tool not found" error

## 7. Invocation contract (subprocess handlers)

For Python/Node/shell handlers, the kernel spawns a subprocess per call
with:

- **stdin:** JSON params from the MCP request
- **stdout:** expected to be JSON result
- **stderr:** captured to `$MAKAKOO_HOME/logs/mcp/<tool-name>/<timestamp>.stderr`
- **timeout:** default 30 seconds, configurable via
  `[mcp.tools] ... timeout = "2m"`
- **env:** standard plugin env (SOCKET_PATH, PLUGIN_NAME, etc.)
- **capability socket:** available via `MAKAKOO_SOCKET_PATH`

**Exit codes:**
- 0 = success, stdout is JSON result
- 1 = handler error, stderr contains error message
- 2 = invalid input
- Other = unknown error

## 8. Forbidden for tools at v0.1

- **Long-running tools.** A tool call that takes more than 30 seconds
  by default is probably the wrong shape — use a SANCHO task + poll
  the result via a status tool
- **Binary content in text/ fields.** Use the MCP content block
  protocol properly; binary data goes in explicit binary blocks
- **Side effects outside declared capabilities.** A tool that reads
  the Brain must declare `brain/read` in its plugin's grants
- **Registering tool names outside the plugin's declared prefix.**
  Plugin `arbitrage` can register `arbitrage_*` but not `github_*`
  (lint check at install time)

## 9. Versioning

Same semver rules.

Tool **renames** are major version bumps because existing hosts will
see the old name disappear. New tools are minor bumps.

## 10. Example: `brain_search` (Rust in-process)

Lives in `crates/mcp/src/handlers/tier_a/brain.rs`. Handler signature:

```rust
pub async fn brain_search(
    ctx: &ToolContext,
    params: Value,
) -> Result<Value, ToolError> {
    let query = params["query"].as_str().ok_or(ToolError::InvalidParams)?;
    let limit = params["limit"].as_u64().unwrap_or(10) as usize;
    let hits = ctx.brain.search(query, limit).await?;
    Ok(json!({"hits": hits}))
}
```

Registered at kernel boot as a built-in tool. Always available on every
Makakoo install.

## 11. Example: `arbitrage_status` (Python subprocess)

Plugin manifest:
```toml
[mcp]
tools = [
  { name = "arbitrage_status", handler = "arbitrage.mcp:status" },
]
```

Handler:
```python
# plugins-core/agent-arbitrage/arbitrage/mcp.py
def status(params: dict) -> dict:
    from arbitrage import state
    from makakoo import Client
    client = Client.connect_from_env()

    s = state.load()
    return {
        "pending_trades": len(s.pending),
        "total_pnl": s.pnl_cumulative,
        "last_tick": s.last_tick_ts,
    }
```

Host invokes via MCP:
```
> Check my arbitrage status

[tool: arbitrage_status]
{"pending_trades": 2, "total_pnl": 42.1, "last_tick": "2026-04-15T17:42:01Z"}
```

---

**Status:** v0.1 LOCKED.
