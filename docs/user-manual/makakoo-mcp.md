# `makakoo mcp` — CLI reference

`makakoo mcp` is a thin dispatcher that forwards arguments verbatim to the
`makakoo-mcp` binary — the Makakoo MCP stdio server. The server exposes all
Makakoo tools (Brain search, write grants, agent control, bucket ops, …)
over the MCP protocol to any MCP client: Claude Code, Claude Desktop,
Gemini CLI, Cursor, OpenCode, Vibe, Qwen, or any MCP-aware tool you wire
up yourself.

`makakoo infect` wires the MCP server into every detected CLI's config
automatically. Use `makakoo mcp` directly when you need to run the server
manually, test its health, or run it in HTTP mode for peer federation.

## `makakoo-mcp` flag reference

The `makakoo mcp` subcommand forwards all arguments to `makakoo-mcp`:

| Flag | Meaning |
|---|---|
| *(none)* | Run the MCP stdio loop (the normal mode for AI CLI integration). |
| `--health` | Print `{"ok":true,"tools":N}` and exit. Used by smoke tests and `makakoo infect --verify`. |
| `--list-tools` | Print the full `tools/list` descriptor array as pretty JSON and exit. |
| `--http <addr:port>` | Run as an HTTP server instead of stdio. Ed25519 auth is mandatory in this mode. |
| `--bind <ip>` | Override the bind interface when `--http` is set (default `127.0.0.1`). |
| `--trust-file <path>` | Path to the Ed25519 peer trust file (default `$MAKAKOO_HOME/config/peers/trusted.keys`). |
| `--signing-key <path>` | Path to this server's Ed25519 signing key (auto-generated on first run if absent). |

## Key use patterns

### Smoke-test the MCP server

```sh
# confirm the server starts, lists tools, and exits cleanly
makakoo mcp --health
# {"ok":true,"tools":41}

# inspect every tool descriptor
makakoo mcp --list-tools | jq '.[].name'
```

### Manual stdio run (for debugging an MCP client)

```sh
# run the stdio server in the foreground
# your MCP client connects via stdin/stdout
makakoo mcp
```

### HTTP mode for peer Makakoo federation

```sh
# start the HTTP MCP server on a fixed port
# peers must present a valid Ed25519 signature on every request
makakoo mcp --http 127.0.0.1:9090

# add a peer's pubkey to the trust file first:
makakoo adapter trust add --peer <name> --pubkey <base64-pubkey>
```

## MCP config snippet (manual wiring)

If you need to wire the server into a CLI config by hand instead of via
`makakoo infect`:

```json
{
  "harvey": {
    "command": "makakoo-mcp",
    "args": []
  }
}
```

## Related commands

- [`makakoo-infect.md`](makakoo-infect.md) — writes this config entry into all CLI hosts automatically
- [`makakoo-adapter.md`](makakoo-adapter.md) — adapter layer that can back the MCP server
- [`../concepts/architecture.md`](../concepts/architecture.md) — how MCP fits in the overall stack

## Common gotcha

**The AI CLI reports "server not found" or "spawn failed" for the `harvey` MCP entry.**
The `makakoo-mcp` binary is a sibling of `makakoo` on `$PATH` after
`cargo install`. If you ran `makakoo infect` before `cargo install` finished,
the MCP config entry points to a binary that does not exist yet. Fix:
run `cargo install makakoo makakoo-mcp` (or `makakoo install`) to ensure
the binary is on `$PATH`, then restart your AI CLI to pick up the new entry.
