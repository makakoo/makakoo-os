# Loops — Scheduled Task Management

**Skill #meta/loops | Owner:** Harvey

## Purpose

Central registry for all cron/scheduled jobs. Every automated loop lives here — no scattered crontab entries.

## Active Crontab

```cron
# switchAILocal Watchdog — every 5 min (✅ ACTIVE)
*/5 * * * * /usr/local/opt/python@3.11/bin/python3.11 -u $HARVEY_HOME/core/watchdogs/switchailocal_watchdog.py >> $HARVEY_HOME/data/logs/switchailocal-watchdog.log 2>&1

# Inbox Watchdog — every 30 min (✅ ACTIVE)
*/30 * * * * /usr/local/opt/python@3.11/bin/python3.11 -u $HARVEY_HOME/agents/career-manager/inbox_watchdog.py >> $HARVEY_HOME/data/logs/inbox_watchdog.log 2>&1

# Polymarket Intraday Trader v2 — paper trading, every 30 min (✅ ACTIVE, PAPER ONLY)
*/30 * * * * $HARVEY_HOME/agents/arbitrage-agent/run_intraday_trader.sh

# Polymarket Nightly Scan — 2 AM daily (✅ ACTIVE) — FIXED: missing `os` import
0 2 * * * /usr/local/opt/python@3.11/bin/python3.11 -u $HARVEY_HOME/agents/arbitrage-agent/nightly_scan.py >> $HARVEY_HOME/data/logs/nightly-polymarket-scan.log 2>&1

# BTC Signal Evolution — 10 PM nightly, 8h run (✅ ACTIVE)
# Only starts if not already running (pgrep guard)
0 22 * * * pgrep -f "btc_evolution.py" > /dev/null || (cd $HARVEY_HOME/agents/arbitrage-agent && /usr/local/opt/python@3.11/bin/python3.11 btc_evolution.py --hours 8 --population 20 --generations 999 --holding 240 --days 7 >> $HARVEY_HOME/data/arbitrage-agent/v2/logs/btc_evolution.log 2>&1)

# BTC Intraday Signaler v2 — every 10 min, restart if dead (✅ ACTIVE)
*/10 * * * * pgrep -f "intraday_signaler_v2.py" > /dev/null || (cd $HARVEY_HOME/agents/arbitrage-agent && nohup /usr/local/opt/python@3.11/bin/python3.11 -u intraday_signaler_v2.py >> $HARVEY_HOME/data/arbitrage-agent/v2/logs/intraday_signaler_v2.log 2>&1)

# Continuous Improve v3 — every 10 min, restart if dead (✅ ACTIVE)
*/10 * * * * pgrep -f "continuous_improve.py" > /dev/null || (cd $HARVEY_HOME/agents/arbitrage-agent && nohup /usr/local/opt/python@3.11/bin/python3.11 -u continuous_improve.py >> $HARVEY_HOME/data/arbitrage-agent/v2/logs/auto_improve.log 2>&1)

# Polymarket Real-Data Evolution — 3 AM daily (✅ ACTIVE)
# POP_SIZE=24, 10min/genome → ~4h per run, virtual money + real Polymarket data
0 3 * * * pgrep -f "autoimprove_live.py" > /dev/null || (cd $HARVEY_HOME/agents/arbitrage-agent && POP_SIZE=24 SESSION_MINUTES=10 GENERATIONS_PER_RUN=99 /usr/local/opt/python@3.11/bin/python3.11 autoimprove_live.py >> $HARVEY_HOME/data/arbitrage-agent/v2/logs/autoimprove_live.log 2>&1)

# Trading P&L Log to Brain — every 15 min (✅ ACTIVE)
*/15 * * * * /usr/local/opt/python@3.11/bin/python3.11 -u $HARVEY_HOME/agents/arbitrage-agent/log_trading_pnl.py

# Auto-Improver — every 1hr via launchd (com.harvey.autoimprover.plist)
# Status: launchctl list | grep autoimprov
# Logs: $HARVEY_HOME/tmp/autoimprover.log

# Auto-Improver Nightly Run — 4 AM daily (✅ ACTIVE)
0 4 * * * /usr/local/opt/python@3.11/bin/python3.11 -u $HARVEY_HOME/harvey-os/skills/meta/autoimprover/run_improvements.py >> $HARVEY_HOME/data/logs/autoimprover.log 2>&1
```

## Adding a New Cron Job

1. Add the cron entry above in the Active Crontab section
2. Create the skill directory at `harvey-os/skills/<category>/<name>/`
3. Add SKILL.md with trigger phrases and run procedure
4. Document in this register

## Cron Path Convention

All cron jobs use:
```
/usr/local/opt/python@3.11/bin/python3.11 -u $HARVEY_HOME/<agents|core>/<path>/<script>.py
```

Logs go to: `$HARVEY_HOME/data/logs/<script>.log`
