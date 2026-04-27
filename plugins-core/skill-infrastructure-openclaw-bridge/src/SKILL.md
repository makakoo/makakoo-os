---
name: openclaw-bridge
description: Harvey OS delegation bridge to OpenClaw. Routes tasks to OpenClaw as a sub-agent, manages sessions, synthesizes results, and maintains an audit log. OpenClaw is invoked via CLI subprocess (primary) or MCP (if OpenClaw exposes an MCP server). Part of Harvey's multi-agent orchestration system.
version: 0.1.0
author: Harvey OS
metadata:
  harvey:
    tags: [orchestration, multi-agent, openclaw, delegation, subprocess]
    related_skills: [orchestrator, native-mcp, openclaw-migration]
    phase: research-complete
---

# OpenClaw Bridge Skill

## Overview

The OpenClaw Bridge is Harvey's delegation channel to OpenClaw — Harvey decides when and how to route tasks to OpenClaw as a sub-agent, then synthesizes the results.

**This is not a flat tool integration.** It's a capability-aware routing layer with bidirectional communication, session persistence, result synthesis, and error recovery.

## Requirements

- **OpenClaw CLI** installed and on PATH (`openclaw` command)
- **OpenClaw configured** with at least one agent and authenticated model provider
- **Python 3.11+** (Harvey's runtime)
- **jq** for JSON parsing (`brew install jq`)

## Data Volume

All bridge state lives in:
```
$HARVEY_HOME/data/openclaw-bridge/
├── state.json           # Bridge state, session ID cache
├── sessions/            # Per-session context
├── logs/                # Delegation audit logs
└── capabilities.json    # OpenClaw capability registry
```

## Quick Start

### Check if OpenClaw is available

```bash
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/openclaw-bridge/bridge.py status
```

### Run a delegation (from Harvey, not manually)

Harvey executes this internally when routing to OpenClaw:

```bash
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/openclaw-bridge/bridge.py \
  delegate \
  --prompt "Summarize the Discord #engineering channel from yesterday" \
  --thinking medium \
  --timeout 300
```

### Session Management

```bash
# List active sessions
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/openclaw-bridge/bridge.py sessions list

# Kill a session
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/openclaw-bridge/bridge.py sessions kill --session-id <uuid>

# Reset all sessions (fresh start)
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/openclaw-bridge/bridge.py sessions reset
```

### Capability Discovery

```bash
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/openclaw-bridge/bridge.py capabilities
```

## Architecture

```
Sebastian → Harvey → Bridge Skill → OpenClaw CLI (subprocess)
                    ↓
              Session Manager
              (data/openclaw-bridge/sessions/)
                    ↓
              Result Synthesizer
                    ↓
              Harvey → Sebastian
                    ↓
              Log to Brain (data/Brain/journals/YYYY_MM_DD.md)
```

## Routing Triggers

Harvey delegates to OpenClaw when:

| Trigger | Example |
|---------|---------|
| `explicit` | "ask OpenClaw to..." |
| `channel_action` | Discord/Telegram/Slack message |
| `browser_automation` | "take a screenshot of..." |
| `openclaw_skill` | Uses a skill OpenClaw has installed |
| `openclaw_memory` | "based on what OpenClaw knows about..." |

Harvey makes this decision autonomously in the `bridge.py route()` method.

## Delegation Prompt Format

Harvey wraps the task before sending to OpenClaw:

```
## Task from Harvey OS (Sebastian's cognitive extension)

### Your role
You are OpenClaw, operating as a sub-agent of Harvey OS.

### Sebastian's request
{prompt}

### Harvey's context
- Active project: {project}
- Recent: {recent_brain_entries}
- Expected output: {format}

### Instructions
- Execute autonomously.
- Report results clearly.
- Log significant findings to memory.

## End of delegation
```

## Result Synthesis Rules

| OpenClaw Output | Harvey Action |
|-----------------|--------------|
| Plain text | Present directly |
| Structured JSON | Format as markdown |
| Error | Log + graceful failure |
| Tool list | Summarize actions done |
| Screenshot | Describe + offer to show |

## Error Recovery

| Error | Recovery |
|-------|----------|
| OpenClaw not installed | Return "not available" + install instructions |
| Timeout | Retry once, 2x timeout |
| Rate limit | Back off 30s, retry max 2x |
| Invalid session | Create new, warn context lost |
| Permission error | Report to Sebastian |

## Audit Log

Every delegation is logged to:
`data/openclaw-bridge/logs/YYYY_MM_DD.jsonl`

```json
{
  "timestamp": "2026-03-28T14:32:01Z",
  "task": "Summarize Discord #engineering",
  "session_id": "oc-sess-abc123",
  "trigger": "channel_action",
  "duration_ms": 45230,
  "result_type": "text",
  "harvey_synthesis": "...",
  "error": null
}
```

And a summary is written to today's Brain journal.

## Files

```
harvey-os/skills/infrastructure/openclaw-bridge/
├── bridge.py              # Main: routing, session, synthesis
├── cli_executor.py        # CLI subprocess executor
├── session_manager.py     # Session lifecycle
├── result_synthesizer.py  # Output parsing + formatting
├── prompts/
│   └── delegation.md      # Prompt templates
└── SKILL.md               # This file
```

## Configuration

Bridge configuration in `data/openclaw-bridge/config.json`:

```json
{
  "openclaw_path": "openclaw",
  "default_thinking": "medium",
  "default_timeout": 300,
  "session_idle_ttl": 1800,
  "max_retries": 2,
  "gateway_url": "http://localhost:8080",
  "preferred_mode": "cli"
}
```

## Troubleshooting

### "openclaw: command not found"
OpenClaw is not installed. Install: `curl -s https://openclaw.ai/install.sh | sh`

### "No agent configured"
Run `openclaw onboard` or `openclaw agents add`

### "Session not found"
The session may have expired. Bridge will create a new one automatically.

### Bridge hangs on delegation
Check if OpenClaw daemon is stuck: `openclaw status`. Kill gateway if needed: `openclaw gateway stop`

## Status

**Phase:** Research Complete — Ready for implementation
**Files created:** `openclawBridge/research/01-landscape-analysis.md`, `openclawBridge/specs/SPEC.md`, `openclawBridge/audits/01-technical-audit.md`
