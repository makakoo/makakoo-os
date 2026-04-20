#!/bin/bash
# Cron wrapper for Polymarket Intraday Trader v2
# Kills old instances before starting, uses venv Python, paper trading mode

HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"
VENV_PYTHON="$HARVEY_HOME/arbitrage-research-agent/venv/bin/python"
TRADER_DIR="$HARVEY_HOME/data/arbitrage-agent/v2"
LOG_DIR="$TRADER_DIR/logs"

mkdir -p "$LOG_DIR"

pkill -f "intraday_trader.py" 2>/dev/null
sleep 1

exec "$VENV_PYTHON" "$TRADER_DIR/intraday_trader.py" \
    --capital 100 \
    --poll 5 \
    >> "$LOG_DIR/intraday_v5.log" 2>&1
