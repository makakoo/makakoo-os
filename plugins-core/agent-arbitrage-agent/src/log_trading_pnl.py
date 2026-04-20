#!/usr/bin/env python3
"""Log intraday trader P&L to Brain."""

import json
import sys
from datetime import datetime
from pathlib import Path

HARVEY_ROOT = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
TRADER_DIR = HARVEY_ROOT / "data" / "arbitrage-agent" / "v2"
JOURNAL_FILE = TRADER_DIR / "state" / "intraday_journal.jsonl"
BEST_PARAMS = TRADER_DIR / "state" / "best_intraday_params.json"
STATE_FILE = TRADER_DIR / "state" / "intraday_trades.json"


def log_to_brain(message: str):
    today = datetime.now().strftime("%Y_%m_%d")
    journal = HARVEY_ROOT / "data" / "Brain" / "journals" / f"{today}.md"
    if journal.exists():
        content = journal.read_text()
        if not content.endswith("\n"):
            content += "\n"
    else:
        content = ""
    ts = datetime.now().strftime("%H:%M")
    entry = f"- [{ts}] TRADING: {message}\n"
    journal.write_text(content + entry)


def main():
    print("=== Trading P&L Report ===")

    if STATE_FILE.exists():
        with open(STATE_FILE) as f:
            state = json.load(f)
        capital = state.get("capital", 0)
        pnl = state.get("pnl", 0)
        wins = state.get("wins", 0)
        losses = state.get("losses", 0)
        trades_list = state.get("trades", [])
        num_trades = len(trades_list) if isinstance(trades_list, list) else trades_list
        wr = (wins / num_trades * 100) if num_trades > 0 else 0

        msg = f"Intraday Trader — Capital ${capital:.2f}, P&L ${pnl:+.4f}, WR {wr:.0f}% ({wins}W/{losses}L/{num_trades} total)"
        print(msg)
        log_to_brain(msg)
    else:
        print("No trading state found")

    if BEST_PARAMS.exists():
        with open(BEST_PARAMS) as f:
            params = json.load(f)
        score = params.get("score", 0)
        msg = f"Best params — score={score:.4f}, mom_th={params.get('mom_th')}, vol_th={params.get('vol_th')}"
        print(msg)
        log_to_brain(msg)

    return 0


if __name__ == "__main__":
    sys.exit(main())
