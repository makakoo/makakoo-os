#!/bin/bash
# Nightly parameter evolution for Polymarket Intraday Trader v2
# Uses GBM backtesting to evolve trading parameters

HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"
VENV_PYTHON="$HARVEY_HOME/arbitrage-research-agent/venv/bin/python"
TRADER_DIR="$HARVEY_HOME/data/arbitrage-agent/v2"
LOG_DIR="$TRADER_DIR/logs"

mkdir -p "$LOG_DIR"

echo "=== INTRADAY EVOLUTION $(date) ===" >> "$LOG_DIR/evolution.log"

exec "$VENV_PYTHON" "$TRADER_DIR/intraday_trader.py" \
    --improve --gens 20 --pop 50 \
    >> "$LOG_DIR/evolution.log" 2>&1
