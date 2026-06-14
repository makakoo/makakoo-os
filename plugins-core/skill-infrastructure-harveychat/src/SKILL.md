# HarveyChat — External Communication Gateway

**Category:** infrastructure
**Status:** active
**Core Tool:** #3 (alongside Brain + switchAILocal)

## What

External messaging gateway that lets Sebastian talk to the configured Makakoo
persona from any device via Telegram. Fully agentic: the bot has Brain search,
Brain write, skill discovery, file/report helpers, safe diagnostics, browser
read gates, email/calendar hooks, image/video/document understanding, and
Cortex long-term chat memory.

## Architecture

```
Phone/Desktop → Telegram Bot → HarveyChat Gateway → switchAILocal → LLM/tools
                                     ↕                 ↕
                       SQLite Store + Cortex      Brain search/write
```

## Components

| Module | Path | Purpose |
|--------|------|---------|
| Gateway | `core/chat/gateway.py` | Central message router |
| Bridge | `core/chat/bridge.py` | LLM backend with tool-calling |
| Agent | `core/agent/harvey_agent.py` | Tool-calling loop (7 tools) |
| Store | `core/chat/store.py` | SQLite conversation persistence |
| Brain Sync | `core/chat/brain_sync.py` | Logs significant chats to Brain |
| Config | `core/chat/config.py` | Settings from file + env vars |
| Telegram | `core/chat/channels/telegram/channel.py` | Telegram bot (async polling) |
| Audio | `core/chat/audio.py` | Voice transcription + TTS output |
| Media | `core/chat/media.py` | Image vision + document extraction |
| CLI | `core/chat/__main__.py` | start/stop/status/setup commands |

## Tools Available (Agentic Loop)

| Tool | What it does |
|------|-------------|
| `brain_search` | Search Brain knowledge graph |
| `brain_write` | Write to today's journal |
| `skill_discover` | Find relevant Harvey skills |
| `get_emails` | Fetch recent Gmail messages |
| `get_calendar` | Today's calendar events |
| `superbrain_status` | Brain health check |
| `run_command` | Execute safe read-only shell commands |

### Safe Shell Commands (whitelist only)

Allowed: `ps aux`, `uptime`, `who`, `hostname`, `df -h`, `free`, `top`,
`launchctl list`, `crontab -l`, `git status`, `git log --oneline -5`

**Blocked:** no writes, no sudo, no network, no git push, no package installs

## Capabilities by Attachment Type

| Type | Processing |
|------|-----------|
| Voice message | ffmpeg convert → faster-whisper (local) → transcribed |
| Photo | Downloaded → base64 → MiniMax image-01 → vision description |
| PDF | pdftotext/pypdf → text extracted → LLM summary |
| Word doc | python-docx → text extracted → LLM summary |
| Plain text | .txt, .md, .py, .json, etc. → directly read and analyzed |
| Video | Acknowledged (future: frame extraction) |

## Quick Start

```bash
# 1. Install the plugin. This creates its own .venv.
makakoo plugin install --core agent-harveychat

# 2. Configure secrets/env.
makakoo secret set telegram.bot_token
export TELEGRAM_BOT_TOKEN=...
export TELEGRAM_ALLOWED_USERS=746496145
export SWITCHAI_KEY=...
export SWITCHAI_MODEL=ail-compound

# 3. Start.
cd ~/MAKAKOO/plugins/agent-harveychat
.venv/bin/python -u src/agent.py start --daemon

# Check status
.venv/bin/python -u src/agent.py status
```

## Configuration

Environment variables (override config file):
- `TELEGRAM_BOT_TOKEN` — Bot token from @BotFather
- `TELEGRAM_ALLOWED_USERS` — Comma-separated Telegram user IDs (empty = allow all)
- `SWITCHAI_KEY` — switchAILocal API key
- `SWITCHAI_MODEL` — model alias (VPS default: `ail-compound`)
- `HARVEYCHAT_WORKFLOWS` — set `1` to opt into experimental background research/image/archive workflows; default `0` keeps Telegram conversational.

## Data

- Conversations DB: `~/MAKAKOO/data/chat/conversations.db`
- Config: `~/MAKAKOO/data/chat/config.json`
- Logs: `~/MAKAKOO/data/chat/harveychat.log`
- PID: `~/MAKAKOO/data/chat/harveychat.pid`
- Voice output: `~/MAKAKOO/data/chat/voice/`

## Dependencies

### Python (pip install)
```
python-telegram-bot==22.5
requests==2.32.5
httpx==0.28.1
pypdf==6.13.2
python-docx==1.2.0
```

### System (brew install)
```
ffmpeg tesseract poppler
```

## Cortex Memory

HarveyChat supports native Cortex Memory in `core/cortex/`. When
`cortex.enabled` is true, the gateway extracts PII-scrubbed durable memories
into local SQLite, retrieves relevant memories before each LLM turn, and
injects them as bounded context. `/status` reports `Cortex Memory:
enabled|disabled`.

Config lives in `~/MAKAKOO/data/chat/config.json` under the `cortex` key. Full manual: `docs/agents/harveychat-cortex-memory.md`.

Recommended long-context bot settings:

```json
{
  "bridge": {"max_history_messages": 80, "max_tokens": 8192, "switchai_model": "ail-compound"},
  "cortex": {"enabled": true, "memory_limit": 12, "max_memory_chars": 1000, "max_prompt_memory_chars": 8000}
}
```

## Security

- `allowed_user_ids` restricts who can talk to Harvey (recommended)
- Bot token stored in config.json or env var — never in code
- Shell commands whitelist-only (no writes, no network, no git push)
- All messages go through switchAILocal (local-first, no telemetry)
- Conversation history is local SQLite only

## Extending Channels

Add a new channel by implementing `BaseChannel` in `core/chat/channels/`:

```python
from core.chat.channels.base import BaseChannel

class SlackChannel(BaseChannel):
    @property
    def name(self) -> str:
        return "slack"

    async def start(self, on_message):
        ...

    async def stop(self):
        ...

    async def send(self, user_id, text):
        ...
```

Register it in `gateway.py`'s `__init__`.
