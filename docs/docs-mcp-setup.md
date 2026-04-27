# Docs MCP Setup

`makakoo-docs-mcp` is an MCP server baked into the `makakoo` binary that lets
any AI CLI query the Makakoo OS and Tytus documentation in real time, with
citations pointing back to the exact source file. Add one entry to your CLI's
MCP config, restart the CLI, and from that point any question about Makakoo
commands, plugins, agents, adapters, or Tytus pod management will be answered
by the real documentation — no copy-paste, no hallucinated flags, no stale
blog posts.

---

## Prerequisites

`makakoo` must be on your `$PATH`. Verify:

```sh
which makakoo
```

Expected output: a path such as `/Users/<you>/.cargo/bin/makakoo`. If you get
`command not found`, install Makakoo first — see
[`getting-started.md`](./getting-started.md).

---

## Claude Code

Claude Code reads MCP servers from two places:

- **Project-level:** `.mcp.json` in the project root (checked into the repo)
- **Global (all projects):** `~/.claude/mcp.json`

Use the project-level file if you only want docs-MCP in one repo. Use the
global file to have it everywhere.

**Config file path:**

```
~/.claude/mcp.json             # global
<project-root>/.mcp.json       # project-scoped
```

**Add this block** (merge into the `mcpServers` object if it already exists):

```json
{
  "mcpServers": {
    "makakoo-docs": {
      "command": "makakoo",
      "args": ["docs-mcp", "--stdio"]
    }
  }
}
```

**Restart:** Close the Claude Code session and reopen it. Use `/mcp` to
confirm `makakoo-docs` appears in the tool list.

**Smoke test:** Ask Claude: "How do I install a plugin in Makakoo OS?" Expect
an answer that ends with a citation like `docs/plugins/index.md`.

---

## Gemini CLI

Gemini CLI reads MCP servers from:

```
~/.gemini/settings.json
```

The `mcpServers` block follows the same key-value shape as Claude Code.

**Add this entry** to the `mcpServers` object:

```json
{
  "mcpServers": {
    "makakoo-docs": {
      "command": "makakoo",
      "args": ["docs-mcp", "--stdio"]
    }
  }
}
```

**Full example** (existing servers left in place):

```json
{
  "mcpServers": {
    "harvey": {
      "command": "/Users/<you>/.cargo/bin/makakoo-mcp",
      "args": []
    },
    "makakoo-docs": {
      "command": "makakoo",
      "args": ["docs-mcp", "--stdio"]
    }
  }
}
```

**Restart:** Exit Gemini CLI and relaunch. Gemini picks up MCP changes on
startup.

**Smoke test:** Ask Gemini: "What MCP tools does the Makakoo harvey server
expose?" and expect a cited answer drawn from the adapter docs.

---

## OpenCode

OpenCode stores its MCP config in:

```
~/.config/opencode/opencode.json
```

The MCP block key is `mcp` (not `mcpServers`). Each entry has a `command`
array and an `enabled` flag.

**Add this entry** to the `mcp` object:

```json
{
  "mcp": {
    "makakoo-docs": {
      "command": ["makakoo", "docs-mcp", "--stdio"],
      "enabled": true,
      "type": "local"
    }
  }
}
```

**Restart:** Quit and relaunch OpenCode. The new server appears in the
`/tools` panel on next open.

**Smoke test:** Ask OpenCode: "How do I refresh the docs index in Makakoo?"
and expect a citation to `docs/user-manual/` or similar.

---

## Cursor

Cursor reads MCP servers from:

```
~/.cursor/mcp.json             # global (all workspaces)
<workspace>/.cursor/mcp.json   # workspace-scoped
```

The format is identical to Claude Code's `mcpServers` shape.

**Config snippet:**

```json
{
  "mcpServers": {
    "makakoo-docs": {
      "command": "makakoo",
      "args": ["docs-mcp", "--stdio"],
      "type": "stdio"
    }
  }
}
```

**Restart:** Use the Cursor command palette: `Developer: Reload Window`
(or close and reopen). Cursor does not hot-reload MCP configs.

**Smoke test:** Open Cursor's AI panel and ask: "How does Makakoo's plugin
system work?" Look for a `docs/plugins/` citation in the response.

---

## Qwen

Qwen CLI mirrors the Gemini settings format exactly:

```
~/.qwen/settings.json
```

**Add this entry** to the `mcpServers` object:

```json
{
  "mcpServers": {
    "makakoo-docs": {
      "command": "makakoo",
      "args": ["docs-mcp", "--stdio"]
    }
  }
}
```

**Restart:** Exit and relaunch `qwen`. MCP servers are loaded at startup.

**Smoke test:** Ask Qwen: "What is the Octopus peer system in Makakoo?" and
check for a citation to `docs/concepts/` or the architecture docs.

---

## Vibe

