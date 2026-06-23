# Architecture Guide

Detailed system architecture of Makakoo OS.

## System Overview

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              USER                                               │
│                    (terminal / chat / API)                                        │
│                                                                                  │
│    ┌──────────────────────────────────────────────────────────────────────┐    │
│    │  Claude Code  │  Gemini CLI  │  OpenCode  │  Vibe  │  Your App  │    │
│    └──────────────────────────────────────────────────────────────────────┘    │
└────────────────────────────────────┬───────────────────────────────────────────┘
                                     │ infection
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                         INFECTION LAYER                                          │
│                                                                                  │
│   ┌─────────────────────────────────────────────────────────────────────────┐   │
│   │  AGENTS.md (system prompt) + MCP tools + Environment Variables            │   │
│   └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                  │
│   Injects into: ~/.claude/, ~/.gemini/, ~/.opencode/, ~/.vibe/, ~/.qwen/       │
└────────────────────────────────────┬───────────────────────────────────────────┘
                                     │
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        MAKAKOO MCP SERVER                                        │
│                              (stdio JSON-RPC)                                     │
│                                                                                  │
│   ┌─────────────────────────────────────────────────────────────────────────┐   │
│   │  40+ Tools: brain_query, superbrain_search, plugin_*, secret_*, etc.  │   │
│   └─────────────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────┬───────────────────────────────────────────┘
                                     │ Unix socket / Named pipe
                                     ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                         MAKAKOO CORE (Rust)                                      │
│                                                                                  │
│   ┌────────────────┐  ┌────────────────┐  ┌────────────────┐  ┌────────────────┐│
│   │    SANCHO      │  │   Superbrain   │  │     Brain     │  │ Capabilities   ││
│   │   Scheduler    │  │   (FTS+Vec)    │  │  (Journals)    │  │   (Sandbox)    ││
│   └────────────────┘  └────────────────┘  └────────────────┘  └────────────────┘│
│                                                                                  │
│   ┌────────────────┐  ┌────────────────┐  ┌────────────────┐                    │
│   │  Event Bus     │  │    Config      │  │   Platform     │                    │
│   │  (Internal)    │  │   (TOML)       │  │ (macOS/Linux/  │                    │
│   └────────────────┘  └────────────────┘  │   Windows)      │                    │
│                                        └────────────────┘                    │
└────────────────────────────────────┬───────────────────────────────────────────┘
                                     │
              ┌──────────────────────┴──────────────────────┐
              ▼                                              ▼
┌─────────────────────────────┐          ┌─────────────────────────────────────┐
│         PLUGINS              │          │           DAEMON                   │
│                              │          │                                     │
│  ┌─────────┐ ┌───────────┐ │          │  ┌─────────────────────────────────┐│
│  │arbitrage│ │career-   │ │          │  │  Process Manager                 ││
│  │-agent   │ │manager   │ │          │  │  Health Monitor                   ││
│  └─────────┘ └───────────┘ │          │  │  Auto-restart                    ││
│                              │          │  │  Log rotation                    ││
│  ┌─────────┐ ┌───────────┐ │          │  └─────────────────────────────────┘│
│  │harvey-  │ │ watchdog- │ │          │                                     │
│  │chat     │ │postgres   │ │          │  ┌─────────────────────────────────┐│
│  └─────────┘ └───────────┘ │          │  │  launchd / systemd / Windows     ││
│                              │          │  │  (auto-start on boot)           ││
│  Each plugin in a sandbox:  │          │  └─────────────────────────────────┘│
│  - Unix socket per plugin    │          │                                     │
│  - PID verification          │          └─────────────────────────────────────┘
│  - Capability grants         │                                                      
└─────────────────────────────┘                                                      
```

## Data Flow Diagrams

### Query Flow

```
User: "What did I decide about the database?"
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│                      Claude Code                             │
│  Infected with AGENTS.md, sees Makakoo tools               │
└────────────────────────────┬────────────────────────────────┘
                             │
                             │ makakoo_brain_query
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                    makakoo-mcp                              │
│  JSON-RPC over stdio → Unix socket                         │
└────────────────────────────┬────────────────────────────────┘
                             │
                             │ IPC
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                   makakoo-core                               │
│                                                              │
│  ┌───────────────┐    ┌───────────────┐                   │
│  │ Superbrain    │    │ Brain Reader  │                   │
│  │ (FTS5+Vec)   │    │               │                   │
│  └───────┬───────┘    └───────┬───────┘                   │
│          │                    │                              │
│          ▼                    ▼                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              LLM Gateway (switchAILocal)              │  │
│  │              localhost:18080                           │  │
│  └──────────────────────────┬───────────────────────────┘  │
└─────────────────────────────┼────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    LLM Gateway                              │
│               (AIL / Claude / Gemini)                       │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│  Response: "Based on your journals from April 20th..."      │
│  Citations: Brain/journals/2026_04_20.md                  │
└─────────────────────────────────────────────────────────────┘
```

### Write Flow

```
User: "Remember I chose PostgreSQL"
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│                      Claude Code                             │
└────────────────────────────┬────────────────────────────────┘
                             │
                             │ makakoo_brain_write
                             ▼
