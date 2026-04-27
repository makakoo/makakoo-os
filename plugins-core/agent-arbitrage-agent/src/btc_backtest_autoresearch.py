#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Backtest Autoresearch — proper evolution that actually improves the sniper.

Unlike btc_autoresearch.py (which evolves the wrong genome against wrong markets),
this one:
  1. Loads ACTUAL historical BTC sniper trades from intraday_journal.jsonl
  2. Evolves SniperParams (the real param struct the live sniper uses)
  3. Replays each trade through the filter to see which would have fired
  4. Scores by realized PnL + WR + trade count
  5. Writes winner to sniper_best_params.json (the file live sniper reads)

Because the backtest is instant (no live API calls), we can run 10,000+ experiments
per minute vs the old 1-per-10-minutes rate.

Usage:
    /usr/local/bin/python3.11 agents/arbitrage-agent/btc_backtest_autoresearch.py
    # or with options:
    ITERATIONS=5000 MUTATION_RATE=0.4 ./btc_backtest_autoresearch.py

Output:
    sniper_best_params.json        — updated in-place when improvements found
    backtest_evolution.tsv         — one row per experiment
"""

import copy
import json
import os
import random
import signal
import sys
import time
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path

# ── Paths ────────────────────────────────────────────────────────────────────
HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
JOURNAL_FILE = STATE_DIR / "intraday_journal.jsonl"
BEST_PARAMS_FILE = STATE_DIR / "sniper_best_params.json"
EVOLUTION_TSV = STATE_DIR / "backtest_evolution.tsv"

STATE_DIR.mkdir(parents=True, exist_ok=True)
LOG_DIR.mkdir(parents=True, exist_ok=True)

# ── Config ────────────────────────────────────────────────────────────────────
ITERATIONS = int(os.environ.get("ITERATIONS", "2000"))
MUTATION_RATE = float(os.environ.get("MUTATION_RATE", "0.35"))
RESTART_EVERY = int(os.environ.get("RESTART_EVERY", "200"))  # jump out of local minima
MIN_TRADES = int(os.environ.get("MIN_TRADES", "15"))  # reject overfit single-trade wins


def log(msg: str) -> None:
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_DIR / "btc_backtest_autoresearch.log", "a") as f:
        f.write(line + "\n")


# ── Load historical BTC trades ────────────────────────────────────────────────
def load_btc_trades() -> list[dict]:
    """Load all BTC-specific trades from the journal (entries with btc_delta field)."""
    trades = []
    with open(JOURNAL_FILE) as f:
        for line in f:
            try:
                t = json.loads(line)
                if "btc_delta" in t and "conf" in t and "won" in t and "pnl" in t:
                    trades.append(t)
            except Exception:
                continue
    return trades


# ── Replay filter ─────────────────────────────────────────────────────────────
def would_fire(trade: dict, delta_thresh: float, conf_thresh: float, ens_thresh: float) -> bool:
    """Mirror btc_sniper_live.py::_check_signal filter logic.

    Fire condition:
      (|btc_delta| >= delta_thresh OR poly_conv >= 0.90)
      AND (conf >= conf_thresh OR poly_conv >= 0.90)
      AND conf >= ens_thresh  (else ensemble would have returned Neutral)
    """
    btc_delta = abs(trade["btc_delta"])
    conf = trade["conf"]
    poly_price = trade.get("poly_price", 0.5)
    poly_conv = abs(poly_price - 0.5) * 2

    # Poly conviction override: if market is extreme, always fires
    if poly_conv >= 0.90:
        # ens_thresh still applies — ensemble must have returned a direction
        return conf >= ens_thresh

    # Normal path: all three filters must pass
    if btc_delta < delta_thresh:
        return False
    if conf < conf_thresh:
        return False
    if conf < ens_thresh:
        return False
    return True


# ── Score a parameter set ─────────────────────────────────────────────────────
def backtest_score(trades: list[dict], params: dict) -> dict:
    """Replay all trades through filter. Return aggregate score."""
    delta_thresh = params["delta_thresh"]
    conf_thresh = params["conf_thresh"]
    ens_thresh = params["ens_thresh"]

    fired = [t for t in trades if would_fire(t, delta_thresh, conf_thresh, ens_thresh)]
    n = len(fired)
    if n == 0:
        return {"score": -9999.0, "pnl": 0.0, "wins": 0, "losses": 0, "n": 0, "wr": 0.0}

    wins = sum(1 for t in fired if t.get("won"))
    losses = n - wins
    pnl = sum(t.get("pnl", 0) for t in fired)
    wr = wins / n

    # Penalty if trade count below min: discourage overfitting to 1 lucky trade
    penalty = 0.0
    if n < MIN_TRADES:
        penalty = (MIN_TRADES - n) * 5.0

    # Score: PnL is primary, WR is bonus, log(trade count) rewards coverage
    # Formula tuned so $278 PnL / 22 trades / 73% WR (the known-good bucket) scores well
    # Add strong fire-rate bonus: historical conf filter is biased (trades only fired at old
    # loose thresholds), so we need trade volume to compensate for unknown near-miss trades
    import math
    score = pnl * 1.5 + wr * 20.0 + math.log(1 + n) * 8.0 - penalty

    return {
        "score": score,
        "pnl": pnl,
        "wins": wins,
        "losses": losses,
        "n": n,
        "wr": wr,
    }


# ── Param loading / mutation / saving ─────────────────────────────────────────
KNOWN_SNIPER_FIELDS = {
    "version", "name", "delta_thresh", "conf_thresh", "ens_thresh",
    "spend_ratio", "max_bet_pct", "profit_target_bps", "stop_loss_bps",
    "max_hold_seconds", "min_market_volume", "max_spread_bps",
    "session_minutes", "pop_size",
}


def load_best_params() -> dict:
    """Load current SniperParams from sniper_best_params.json."""
    if BEST_PARAMS_FILE.exists():
        with open(BEST_PARAMS_FILE) as f:
            d = json.load(f)
        # Strip non-param fields that might exist
        return {k: v for k, v in d.items() if k in KNOWN_SNIPER_FIELDS}
    # Default
    return {
        "version": "pro1.0",
        "name": "backtest_init",
        "delta_thresh": 15.0,
        "conf_thresh": 0.80,
        "ens_thresh": 0.60,
        "spend_ratio": 0.35,
        "max_bet_pct": 0.4,
        "profit_target_bps": 200,
        "stop_loss_bps": 100,
        "max_hold_seconds": 300,
        "min_market_volume": 47000,
        "max_spread_bps": 950,
        "session_minutes": 60,
        "pop_size": 12,
    }


def mutate_params(params: dict, rate: float = MUTATION_RATE) -> dict:
    """Mutate the three replayable threshold params."""
    new = copy.deepcopy(params)
    new["name"] = f"bt_{datetime.now().strftime('%H%M%S')}_{random.randint(1000,9999)}"

    # Mutation ranges centered on known-good space
    mutations = [
        ("delta_thresh", 8.0, 35.0, 1.0),
        ("conf_thresh", 0.50, 0.95, 0.05),
        ("ens_thresh", 0.30, 0.85, 0.05),
    ]
    for attr, lo, hi, step in mutations:
        if random.random() < rate:
            val = round(random.uniform(lo, hi) / step) * step
            new[attr] = round(val, 2)
    return new


def random_restart_params(base: dict) -> dict:
    """Jump to a random point in the search space to escape local minima."""
    new = copy.deepcopy(base)
    new["name"] = f"btrestart_{datetime.now().strftime('%H%M%S')}"
    new["delta_thresh"] = round(random.uniform(8.0, 30.0) / 1.0) * 1.0
    new["conf_thresh"] = round(random.uniform(0.50, 0.95) / 0.05) * 0.05
    new["ens_thresh"] = round(random.uniform(0.30, 0.85) / 0.05) * 0.05
    return new


def save_best_params(params: dict, result: dict) -> None:
    """Write winning params to sniper_best_params.json (the file live sniper reads)."""
    out = copy.deepcopy(params)
    out["best_score"] = result["score"]
    out["generation"] = int(time.time())
    with open(BEST_PARAMS_FILE, "w") as f:
        json.dump(out, f, indent=2)


# ── TSV logging ───────────────────────────────────────────────────────────────
def init_tsv() -> None:
    if not EVOLUTION_TSV.exists():
        with open(EVOLUTION_TSV, "w") as f:
            f.write("iter\tdelta\tconf\tens\tn\twins\tlosses\twr\tpnl\tscore\tstatus\n")


def log_tsv(i: int, p: dict, r: dict, status: str) -> None:
    with open(EVOLUTION_TSV, "a") as f:
        f.write(
            f"{i}\t{p['delta_thresh']}\t{p['conf_thresh']}\t{p['ens_thresh']}\t"
            f"{r['n']}\t{r['wins']}\t{r['losses']}\t{r['wr']:.3f}\t"
            f"{r['pnl']:.2f}\t{r['score']:.2f}\t{status}\n"
        )


# ── Main ──────────────────────────────────────────────────────────────────────
def main():
    log("=" * 70)
    log("BTC BACKTEST AUTORESEARCH — evolves SniperParams against real historical trades")
    log(f"  Iterations: {ITERATIONS}")
    log(f"  Mutation rate: {MUTATION_RATE}")
    log(f"  Restart every: {RESTART_EVERY}")
    log(f"  Min trades: {MIN_TRADES}")
    log("=" * 70)

    # Load historical trades
    trades = load_btc_trades()
    if len(trades) < 20:
        log(f"ERROR: Only {len(trades)} BTC trades in journal. Need 20+. Aborting.")
        return

    log(f"Loaded {len(trades)} historical BTC trades from journal")

    total_pnl_all = sum(t.get("pnl", 0) for t in trades)
    total_wins = sum(1 for t in trades if t.get("won"))
    log(f"Baseline (no filter): n={len(trades)} WR={total_wins/len(trades):.0%} PnL=${total_pnl_all:+.2f}")

    # Initial params
    current = load_best_params()
    current_result = backtest_score(trades, current)
    log(
        f"Starting params: delta={current['delta_thresh']} conf={current['conf_thresh']} ens={current['ens_thresh']}"
    )
    log(
        f"  → n={current_result['n']} WR={current_result['wr']:.0%} "
        f"PnL=${current_result['pnl']:+.2f} score={current_result['score']:.2f}"
    )

    best = copy.deepcopy(current)
    best_result = current_result

    init_tsv()

    keeps = 0
    restarts = 0
    log_tsv(0, current, current_result, "init")

    for i in range(1, ITERATIONS + 1):
        # Random restart periodically to escape local minima
        if i % RESTART_EVERY == 0:
            candidate = random_restart_params(best)
            restarts += 1
        else:
            candidate = mutate_params(current)

        result = backtest_score(trades, candidate)

        if result["score"] > best_result["score"]:
            best = copy.deepcopy(candidate)
            best_result = result
            current = copy.deepcopy(candidate)
            keeps += 1
            log(
                f"[{i}] KEEP delta={candidate['delta_thresh']} conf={candidate['conf_thresh']:.2f} "
                f"ens={candidate['ens_thresh']:.2f} → n={result['n']} WR={result['wr']:.0%} "
                f"PnL=${result['pnl']:+.2f} score={result['score']:.2f}"
            )
            save_best_params(best, best_result)
            log_tsv(i, candidate, result, "keep")
        else:
            # Hill climbing: probabilistically accept slightly worse to explore
            if random.random() < 0.05 and result["score"] > best_result["score"] - 10:
                current = copy.deepcopy(candidate)
                log_tsv(i, candidate, result, "accept_worse")
            else:
                log_tsv(i, candidate, result, "discard")

    # ── Final report ─────────────────────────────────────────────────────────
    log("=" * 70)
    log("BACKTEST AUTORESEARCH COMPLETE")
    log(f"  Iterations: {ITERATIONS}")
    log(f"  Improvements: {keeps}")
    log(f"  Restarts: {restarts}")
    log("=" * 70)
    log(
        f"BEST: delta={best['delta_thresh']} conf={best['conf_thresh']} ens={best['ens_thresh']}"
    )
    log(
        f"  → n={best_result['n']} WR={best_result['wr']:.0%} "
        f"PnL=${best_result['pnl']:+.2f} score={best_result['score']:.2f}"
    )
    log(f"Written to {BEST_PARAMS_FILE}")
    log(f"TSV log: {EVOLUTION_TSV}")


if __name__ == "__main__":
    random.seed()

    def ctrlc_handler(sig, frame):
        log("\nCaught Ctrl+C, exiting")
        sys.exit(0)

    signal.signal(signal.SIGINT, ctrlc_handler)
    signal.signal(signal.SIGTERM, ctrlc_handler)

    main()
