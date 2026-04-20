#!/bin/bash
# Nightly Strategy Research — genetic optimization for Polymarket trader
# Uses strategy_researcher.py to find best params

HARVEY_HOME="${HARVEY_HOME:-$HOME/HARVEY}"
PYTHON="/usr/local/opt/python@3.11/bin/python3.11"
SCRIPT_DIR="$HARVEY_HOME/plugins-core/arbitrage-agent"
LOG_DIR="$HARVEY_HOME/data/logs"

mkdir -p "$LOG_DIR"

echo "=== STRATEGY EVOLUTION $(date) ===" >> "$LOG_DIR/strategy_evolution.log"

exec $PYTHON -u "$SCRIPT_DIR/strategy_researcher.py" \
    >> "$LOG_DIR/strategy_evolution.log" 2>&1