┌─────────────────────────────────────────────────────────────┐
│                   makakoo-core                               │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Brain Writer                                         │  │
│  │  ~/MAKAKOO/data/Brain/journals/2026_04_20.md              │  │
│  └──────────────────────────────────────────────────────┘  │
│                          │                                  │
│                          ▼                                  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  File System                                          │  │
│  │  Appends: "- [[2026-04-20 14:32:00]] Chose PostgreSQL"│  │
│  └──────────────────────────────────────────────────────┘  │
│                          │                                  │
│                          ▼                                  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Superbrain Sync (async)                               │  │
│  │  - Generate vector embedding                           │  │
│  │  - Update FTS5 index                                   │  │
│  │  - Store in vector DB                                  │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Proactive Task Flow

```
┌─────────────────────────────────────────────────────────────┐
│                    SANCHO Scheduler                         │
│                                                              │
│  Timer fires (hourly)                                        │
│         │                                                   │
│         ▼                                                    │
│  ┌──────────────┐                                           │
│  │ Check Gates  │                                           │
│  │  - Time     │                                           │
│  │  - Hours    │                                           │
│  │  - Idle     │                                           │
│  │  - Lock     │                                           │
│  └──────┬───────┘                                           │
│         │ Gates pass?                                        │
│         ▼                                                    │
│     ┌───────┐                                               │
│     │ Run   │ ──▶ Task output → Brain journal               │
│     │ Task  │                                               │
│     └───────┘                                               │
└─────────────────────────────────────────────────────────────┘
```

## Component Details

### Infection Layer

```
┌─────────────────────────────────────────────────────────────┐
│                     INFECTION LAYER                         │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  AGENTS.md / CLAUDE.md / GEMINI.md                  │   │
│  │  ┌────────────────────────────────────────────────┐ │   │
│  │  │  # Makakoo OS Bootstrap                        │ │   │
│  │  │                                                 │ │   │
│  │  │  You are Harvey, Sebastian's AI assistant...    │ │   │
│  │  │                                                 │ │   │
│  │  │  ## Tools                                     │ │   │
│  │  │  Available via MCP:                            │ │   │
│  │  │  - makakoo_brain_query                        │ │   │
│  │  │  - makakoo_superbrain_search                  │ │   │
│  │  │  - makakoo_sancho_status                      │ │   │
│  │  │                                                 │ │   │
│  │  │  ## Brain Location                            │ │   │
│  │  │  ~/MAKAKOO/data/Brain/                             │ │   │
│  │  └────────────────────────────────────────────────┘ │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
│  Targets:                                                   │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐           │
│  │Claude   │ │Gemini   │ │OpenCode │ │ Vibe    │           │
│  │~/.claude│ │~/.gemini│ │~/.open  │ │~/.vibe/ │           │
│  └─────────┘ └─────────┘ └─────────┘ └─────────┘           │
└─────────────────────────────────────────────────────────────┘
```

### Capability Sandbox

```
┌─────────────────────────────────────────────────────────────┐
│                    CAPABILITY SANDBOX                       │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │ Plugin: arbitrage                                     │  │
│  │ manifest.toml:                                        │  │
│  │   grants = [                                          │  │
│  │     "brain/read",                                     │  │
│  │     "net/http:https://clob.polymarket.com/*",         │  │
│  │     "secrets/read:POLYMARKET_API_KEY"                 │  │
│  │   ]                                                   │  │
│  └──────────────────────────────────────────────────────┘  │
│                            │                                │
│                            ▼                                │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Unix Socket ($MAKAKOO_HOME/              │  │
│  │              run/plugins/arbitrage.sock)              │  │
│  │                                                      │  │
│  │   Plugin ──▶ Request ──▶ Kernel checks grants         │  │
│  │                              │                        │  │
│  │                    ┌──────────┴──────────┐             │  │
│  │                    │                     │             │  │
│  │                   ✓ ALLOW              ✗ DENY         │  │
│  │                    │                     │             │  │
│  │                    ▼                     ▼             │  │
│  │              Execute              Error + Log         │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                              │
│  Audit Log: ~/.makakoo/logs/audit.jsonl                     │
│  {"ts":"...", "plugin":"arbitrage", "verb":"net/http",    │
│   "result":"allowed", ...}                                   │
└─────────────────────────────────────────────────────────────┘
```

### Plugin Lifecycle

