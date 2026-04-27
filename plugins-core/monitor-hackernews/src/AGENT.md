---
name: hackernews
description: Hacker News monitor — fetches top stories hourly, notifies new ones via Telegram and email
type: agent
status: active
---

# Hacker News Monitor Agent

Fetches top HN stories hourly. Sends new ones to Telegram and/or email.

## Setup
1. Configure Telegram bot token in `data/chat/config.json`
2. Run: `python3 agents/hackernews/hn_monitor.py`
3. Or set up hourly cron: `python3 agents/hackernews/hn_monitor.py --daemon`

## How It Works
- Fetches top 30 stories from HN API every hour
- Compares against seen index (`data/hackernews/seen.json`)
- New stories → sends to Telegram + email
- Index persists across restarts