Vibe (Mistral's CLI) uses a TOML config file:

```
~/.vibe/config.toml
```

MCP servers are declared as `[[mcp_servers]]` table-array entries:

```toml
[[mcp_servers]]
transport = "stdio"
name      = "makakoo-docs"
command   = "makakoo"
args      = ["docs-mcp", "--stdio"]
```

**Restart:** Exit Vibe and relaunch. TOML config is read at startup.

**Smoke test:** Ask Vibe: "How do I use `makakoo sancho tick`?" and expect
a cited answer from the Sancho or user-manual docs.

---

## Codex

Codex (OpenAI's CLI) also uses a TOML config file:

```
~/.codex/config.toml
```

Add an `[[mcp_servers]]` entry:

```toml
[[mcp_servers]]
transport = "stdio"
name      = "makakoo-docs"
command   = "makakoo"
args      = ["docs-mcp", "--stdio"]
```

**Restart:** Exit and relaunch `codex`. Config is read at startup.

**Smoke test:** Ask Codex: "How do I connect Makakoo to a Tytus pod?" and
expect a Tytus-docs citation.

---

## Refreshing the docs

The docs corpus is baked into the binary at build time. When a new Makakoo
release ships updated documentation, the in-binary index is already fresh as
long as you upgrade Makakoo.

To pull the latest docs without upgrading the full binary:

```sh
makakoo docs --update
```

This fetches the current `docs/` and `spec/` tree from
`github.com/makakoo/makakoo-os/main`, rebuilds the FTS5 index, and writes the
result to `~/.makakoo/docs-cache/index.db`. The MCP server automatically
prefers the cache over the baked-in corpus on next launch.

**Automated weekly refresh (opt-in):**

```sh
makakoo sancho enable docs_weekly_refresh
```

After enabling, SANCHO runs `makakoo docs --update` once a week in the
background. Check status with `makakoo sancho status`.

---

## Troubleshooting

### MCP server does not appear in the tool list

The `makakoo` binary is not on the PATH that your CLI uses.

1. Verify from the terminal you use to launch the CLI:
   ```sh
   which makakoo
   ```
2. If it prints a path, your CLI's shell environment may differ. Add an
   absolute path in the config instead of the bare name:
   ```json
   { "command": "/Users/<you>/.cargo/bin/makakoo" }
   ```
   Replace `/Users/<you>/.cargo/bin/makakoo` with whatever `which makakoo`
   returned.
3. If `which makakoo` itself returns nothing, you need to install Makakoo
   first — see [`getting-started.md`](./getting-started.md).

### Search returns no results

The docs index is stale or missing. Rebuild it:

```sh
makakoo docs --update
```

If `--update` itself errors, check your internet connection and try again. The
fallback baked-in index will serve until the cache is populated.

### Initialize failed

You are missing the `--stdio` flag. The MCP server only communicates over
standard input/output when launched with `--stdio`. Confirm your config
includes the flag:

```json
{ "command": "makakoo", "args": ["docs-mcp", "--stdio"] }
```

Running `makakoo docs-mcp` without `--stdio` starts an interactive shell
session, not an MCP server, which causes the CLI to hang waiting for the
protocol handshake.

### Citation path does not resolve to a real file

The `path` field returned by every tool is repo-relative — it starts at the
`makakoo-os` repo root, for example `docs/plugins/index.md`. It is not an
absolute path on your machine.

To open it in a browser:

```
https://github.com/makakoo/makakoo-os/blob/main/<path>
```

To open it locally, clone the repo first and then resolve from the repo root:

```sh
git clone https://github.com/makakoo/makakoo-os ~/makakoo-os
open ~/makakoo-os/<path>     # macOS
xdg-open ~/makakoo-os/<path> # Linux
```

### Server starts but tool calls return empty results on older releases

Versions below `0.1.1` do not include the `docs-mcp` subcommand. Upgrade:

```sh
cargo install makakoo
```

Then restart your CLI.

---

## Citation format

Every result from `makakoo_docs_search`, `makakoo_docs_read`, and
`makakoo_docs_topic` includes a `path` field. This field is a repo-relative
path rooted at the `makakoo-os` repository:

| `path` value | What it points to |
|---|---|
| `docs/plugins/index.md` | Plugin system overview |
| `docs/concepts/architecture.md` | Makakoo OS architecture reference |
| `spec/USER_GRANTS.md` | Write-permission grant spec |
| `docs/agents/consuming-makakoo-externally.md` | External agent integration |

**To resolve a citation in a browser:**

```
https://github.com/makakoo/makakoo-os/blob/main/<path>
```

Example:

```
https://github.com/makakoo/makakoo-os/blob/main/docs/plugins/index.md
```

**To resolve a citation locally** (after cloning the repo):

```sh
cat ~/makakoo-os/<path>
```

**What your AI CLI shows:** Each CLI renders tool results differently. Claude
Code and OpenCode display the `path` inline in the response. Gemini CLI and
Cursor fold it into a collapsible tool-call block. In all cases the raw JSON
returned by the tool contains the `path` field, which you can use to navigate
directly to the source.

---

## Quick reference

| CLI | Config file | Key format |
|---|---|---|
| Claude Code | `~/.claude/mcp.json` or `.mcp.json` | `mcpServers.<name>.command` |
| Gemini CLI | `~/.gemini/settings.json` | `mcpServers.<name>.command` |
| OpenCode | `~/.config/opencode/opencode.json` | `mcp.<name>.command[]` |
| Cursor | `~/.cursor/mcp.json` | `mcpServers.<name>.command` |
| Qwen | `~/.qwen/settings.json` | `mcpServers.<name>.command` |
| Vibe | `~/.vibe/config.toml` | `[[mcp_servers]]` TOML array |
| Codex | `~/.codex/config.toml` | `[[mcp_servers]]` TOML array |
