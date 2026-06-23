# Makakoo OS Concepts

Understand how Makakoo OS works.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        YOU                                   │
│                   (terminal / chat)                           │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│                      AI CLI                                   │
│           (Claude Code / Gemini / OpenCode / etc.)            │
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │  INFECTION LAYER                                         │ │
│  │  Reads: AGENTS.md, Brain journals, Skills                │ │
│  └─────────────────────────────────────────────────────────┘ │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│                    MAKAKOO MCP                               │
│                   (stdio JSON-RPC)                           │
│                                                               │
│  Tools: brain_query, superbrain_search, agent_status, etc.  │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│                   MAKAKOO CORE                               │
│                    (Rust Kernel)                             │
│                                                               │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────────┐  │
│  │ SANCHO   │ │ Superbrain│ │  Brain   │ │ Capabilities │  │
│  │ (tasks)  │ │ (search) │ │ (memory) │ │  (sandbox)   │  │
│  └──────────┘ └──────────┘ └──────────┘ └──────────────┘  │
└────────────────────────┬────────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│                     PLUGINS                                  │
│            (Agents / Skills / Tasks)                        │
│                                                               │
│  arbitrage │ career-manager │ harveychat │ monitoring       │
└─────────────────────────────────────────────────────────────┘
```

## Core Concepts

### 1. Infection

Makakoo "infects" AI CLIs by adding a bootstrap block to their configuration. This block:

- Points to the shared `AGENTS.md` (system prompt)
- Enables MCP tools via `mcp.json`
- Configures environment variables

**Infected CLIs:** Claude Code, Gemini CLI, OpenCode, Vibe, Qwen, Cursor, Copilot, JetBrains AI

### 2. Brain

The Brain isMakakoo's persistent memory system:

```
~/MAKAKOO/data/Brain/
├── journals/           # Daily logs
│   ├── 2026_04_20.md   # "Today I did X..."
│   └── 2026_04_19.md
├── pages/              # Structured pages
│   ├── projects/       # Project notes
│   ├── decisions/      # Decisions made
│   └── people/         # Contact info
└── superbrain.db      # Vector embeddings
```

**Journals:** Daily entries, auto-created, written to throughout the day
**Pages:** Structured documents, manually created, persistent

### 3. Superbrain

Superbrain provides intelligent search:

| Layer | Technology | Purpose |
|-------|------------|---------|
| Full-text | FTS5 | Keyword search |
| Semantic | Vectors | Meaning search |
| LLM | Synthesis | Natural language answers |

### 4. SANCHO

SANCHO runs proactive tasks automatically:

- **Dream:** LLM reflection on recent decisions
- **Daily Briefing:** Status summary
- **Memory Consolidation:** Optimize memory
- **Wiki Lint:** Brain hygiene
- **Plugin Tasks:** Custom scheduled work

Tasks run based on:
- Time intervals (every 5m, hourly, daily)
- Active hours (e.g., 7am-10pm)
- User idle status
- Screen lock status

### 5. Plugins

Plugins extend Makakoo with new capabilities:

| Type | Example | Purpose |
|------|---------|---------|
| `agent` | arbitrage-agent | Autonomous workers |
| `skill` | skill-research | Reusable prompts |
| `sancho-task` | watchdog | Scheduled jobs |
| `mcp-tool` | custom-server | MCP integration |

### 6. Capabilities

Capabilities sandbox what plugins can do:

```toml
[capabilities]
grants = [
  "brain/read",                    # Read memory
  "net/http:https://api.example/*", # Network access
  "secrets/read:API_KEY",         # Specific secrets
]
```

This prevents plugins from accessing data they shouldn't.

---

## Data Flow

### Writing to Brain

```
User: "Remember I chose PostgreSQL"
    ↓
Claude Code (infected)
    ↓
makakoo_mcp tool: brain_write
    ↓
makakoo-core: append to journal
    ↓
~/MAKAKOO/data/Brain/journals/2026_04_20.md
```

### Querying Brain

```
User: "What database did I choose?"
    ↓
Claude Code (infected)
    ↓
makakoo_mcp tool: superbrain_query
    ↓
makakoo-core:
  1. FTS5 search "database"
  2. Vector search "database choice"
  3. LLM synthesis
    ↓
Answer with citations
```

### Proactive Task

```
SANCHO timer fires (every hour)
    ↓
Check gates (time, active_hours, idle)
    ↓
Gates pass → Run task
    ↓
Task executes (e.g., gym_classify)
    ↓
Result → Brain journal + optional notification
```

---

## Key Design Principles

1. **Local-first:** All data stays on your machine
2. **No telemetry:** Nothing sent to external servers
3. **Capability sandboxing:** Plugins only access what they need
4. **Progressive enhancement:** Tasks run automatically, don't require interaction
5. **Multi-CLI:** Every CLI shares the same memory