```
┌─────────────────────────────────────────────────────────────┐
│                    PLUGIN LIFECYCLE                         │
│                                                              │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐  │
│  │ Install │───▶│ Start  │───▶│ Running │───▶│ Health  │  │
│  └─────────┘    └────┬────┘    └────┬────┘    └────┬────┘  │
│                      │              │              │        │
│                      ▼              ▼              ▼        │
│                 ┌─────────┐   ┌─────────┐   ┌─────────┐   │
│                 │ parse   │   │ fork/   │   │ periodic│   │
│                 │ manifest│   │ supervise│   │ checks │   │
│                 └─────────┘   └─────────┘   └─────────┘   │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │                  Daemon Manager                      │   │
│  │  - Spawns plugin process                            │   │
│  │  - Monitors PID                                     │   │
│  │  - Handles SIGCHLD                                 │   │
│  │  - Restarts on crash (exponential backoff)         │   │
│  │  - Quarantines after 5 crashes/15min               │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Network Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                            LOCAL MACHINE                                 │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │ 127.0.0.1                                                           │  │
│  │                                                                       │  │
│  │  ┌──────────────┐                                                    │  │
│  │  │ :18080       │ switchAILocal (LLM gateway)                        │  │
│  │  │ localhost    │ ┌─────────────────────────────────────────┐       │  │
│  │  └──────────────┘ │ Claude │ Gemini │ AIL │ Ollama │ ... │       │  │
│  │                   └─────────────────────────────────────────┘       │  │
│  │                                                                       │  │
│  │  ┌──────────────┐  ┌──────────────┐                               │  │
│  │  │ :6333        │  │ :5432        │                               │  │
│  │  │ Qdrant       │  │ PostgreSQL   │                               │  │
│  │  │ (vectors)    │  │              │                               │  │
│  │  └──────────────┘  └──────────────┘                               │  │
│  │                                                                       │  │
│  │  ┌──────────────────────────────────────────────────────────────┐  │  │
│  │  │ ~/.makakoo/run/plugins/<plugin>.sock (Unix domain socket)     │  │  │
│  │  └──────────────────────────────────────────────────────────────┘  │  │
│  │                                                                       │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  External connections (if needed):                                       │
│  ─────────────────────────────────────────────                           │
│  ┌──────────────┐                                                       │
│  │ api.polymarket.com    │ Arbitrage agent                              │
│  │ api.github.com       │ GitHub integration                            │
│  │ api.openai.com       │ LLM providers (via gateway)                    │
│  └──────────────────────┘                                               │
└─────────────────────────────────────────────────────────────────────────┘
```

## File Structure

```
~/.makakoo/                    # MAKAKOO_HOME
├── run/                       # Runtime
│   └── plugins/               # Plugin sockets
│       ├── arbitrage.sock
│       ├── harveychat.sock
│       └── ...
├── state/                    # Plugin state directories
│   ├── arbitrage/
│   ├── harveychat/
│   └── ...
├── plugins/                  # Installed plugins
│   ├── arbitrage/
│   ├── harveychat/
│   └── ...
├── distros/                  # User distros
├── config/                   # Configuration
│   ├── makakoo.toml
│   └── capability-overrides.toml
├── logs/                     # Logs
│   ├── daemon/
│   ├── sancho/
│   ├── audit.jsonl           # Capability audit
│   └── plugins/
└── cache/                    # Cache

~/MAKAKOO/                    # User data
├── Brain/                     # Memory
│   ├── journals/
│   ├── pages/
│   └── superbrain.db
├── skills/                    # User skills
├── agents/                    # Agent configs
├── data/                      # Agent data
│   ├── arbitrage-agent/
│   ├── career-manager/
│   └── ...
└── .env                      # Secrets (in home)
```

## Security Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         SECURITY LAYERS                                   │
│                                                                          │
│  Layer 1: Installation                                                  │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │ - Signed binaries (when available)                               │  │
│  │ - Checksum verification                                         │  │
│  │ - Homebrew/winget package integrity                            │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  Layer 2: Capability Sandbox                                           │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │ - Manifest-defined grants only                                   │  │
│  │ - Unix socket + PID verification                                │  │
│  │ - No arbitrary code execution                                   │  │
│  │ - Filesystem isolation                                          │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  Layer 3: Audit Logging                                                │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │ - Every capability call logged                                  │  │
│  │ - JSONL format with timestamps                                  │  │
│  │ - Retention: 100MB / 7 days                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  Layer 4: Secrets Management                                            │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │ - OS Keychain (Keychain/macOS, Secret Service/Linux)             │  │
│  │ - Never in environment variables for plugins                     │  │
│  │ - Explicit allowlist per plugin                                  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  Layer 5: Network Isolation                                            │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │ - URL-scoped HTTP grants                                         │  │
│  │ - No outbound by default                                        │  │
│  │ - Explicit plugin declarations                                  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

## See Also

- [Concepts Overview](./index.md) — High-level concepts
- [SANCHO Guide](./sancho.md) — Proactive tasks
- [Plugin Guide](../plugins/index.md) — Plugin architecture
- [Capabilities Reference](../spec/CAPABILITIES.md) — Security model
