---
name: harveychat
description: Harvey's external messaging gateway — Telegram, WhatsApp, Discord
type: body-adapter
status: active
---

# HarveyChat Agent

Harvey's messaging body — routes external messages through Harvey Agent Core.

## Architecture
- Uses `core/agent/harvey_agent.py` for the agentic tool-calling loop
- Channels in `core/chat/channels/` handle transport (Telegram polling, etc.)
- Gateway in `core/chat/gateway.py` coordinates everything
- Bridge in `core/chat/bridge.py` wraps the agent core with channel-specific config

## Tools Available
- `brain_search` — Search Logseq knowledge graph via Superbrain
- `brain_write` — Write to today's journal
- `skill_discover` — Find relevant skills by semantic search
- `get_emails` — Fetch recent Gmail via gws CLI
- `get_calendar` — Today's calendar events via gws CLI
- `superbrain_status` — Brain health check

## Setup
```
cd ~/MAKAKOO/harvey-os && python3 -m core.chat setup
```

## Run
```
python3 -m core.chat start --daemon
```

## Fallback Chain
1. switchAILocal with tool-calling (full agentic loop)
2. switchAILocal without tools (prompt-based context injection)
3. Direct Anthropic API (no tools, plain LLM)
