#!/usr/bin/env python3
"""
Continuous Auto-Improvement Loop v3 - convergent parameter optimizer.

Key fixes from v2:
- HARD FLOORS on stop_loss and take_profit (never wider than 15%)
- Convergence detection: if params haven't improved in N iterations, reverse direction
- Momentum tracking: needs consecutive winning/losing periods before adjusting
- Cap on consecutive adjustments in same direction
- Revert to best-known params if things get worse
"""

import json
import os
import time
import random
from datetime import datetime
from pathlib import Path

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
DATA_DIR = Path(HARVEY_HOME) / "data" / "arbitrage-agent" / "v2"
STATE_FILE = DATA_DIR / "state" / "paper_trades.json"
PARAMS_FILE = DATA_DIR / "state" / "best_intraday_params.json"
LOG_FILE = DATA_DIR / "logs" / "auto_improve.log"
META_FILE = DATA_DIR / "state" / "continuous_improve_meta.json"

SL_FLOOR = 0.10
SL_CAP = 15.0
TP_FLOOR = 0.20
TP_CAP = 15.0
SIZE_FLOOR = 0.02
SIZE_CAP = 0.40
MAX_CONSECUTIVE_ADJUSTMENTS = 5
STAGNATION_THRESHOLD = 4
IMPROVEMENT_THRESHOLD = 1.0


def log(msg):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_FILE, "a") as f:
        f.write(line + "\n")


def load_meta():
    if META_FILE.exists():
        with open(META_FILE) as f:
            return json.load(f)
    return {
        "consecutive_direction": None,
        "consecutive_count": 0,
        "last_capital": 100.0,
        "best_capital": 100.0,
        "best_params": None,
        "stagnation_count": 0,
        "iterations_since_improvement": 0,
    }


def save_meta(meta):
    with open(META_FILE, "w") as f:
        json.dump(meta, f, indent=2)


def load_state():
    if STATE_FILE.exists():
        with open(STATE_FILE) as f:
            return json.load(f)
    return {"capital": 100.0, "trades": [], "wins": 0, "losses": 0, "breakeven": 0}


def clamp(value, lo, hi):
    return max(lo, min(hi, value))


def analyze_and_adjust():
    state = load_state()
    meta = load_meta()
    capital = state.get("capital", 100.0)
    trades = state.get("trades", [])
    wins = state.get("wins", 0)
    losses = state.get("losses", 0)
    breakeven = state.get("breakeven", 0)

    total = wins + losses + breakeven
    wr = wins / max(total, 1) if total > 0 else 0
    pnl = capital - 100.0

    log(f"ANALYSIS: capital=${capital:.2f} PnL=${pnl:+.2f} trades={total} WR={wr:.0%}")

    with open(PARAMS_FILE) as f:
        data = json.load(f)
    params = data["params"].copy()

    if meta["best_params"] is None:
        meta["best_params"] = params.copy()
        meta["best_capital"] = capital

    if len(trades) < 5:
        log("Too few trades - keeping current params")
        return params

    delta = capital - meta["last_capital"]
    meta["last_capital"] = capital

    if delta > IMPROVEMENT_THRESHOLD:
        meta["iterations_since_improvement"] = 0
        if capital > meta["best_capital"]:
            meta["best_capital"] = capital
            meta["best_params"] = params.copy()
            log(f"  → New best capital: ${meta['best_capital']:.2f}")
    else:
        meta["iterations_since_improvement"] += 1

    if meta["iterations_since_improvement"] >= STAGNATION_THRESHOLD:
        log(
            f"  → STAGNATION detected ({STAGNATION_THRESHOLD} iterations no improvement)"
        )
        if meta["consecutive_direction"] == "tighten":
            direction = "loosen"
        elif meta["consecutive_direction"] == "loosen":
            direction = "tighten"
        else:
            direction = "loosen" if pnl < 0 else "tighten"
        meta["consecutive_direction"] = direction
        meta["consecutive_count"] = 0
        meta["iterations_since_improvement"] = 0
        log(f"  → Reversing direction to: {direction}")
    else:
        if delta > 0.5:
            direction = "tighten"
        elif delta < -0.5:
            direction = "loosen"
        else:
            direction = meta.get("consecutive_direction")
            if direction is None:
                direction = "loosen" if pnl < 0 else "tighten"

    if direction == meta.get("consecutive_direction"):
        meta["consecutive_count"] += 1
    else:
        meta["consecutive_count"] = 1
        meta["consecutive_direction"] = direction

    if meta["consecutive_count"] > MAX_CONSECUTIVE_ADJUSTMENTS:
        log(
            f"  → Max consecutive adjustments reached ({MAX_CONSECUTIVE_ADJUSTMENTS}x {direction}) — reverting to best known params"
        )
        params = meta["best_params"].copy()
        meta["consecutive_count"] = 0
        meta["consecutive_direction"] = None
        meta["iterations_since_improvement"] = 0
    else:
        apply_adjustment(params, direction, pnl, wr)

    params["rsi_oversold"] = clamp(params.get("rsi_oversold", 35), 30, 45)
    params["rsi_overbought"] = clamp(params.get("rsi_overbought", 65), 55, 70)
    params["stop_loss_pct"] = clamp(params.get("stop_loss_pct", 0.3), SL_FLOOR, SL_CAP)
    params["take_profit_pct"] = clamp(
        params.get("take_profit_pct", 0.6), TP_FLOOR, TP_CAP
    )
    params["size_pct"] = clamp(params.get("size_pct", 0.15), SIZE_FLOOR, SIZE_CAP)

    with open(PARAMS_FILE, "w") as f:
        json.dump(
            {
                "params": params,
                "score": data.get("score", 100.0),
                "timestamp": datetime.now().isoformat(),
            },
            f,
            indent=2,
        )

    save_meta(meta)

    log(
        f"UPDATED [{direction}]: RSI={params['rsi_oversold']}/{params['rsi_overbought']} SL={params['stop_loss_pct']:.2f}% TP={params['take_profit_pct']:.2f}% size={params['size_pct']:.1%}"
    )
    return params


def apply_adjustment(params, direction, pnl, wr):
    factor = 0.8 if direction == "tighten" else 1.25
    min_factor = 0.9 if direction == "tighten" else 1.1
    jitter = (
        random.uniform(min_factor, 1.0 / min_factor)
        if random.random() < 0.3
        else factor
    )

    if direction == "tighten":
        params["stop_loss_pct"] = clamp(
            params.get("stop_loss_pct", 0.3) * jitter, SL_FLOOR, SL_CAP
        )
        params["take_profit_pct"] = clamp(
            params.get("take_profit_pct", 0.6) * jitter, TP_FLOOR, TP_CAP
        )
        if pnl > 3 and wr > 0.5:
            params["size_pct"] = clamp(
                params.get("size_pct", 0.15) * 1.2, SIZE_FLOOR, SIZE_CAP
            )
    else:
        params["stop_loss_pct"] = clamp(
            params.get("stop_loss_pct", 0.3) * jitter, SL_FLOOR, SL_CAP
        )
        params["take_profit_pct"] = clamp(
            params.get("take_profit_pct", 0.6) * jitter, TP_FLOOR, TP_CAP
        )
        params["size_pct"] = clamp(
            params.get("size_pct", 0.15) * 0.8, SIZE_FLOOR, SIZE_CAP
        )


if __name__ == "__main__":
    log("=== Continuous Improvement v3 Started ===")
    while True:
        analyze_and_adjust()
        time.sleep(600)
