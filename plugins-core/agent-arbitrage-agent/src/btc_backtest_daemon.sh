#!/bin/bash
# btc_backtest_daemon.sh
# Runs btc_backtest_autoresearch every 30 minutes.
# As new trades accumulate in the journal, params auto-retrain against fresh data.
# The live sniper picks up new params on its next GA cycle (or next restart).
#
# Usage:
#   bash /Users/sebastian/MAKAKOO/agents/arbitrage-agent/btc_backtest_daemon.sh
#   # or in background:
#   nohup bash /Users/sebastian/MAKAKOO/agents/arbitrage-agent/btc_backtest_daemon.sh > /dev/null 2>&1 &

HARVEY_HOME="${HARVEY_HOME:-/Users/sebastian/MAKAKOO}"
PYTHON="/usr/local/opt/python@3.11/bin/python3.11"
SCRIPT="$HARVEY_HOME/agents/arbitrage-agent/btc_backtest_autoresearch.py"
INTERVAL="${INTERVAL:-1800}"  # 30 min default

echo "[btc_backtest_daemon] Starting — retraining every ${INTERVAL}s"

while true; do
    echo "[btc_backtest_daemon] $(date) — running backtest autoresearch"
    ITERATIONS=2000 "$PYTHON" "$SCRIPT" 2>&1 | tail -20
    echo "[btc_backtest_daemon] $(date) — sleeping ${INTERVAL}s"
    sleep "$INTERVAL"
done
