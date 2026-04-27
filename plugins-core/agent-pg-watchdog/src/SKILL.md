---
name: pg-watchdog
description: PostgreSQL cluster watchdog — monitors all PG clusters via pg_lsclusters every 15 min. Auto-restarts downed clusters, uses AI diagnosis via switchAILocal if restart fails.
version: 1.0.0
tags: [postgresql, monitoring, watchdog, infrastructure]
---

# PostgreSQL Cluster Watchdog

Runs every 15 min via cron. Checks all PostgreSQL clusters.

## What It Does

1. Runs `pg_lsclusters` — checks status of all clusters
2. If any are down → tries `pg_ctlcluster <ver> <name> start` (up to 2 attempts)
3. If restart fails → reads PG log tail, sends to switchAILocal AI for diagnosis
4. AI returns diagnosis + fix command → validates safety → executes
5. Logs to `data/logs/pg_watchdog.log` + Logseq Brain journal

## How to Run

```bash
# Manual run
python3 pg_watchdog.py

# Cron (every 15 min)
*/15 * * * * python3 /path/to/agents/pg-watchdog/pg_watchdog.py
```

## Safety

- Only `pg_ctlcluster` commands allowed from AI
- No pipes, semicolons, backticks (prevents injection)
- Cluster version+name must match
