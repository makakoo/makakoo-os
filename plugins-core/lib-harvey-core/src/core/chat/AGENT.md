---
name: harveychat
description: Harvey's external messaging gateway — Telegram, WhatsApp, Discord
type: body-adapter
status: active
---

# HarveyChat Agent

Harvey's messaging body — routes external messages through Harvey Agent Core to any channel (Telegram, WhatsApp, Discord, etc.).

## Architecture
- Uses `core/agent/harvey_agent.py` for the agentic tool-calling loop
- Channels in `core/chat/channels/` handle transport (Telegram polling, etc.)
- Gateway in `core/chat/gateway.py` coordinates everything
- Bridge in `core/chat/bridge.py` wraps the agent core with channel-specific config

## Tools Available
- `brain_search` — Search Brain knowledge graph via Superbrain
- `brain_write` — Write to today's journal
- `skill_discover` — Find relevant skills by semantic search
- `get_emails` — Fetch recent Gmail via gws CLI
- `get_calendar` — Today's calendar events via gws CLI
- `superbrain_status` — Brain health check
- `run_command` — Execute safe read-only shell commands (crontab, launchctl, ps, ls, git status, etc.)
- `browse_url` — Fetch and summarize any public web page
- `set_telegram_profile_photo` — Set the bot's Telegram profile photo
- `generate_image` — Generate images using MiniMax image-01 via switchAILocal (saves to file)

## File Sending (via markers)
Harvey can send files and photos via Telegram using special markers in his response:
- `[[SEND_FILE:/path/to/file.pdf]]` — sends a file as Telegram document
- `[[SEND_PHOTO:/path/to/image.png]]` — sends an image as Telegram photo

The gateway parses these markers after the LLM responds, sends the files, then removes markers from the displayed text.

## Audio Support
- **Voice input**: Downloads Telegram voice messages, converts with ffmpeg, transcribes with faster-whisper (local)
- **Voice output**: Generates audio via macOS `say` command, sends as voice notes to Telegram
- Requires: `faster-whisper` (pip), `ffmpeg`, `say` (macOS built-in)

## Media Attachments
- **Photos**: Downloaded, base64-encoded, sent to MiniMax image-01 vision model → description + analysis
- **Documents/PDFs**: Downloaded, text extracted via pdftotext/PyPDF2/OCR, fed to LLM for summary
- **Videos**: Acknowledged with placeholder (future: frame extraction + vision)
- Requires: `ffmpeg`, `tesseract` (for OCR), `pdftotext`/`PyPDF2` (for PDFs), `python-docx` (for Word docs)

## Setup

### 1. Install dependencies
```bash
pip install faster-whisper python-telegram-bot
brew install ffmpeg
```

### 2. Configure Telegram bot
1. Create bot via @BotFather on Telegram — get bot token
2. Get your user ID via @userinfobot
3. Create config:
```bash
mkdir -p ~/MAKAKOO/data/chat
cat > ~/MAKAKOO/data/chat/config.json << 'EOF'
{
  "telegram": {
    "bot_token": "YOUR_BOT_TOKEN",
    "allowed_user_ids": [YOUR_USER_ID],
    "polling_timeout": 30
  },
  "bridge": {
    "switchai_url": "http://localhost:18080/v1",
    "switchai_model": "minimax:MiniMax-M2.7",
    "switchai_api_key": "YOUR_SWITCHAI_KEY",
    "max_history_messages": 20,
    "max_tokens": 4096
  },
  "log_to_brain": true
}
EOF
```

### 3. Start HarveyChat
```bash
cd ~/MAKAKOO/harvey-os
python3 -m core.chat start --daemon
```

## Commands (in Telegram)
- `/start` — Welcome message
- `/status` — System status and stats
- `/clear` — Clear conversation context

## Fallback Chain
1. HarveyAgent via switchAILocal (full tool-calling loop, 5 rounds max)
2. Direct switchAILocal (prompt-based, auto-fetches Brain/emails context)
3. Generic offline message

## Safe Shell Commands (via run_command tool)
Allowed read-only commands:
- System: `ps aux`, `uptime`, `who`, `hostname`, `df -h`, `free`, `top`
- macOS: `launchctl list`, `pmset -g`, `caffeinate -u`
- Cron: `crontab -l`
- Git: `git status`, `git log --oneline -5`, `git branch`
- Health: `curl -s http://localhost:18080/health`
- Files: `ls -la`, `pwd`
