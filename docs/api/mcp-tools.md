# MCP Tools Reference

Complete reference for all Makakoo MCP tools.

## Overview

Makakoo exposes 40+ tools via the MCP (Model Context Protocol) server.

```
┌─────────────────────────────────────────────────────────────┐
│                    MCP CLIENT                                │
│           (Claude Code / Gemini / etc.)                       │
│                                                              │
│  { "method": "tools/call", "params": { "name": "..." } }  │
└────────────────────────────┬────────────────────────────────┘
                             │ stdio
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                   makakoo-mcp                                │
│                 (JSON-RPC Server)                           │
│                                                              │
│  Tools: brain, superbrain, sancho, plugins, secrets        │
└────────────────────────────┬────────────────────────────────┘
                             │ Unix socket
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                    makakoo-core                             │
│                   (Rust Kernel)                             │
│                                                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────────┐  │
│  │  Brain   │ │ Superbrain│ │ SANCHO   │ │ Capabilities│  │
│  └──────────┘ └──────────┘ └──────────┘ └─────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

## Tool Categories

| Category | Count | Description |
|----------|-------|-------------|
| Brain | 8 | Memory read/write/search |
| Superbrain | 3 | Vector + FTS search |
| SANCHO | 4 | Task management |
| Plugin | 5 | Plugin lifecycle |
| Secret | 3 | Secrets management |
| LLM | 4 | LLM gateway access |
| System | 5 | System info/status |

## Brain Tools

### brain_read

Read Brain files.

```json
{
  "name": "brain_read",
  "params": {
    "path": "journals/2026_04_20.md"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | string | Yes | Path relative to Brain root |

**Returns:** File contents or error.

---

### brain_write

Write to Brain.

```json
{
  "name": "brain_write",
  "params": {
    "entry": "- Did something at [[timestamp]]",
    "journal": true,
    "date": "2026_04_20"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `entry` | string | Yes | Content to write |
| `journal` | boolean | No | Write to journal (default: true) |
| `date` | string | No | Journal date (default: today) |
| `page` | string | No | Page path (overrides journal) |

---

### brain_create_page

Create a new page.

```json
{
  "name": "brain_create_page",
  "params": {
    "path": "projects/my-project.md",
    "content": "# My Project\n\n..."
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | string | Yes | Page path |
| `content` | string | Yes | Page content |
| `overwrite` | boolean | No | Overwrite existing (default: false) |

---

### brain_search

Full-text search.

```json
{
  "name": "brain_search",
  "params": {
    "query": "polymarket",
    "limit": 10
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | string | Yes | Search query |
| `limit` | number | No | Max results (default: 10) |
| `type` | string | No | Filter: "journal", "page", "all" |

---

### brain_recent

Get recent entries.

```json
{
  "name": "brain_recent",
  "params": {
    "days": 7
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `days` | number | No | Days back (default: 7) |
| `limit` | number | No | Max entries (default: 50) |

---

### brain_list_pages

List all pages.

```json
{
  "name": "brain_list_pages",
  "params": {
    "path": "projects"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | string | No | Subdirectory filter |

---

### brain_list_journals

List journals.

```json
{
  "name": "brain_list_journals",
  "params": {
    "year": 2026,
    "month": 4
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `year` | number | No | Year filter |
| `month` | number | No | Month filter |

---

### brain_delete_page

Delete a page.

```json
{
  "name": "brain_delete_page",
  "params": {
    "path": "projects/old-project.md"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `path` | string | Yes | Page path to delete |

---

## Superbrain Tools

### superbrain_query

Semantic query with LLM synthesis.

```json
{
  "name": "superbrain_query",
  "params": {
    "question": "What did I decide about the database?",
    "model": "minimax/ail-compound"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `question` | string | Yes | Natural language question |
| `model` | string | No | Model to use (default: configured) |
| `limit` | number | No | Context limit |

---

### superbrain_search

Vector + FTS hybrid search.

```json
{
  "name": "superbrain_search",
  "params": {
    "query": "arbitrage trading strategies",
    "limit": 10,
    "mode": "hybrid"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `query` | string | Yes | Search query |
| `limit` | number | No | Max results (default: 10) |
| `mode` | string | No | "vector", "fts", or "hybrid" |

---

### superbrain_index

Trigger index rebuild.

```json
{
  "name": "superbrain_index",
  "params": {
    "force": true
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `force` | boolean | No | Force rebuild (default: false) |

---

## SANCHO Tools

### sancho_status

Get task status.

```json
{
  "name": "sancho_status",
  "params": {}
}
```

**Returns:**
```json
{
  "tasks": [
    {
      "name": "dream",
      "type": "native",
      "interval": "4h",
      "last_run": "2026-04-20T08:00:00Z",
      "next_run": "2026-04-20T12:00:00Z",
      "status": "ok"
    }
  ]
}
```

---

### sancho_run

Run a task manually.

```json
{
  "name": "sancho_run",
  "params": {
    "task": "dream",
    "force": true
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `task` | string | Yes | Task name |
| `force` | boolean | No | Skip gates (default: false) |

---

### sancho_history

Get task history.

```json
{
  "name": "sancho_history",
  "params": {
    "task": "dream",
    "limit": 20
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `task` | string | No | Filter by task |
| `limit` | number | No | Max entries (default: 20) |

---

### sancho_pause

Pause/resume SANCHO.

```json
{
  "name": "sancho_pause",
  "params": {
    "action": "pause"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `action` | string | Yes | "pause" or "resume" |

---

## Plugin Tools

### plugin_list

List installed plugins.

```json
{
  "name": "plugin_list",
  "params": {
    "type": "agent"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `type` | string | No | Filter: "agent", "skill", "task" |

---

### plugin_info

Get plugin details.

```json
{
  "name": "plugin_info",
  "params": {
    "name": "arbitrage"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name` | string | Yes | Plugin name |

---

### plugin_install

Install a plugin.

```json
{
  "name": "plugin_install",
  "params": {
    "source": "skill-research-arxiv",
    "core": true
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `source` | string | Yes | Plugin source |
| `core` | boolean | No | Install from core |
| `url` | string | No | Git URL |
| `path` | string | No | Local path |

---

### plugin_uninstall

Uninstall a plugin.

```json
{
  "name": "plugin_uninstall",
  "params": {
    "name": "arbitrage"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name` | string | Yes | Plugin name |

---

### plugin_status

Get plugin health.

```json
{
  "name": "plugin_status",
  "params": {
    "name": "arbitrage"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name` | string | Yes | Plugin name |

---

## Secret Tools

### secret_list

List secrets.

```json
{
  "name": "secret_list",
  "params": {}
}
```

**Returns:**
```json
{
  "secrets": [
    { "name": "POLYMARKET_API_KEY", "type": "api_key" }
  ]
}
```

---

### secret_read

Read a secret.

```json
{
  "name": "secret_read",
  "params": {
    "name": "POLYMARKET_API_KEY"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name` | string | Yes | Secret name |

---

### secret_set

Set a secret.

```json
{
  "name": "secret_set",
  "params": {
    "name": "MY_API_KEY",
    "value": "sk-..."
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `name` | string | Yes | Secret name |
| `value` | string | Yes | Secret value |

---

## LLM Tools

### llm_chat

Call LLM.

```json
{
  "name": "llm_chat",
  "params": {
    "model": "minimax/ail-compound",
    "messages": [
      {"role": "user", "content": "Hello"}
    ]
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `model` | string | No | Model (default: configured) |
| `messages` | array | Yes | Message array |
| `temperature` | number | No | Temperature |
| `max_tokens` | number | No | Max tokens |

---

### llm_embed

Get embeddings.

```json
{
  "name": "llm_embed",
  "params": {
    "text": "Hello world",
    "model": "gemini-embedding-2"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `text` | string | Yes | Text to embed |
| `model` | string | No | Model (default: configured) |

---

### llm_models

List available models.

```json
{
  "name": "llm_models",
  "params": {}
}
```

---

### llm_usage

Get usage stats.

```json
{
  "name": "llm_usage",
  "params": {
    "days": 7
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `days` | number | No | Days back (default: 7) |

---

## System Tools

### system_health

Check system health.

```json
{
  "name": "system_health",
  "params": {}
}
```

**Returns:**
```json
{
  "status": "ok",
  "daemon": "running",
  "brain": "accessible",
  "plugins": 15,
  "version": "0.1.0"
}
```

---

### system_info

Get system info.

```json
{
  "name": "system_info",
  "params": {}
}
```

---

### system_logs

Get logs.

```json
{
  "name": "system_logs",
  "params": {
    "lines": 50,
    "filter": "error"
  }
}
```

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `lines` | number | No | Lines (default: 50) |
| `filter` | string | No | Level filter |

---

### system_restart

Restart daemon.

```json
{
  "name": "system_restart",
  "params": {}
}
```

---

### system_version

Get version.

```json
{
  "name": "system_version",
  "params": {}
}
```

---

## Error Responses

```json
{
  "error": {
    "code": -32001,
    "message": "capability denied: brain/read",
    "data": {
      "required": "brain/read",
      "granted": []
    }
  }
}
```

### Error Codes

| Code | Meaning |
|------|---------|
| -32000 | Internal error |
| -32001 | Capability denied |
| -32002 | Resource not found |
| -32003 | Invalid parameters |
| -32004 | Rate limited |
| -32005 | Not implemented |

## Examples

### Complete Workflow

```javascript
// 1. Check system health
const health = await mcp.call({ name: "system_health" });

// 2. Search brain
const results = await mcp.call({
  name: "brain_search",
  params: { query: "polymarket", limit: 10 }
});

// 3. Read relevant page
const page = await mcp.call({
  name: "brain_read",
  params: { path: results[0].path }
});

// 4. Ask LLM about it
const answer = await mcp.call({
  name: "superbrain_query",
  params: { question: "What does this tell me?" }
});
```

## See Also

- [Concepts Overview](../concepts/index.md) — MCP in context
- [Plugin Guide](../plugins/index.md) — Plugin MCP tools
- [Installation Guide](../getting-started.md) — MCP setup
