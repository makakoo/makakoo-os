#!/bin/bash
# sniper_monitor.sh — live dashboard of the BTC sniper
# Usage: bash /Users/sebastian/MAKAKOO/agents/arbitrage-agent/sniper_monitor.sh

LOG=/Users/sebastian/MAKAKOO/data/arbitrage-agent/v2/logs/btc_sniper_live.log
PARAMS=/Users/sebastian/MAKAKOO/data/arbitrage-agent/v2/state/sniper_best_params.json

while true; do
    clear
    echo "═══════════════════════════════════════════════════════════════"
    echo "  BTC SNIPER — LIVE MONITOR  $(date '+%H:%M:%S')"
    echo "═══════════════════════════════════════════════════════════════"

    # Process status
    if pgrep -f "btc_sniper_live.py --live" > /dev/null; then
        SNIPER_PID=$(pgrep -f "btc_sniper_live.py --live")
        echo "  ✅ Sniper running (PID $SNIPER_PID)"
    else
        echo "  ❌ SNIPER DEAD"
    fi
    if pgrep -f "btc_backtest_daemon" > /dev/null; then
        echo "  ✅ Backtest daemon running"
    else
        echo "  ⚠️  Backtest daemon stopped"
    fi

    echo ""
    echo "  FILTER THRESHOLDS:"
    /usr/local/opt/python@3.11/bin/python3.11 -c "
import json
d = json.load(open('$PARAMS'))
print(f\"    delta >= \${d['delta_thresh']:.0f}   conf >= {d['conf_thresh']:.2f}   ens >= {d['ens_thresh']:.2f}\")
print(f\"    spend={d['spend_ratio']*100:.0f}%  max_bet={d['max_bet_pct']*100:.0f}%  TP={d['profit_target_bps']}bps  SL={d['stop_loss_bps']}bps\")
"

    echo ""
    echo "  LAST 5 STATUS TICKS:"
    grep "trades=.*Bk=" "$LOG" | tail -5 | sed 's/^/    /'

    echo ""
    echo "  ALL TRADES FIRED THIS SESSION:"
    FIRES=$(grep -cE "\[FIRE|\[ENTRY" "$LOG" 2>/dev/null | head -1)
    FIRES=${FIRES:-0}
    if [ "$FIRES" -gt 0 ] 2>/dev/null; then
        grep "\\[FIRE\\|\\[ENTRY\\|\\[EXIT" "$LOG" | tail -10 | sed 's/^/    /'
    else
        echo "    (none yet — waiting for delta>=\$10 or Poly conviction >=0.95)"
    fi

    echo ""
    echo "  Ctrl+C to exit monitor (does NOT stop the sniper)"
    sleep 5
done
