# IDE Integration Guide

Connect Makakoo OS to your IDE.

## Overview

Makakoo can integrate with:

| IDE | Integration | Status |
|-----|------------|--------|
| VSCode | VSCode Copilot | ✅ |
| VSCode | Continue extension | ✅ |
| VSCode | Cline extension | ✅ |
| JetBrains | JetBrains AI | ✅ |
| Cursor | Cursor AI | ✅ |
| Neovim | Various plugins | 🟡 |

## VSCode Copilot

### Setup

1. Install VSCode
2. Install GitHub Copilot extension
3. Configure Makakoo as backend:

```json
// .vscode/settings.json
{
  "github.copilot.advanced": {
    "backend": "makakoo",
    "makakoo": {
      "enabled": true
    }
  }
}
```

### Using with Makakoo

Copilot in VSCode now has access to:
- Your Brain (journals + pages)
- Makakoo tools via sidebar
- Superbrain search

### Keybindings

| Key | Action |
|-----|--------|
| `Ctrl+Shift+M` | Open Makakoo panel |
| `Ctrl+Shift+Q` | Brain query |
| `Ctrl+Shift+S` | Search Brain |

---

## Continue Extension

[Continue](https://continue.dev) is an open-source Copilot alternative for VSCode and JetBrains.

### Installation

1. Install Continue extension from VSCode marketplace
2. Configure Makakoo:

```json
// .continue/config.json
{
  "models": [
    {
      "name": "makakoo",
      "provider": "custom",
      "api_base": "http://localhost:18080/v1",
      "api_key": "makakoo"
    }
  ],
  "allowAnonymousTelemetry": false
}
```

### Features

- Chat with your Brain
- Code with context from your notes
- Semantic search integration

---

## Cline Extension

[Cline](https://github.com/cline/cline) is an autonomous coding agent for VSCode.

### Installation

1. Install Cline from VSCode marketplace
2. Configure Makakoo MCP:

```json
// .vscode/mcp.json
{
  "servers": {
    "makakoo": {
      "command": "makakoo-mcp",
      "args": []
    }
  }
}
```

### Usage

Cline can now:
- Read from your Brain
- Write decisions to journals
- Query Superbrain for context

---

## JetBrains AI

### Setup

1. Install JetBrains AI plugin
2. Configure custom backend:

```properties
# jetbrains://settings/makakoo
makakoo.enabled=true
makakoo.mcp.path=/path/to/makakoo-mcp
```

### Features

- Ask questions about your Brain
- AI Assistant with your context
- Code completion with project memory

---

## Cursor

### Installation

1. Install Cursor
2. Makakoo auto-detects and integrates

### Configuration

```json
// .cursor/mcp.json
{
  "mcpServers": {
    "makakoo": {
      "command": "makakoo-mcp"
    }
  }
}
```

### Cursor-Specific Features

- `@brain` mention in composer
- Project memory panel
- AI Agent with your knowledge

---

## Neovim

### LSP Integration

Using `nvim-lspconfig` and `copilot.lua`:

```lua
-- init.lua
require('copilot').setup({
  suggestion = { enabled = true },
  panel = { enabled = true },
})

-- Add Makakoo tools
require('makakoo').setup({
  -- Your config
})
```

### Telescope Integration

Search your Brain from Neovim:

```lua
-- Search Brain
:Telescope makakoo search

-- Query Brain
:Telescope makakoo query
```

---

## Custom MCP Client

Any tool can use Makakoo MCP:

```python
import subprocess
import json

def makakoo_call(tool_name, params):
    """Call Makakoo MCP tool"""
    result = subprocess.run(
        ['makakoo-mcp', '--json'],
        input=json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": params
            }
        }).encode(),
        capture_output=True
    )
    return json.loads(result.stdout)

# Use
result = makakoo_call("brain_search", {"query": "polymarket"})
```

---

## Troubleshooting

### IDE Not Detecting Makakoo

```bash
# Check MCP server
makakoo-mcp --version

# Restart MCP
makakoo daemon restart
```

### Connection Refused

```bash
# Check daemon
makakoo daemon status

# Check MCP port
curl http://localhost:6333/health
```

### Slow Responses

```bash
# Check LLM gateway
curl http://localhost:18080/health

# Check vector DB
curl http://localhost:6333/collections
```

---

## See Also

- [MCP Tools Reference](../api/mcp-tools.md) — All available tools
- [Installation Guide](../getting-started.md) — Initial setup
- [Concepts Overview](./index.md) — Architecture context
