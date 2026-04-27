#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Signal Evolution — Genetic algorithm to evolve BTC technical analysis parameters.

Uses 7 days of Binance 1m klines as historical data.
Fitness = directional accuracy of BTC signals over 60-minute holding periods.

Run: python3 btc_evolution.py [--hours 8] [--population 20] [--generations 100]
"""

import argparse
import copy
import json
import math
import os
import random
import statistics
import time
from datetime import datetime
from typing import Optional
from pathlib import Path

import requests

STATE_DIR = Path(os.environ.get("MAKAKOO_HOME", os.environ.get("HARVEY_HOME", "/Users/sebastian/MAKAKOO"))) / "data" / "arbitrage-agent" / "v2" / "state"
STATE_DIR.mkdir(parents=True, exist_ok=True)
BEST_GENOME_FILE = STATE_DIR / "btc_best_genome.json"
EVOLUTION_LOG = STATE_DIR / "evolution_log.jsonl"

POP_SIZE = 20
MUTATION_RATE = 0.35
ELITE_COUNT = 3
PAPER_CAPITAL = 100.0


def fetch_binance_klines(
    days: int = 7, interval: str = "1m", limit: int = 1000
) -> list[dict]:
    """Fetch historical Binance klines, paginating backwards."""
    all_klines = []
    end_time = int(time.time() * 1000)

    for _ in range(days * 2):
        url = "https://api.binance.com/api/v3/klines"
        params = {
            "symbol": "BTCUSDT",
            "interval": interval,
            "limit": limit,
            "endTime": end_time,
        }
        try:
            r = requests.get(url, params=params, timeout=10)
            if r.status_code != 200:
                break
            batch = r.json()
            if not batch:
                break
            for k in batch:
                all_klines.append(
                    {
                        "ts": k[0] / 1000.0,
                        "open": float(k[1]),
                        "high": float(k[2]),
                        "low": float(k[3]),
                        "close": float(k[4]),
                        "volume": float(k[5]),
                    }
                )
            end_time = int(batch[0][0] - 60000)
            if len(batch) < limit:
                break
            time.sleep(0.25)
        except Exception as e:
            print(f"Binance fetch error: {e}")
            break

    all_klines.reverse()
    print(f"Fetched {len(all_klines)} klines ({days} days)")
    return all_klines


def precompute_indicators(closes: list[float], g) -> dict:
    """Precompute all indicators for every candle index — O(n) per indicator."""
    from indicators import Indicator

    n = len(closes)
    warmup = max(
        g.rsi_period + 5,
        g.macd_slow + 5,
        g.bb_period + 5,
        g.sr_lookback + 5,
        50,
    )

    rsi_vals = [50.0] * n
    macd_hist = [0.0] * n
    bb_pos = [0.5] * n
    bb_upper = [0.0] * n
    bb_lower = [0.0] * n
    supports = [[] for _ in range(n)]
    resistances = [[] for _ in range(n)]

    # RSI — compute once for full array
    try:
        all_rsi = Indicator.rsi(closes, g.rsi_period)
        rsi_offset = len(closes) - len(all_rsi)
        for j, v in enumerate(all_rsi):
            rsi_vals[j + rsi_offset] = v
    except Exception:
        pass

    # MACD — compute once for full array
    try:
        _, _, all_hist = Indicator.macd(closes, g.macd_fast, g.macd_slow, g.macd_signal)
        hist_offset = len(closes) - len(all_hist)
        for j, v in enumerate(all_hist):
            macd_hist[j + hist_offset] = v
    except Exception:
        pass

    # Bollinger Bands — compute once for full array
    try:
        all_bu, _, all_bl = Indicator.bollinger_bands(closes, g.bb_period, g.bb_std)
        bb_offset = len(closes) - len(all_bu)
        for j in range(len(all_bu)):
            idx = j + bb_offset
            bb_upper[idx] = all_bu[j]
            bb_lower[idx] = all_bl[j]
            bw = all_bu[j] - all_bl[j]
            bb_pos[idx] = (closes[idx] - all_bl[j]) / bw if bw > 0 else 0.5
    except Exception:
        pass

    # S/R — only for last N candles (expensive)
    sr_start = max(warmup - g.sr_lookback, 0)
    for i in range(sr_start, n):
        lookback = min(g.sr_lookback, i)
        if lookback < 5:
            continue
        window = closes[i - lookback : i + 1]
        sr_candles = [
            type("C", (), {"close": c, "high": c, "low": c})() for c in window
        ]
        try:
            su, re = Indicator.support_resistance(sr_candles, lookback=lookback)
            supports[i] = su or []
            resistances[i] = re or []
        except Exception:
            pass

    return {
        "rsi": rsi_vals,
        "macd_hist": macd_hist,
        "bb_pos": bb_pos,
        "bb_upper": bb_upper,
        "bb_lower": bb_lower,
        "supports": supports,
        "resistances": resistances,
        "warmup": warmup,
    }


def score_at_index(i: int, pre: dict, closes: list[float], g) -> dict:
    """Compute signal score at index i using precomputed indicators."""
    rsi = pre["rsi"][i]
    hist = pre["macd_hist"][i]
    bb_pos_val = pre["bb_pos"][i]
    bb_up = pre["bb_upper"][i]
    bb_low = pre["bb_lower"][i]
    cur = closes[i]

    rw = g.rsi_weight
    mw = g.macd_weight
    bw = g.bb_weight
    sw = g.sr_weight

    score = 0.0
    if rsi < 30:
        score += 40 * (30 - rsi) / 30 * rw
    elif rsi > 70:
        score -= 40 * (rsi - 70) / 30 * rw
    elif rsi < 45:
        score -= (45 - rsi) * 0.5 * rw
    elif rsi > 55:
        score += (rsi - 55) * 0.5 * rw

    hist_prev = pre["macd_hist"][i - 1] if i > 0 else 0.0
    if hist > 0 and hist_prev <= 0:
        macd_signal = "bullish_cross"
    elif hist < 0 and hist_prev >= 0:
        macd_signal = "bearish_cross"
    elif hist > 0:
        macd_signal = "bullish"
    else:
        macd_signal = "bearish"

    macd_scores = {
        "bullish_cross": 30,
        "bullish": 15,
        "neutral": 0,
        "bearish": -15,
        "bearish_cross": -30,
    }
    score += macd_scores.get(macd_signal, 0) * mw

    if bb_up > 0 and bb_low > 0:
        if bb_pos_val <= 0.10:
            bb_signal = "near_lower"
        elif bb_pos_val >= 0.90:
            bb_signal = "near_upper"
        elif cur < (bb_up + bb_low) / 2:
            bb_signal = "below_middle"
        else:
            bb_signal = "above_middle"
    else:
        bb_signal = "neutral"

    if bb_signal == "near_lower":
        score += 20 * bw
    elif bb_signal == "near_upper":
        score -= 20 * bw
    elif bb_signal == "below_middle":
        score -= 5 * bw
    elif bb_signal == "above_middle":
        score += 5 * bw

    near_support = False
    near_resistance = False
    su = pre["supports"][i] if pre["supports"][i] else []
    re = pre["resistances"][i] if pre["resistances"][i] else []
    below_supports = [s for s in su if s < cur]
    above_resistances = [r for r in re if r > cur]
    if below_supports:
        ns = max(below_supports)
        near_support = (cur - ns) / cur < 0.005
        if near_support:
            score += 10 * sw
    if above_resistances:
        nr = min(above_resistances)
        near_resistance = (nr - cur) / cur < 0.005
        if near_resistance:
            score -= 10 * sw

    score = max(-100.0, min(100.0, score))

    if score > 15:
        direction = "up"
    elif score < -15:
        direction = "down"
    else:
        direction = "neutral"

    confidence = min(1.0, abs(score) / 50.0)

    return {
        "direction": direction,
        "confidence": confidence,
        "score": score,
        "rsi": rsi,
        "macd_signal": macd_signal,
        "bb_signal": bb_signal,
        "near_support": near_support,
        "near_resistance": near_resistance,
    }


def simulate_genome(
    genome, klines: list[dict], holding_minutes: int = 60
) -> Optional[dict]:
    """
    Backtest one genome against historical klines.
    Optimized: precompute indicators once, then sweep.
    """
    from strategy_genome import StrategyGenome

    try:
        g = StrategyGenome.from_dict(genome) if isinstance(genome, dict) else genome
    except Exception:
        return None

    closes = [k["close"] for k in klines]
    n = len(closes)

    pre = precompute_indicators(closes, g)
    warmup = pre["warmup"]
    holding_candles = holding_minutes
    min_conf = g.min_confidence

    correct = 0
    incorrect = 0
    breakeven = 0
    returns = []
    details = []

    STEP = 5

    for i in range(warmup, n - holding_candles, STEP):
        price_now = closes[i]
        price_later = closes[i + holding_candles]

        sig = score_at_index(i, pre, closes, g)

        if sig["direction"] == "neutral" or sig["confidence"] < min_conf:
            continue

        pnl_pct = (price_later - price_now) / price_now
        direction_correct = False

        if sig["direction"] == "up" and price_later > price_now:
            direction_correct = True
        elif sig["direction"] == "down" and price_later < price_now:
            direction_correct = True
        elif abs(pnl_pct) < 0.001:
            breakeven += 1
        else:
            direction_correct = False

        if direction_correct:
            correct += 1
        elif breakeven == 0 or abs(pnl_pct) >= 0.001:
            incorrect += 1

        returns.append(pnl_pct if direction_correct else -pnl_pct)

        if len(details) < 5:
            details.append(
                {
                    "i": i,
                    "dir": sig["direction"],
                    "conf": sig["confidence"],
                    "score": sig["score"],
                    "rsi": sig["rsi"],
                    "macd": sig["macd_signal"],
                    "bb": sig["bb_signal"],
                    "pnl": pnl_pct,
                    "correct": direction_correct,
                }
            )

    total = correct + incorrect + breakeven
    if total == 0:
        return {
            "fitness": -100.0,
            "accuracy": 0.0,
            "avg_return": 0.0,
            "sharpe": 0.0,
            "n_trades": 0,
            "correct": 0,
            "incorrect": 0,
            "breakeven": 0,
            "details": details,
            "genome_name": g.name,
        }

    accuracy = correct / total
    avg_ret = statistics.mean(returns) if returns else 0.0
    std_ret = statistics.stdev(returns) if len(returns) > 1 else 0.001
    sharpe = (avg_ret / std_ret) if std_ret > 0 else 0.0

    fitness = (accuracy - 0.5) * 200
    fitness += sharpe * 10
    fitness += avg_ret * 500
    fitness += (correct - incorrect) * 0.5

    return {
        "fitness": round(fitness, 4),
        "accuracy": round(accuracy, 4),
        "avg_return": round(avg_ret, 6),
        "sharpe": round(sharpe, 4),
        "n_trades": total,
        "correct": correct,
        "incorrect": incorrect,
        "breakeven": breakeven,
        "details": details,
        "genome_name": g.name,
    }


def mutate_population(base_genome, pop_size=POP_SIZE):
    """Create pop_size mutants from base genome."""
    from strategy_genome import StrategyGenome

    pop = [copy.deepcopy(base_genome)]
    base_genome.name = f"base_{datetime.now().strftime('%H%M%S')}"
    for i in range(pop_size - 1):
        mutant = base_genome.mutate(rate=MUTATION_RATE)
        mutant.name = f"mut_{datetime.now().strftime('%H%M%S')}_{i + 1}"
        pop.append(mutant)
    return pop


def crossover_population(scored_pop, pop_size=POP_SIZE):
    """Breed next generation via tournament selection + crossover."""
    from strategy_genome import StrategyGenome

    scored = sorted(scored_pop, key=lambda x: x[0], reverse=True)
    elites = [copy.deepcopy(g) for _, g in scored[:3]]
    next_pop = list(elites)

    while len(next_pop) < pop_size:
        tournament = random.sample(scored, min(4, len(scored)))
        winner = max(tournament, key=lambda x: x[0])[1]
        opponent = random.choice(scored)[1]
        child = StrategyGenome.crossover(winner, opponent)
        child = child.mutate(rate=0.2)
        child.name = f"child_{datetime.now().strftime('%H%M%S')}_{len(next_pop)}"
        next_pop.append(child)

    return next_pop


def load_best_genome():
    if not BEST_GENOME_FILE.exists():
        return None
    try:
        with open(BEST_GENOME_FILE) as f:
            d = json.load(f)
        from strategy_genome import StrategyGenome

        return StrategyGenome.from_dict(d.get("genome", d))
    except Exception:
        return None


def save_genome(genome, score, source="evolution"):
    try:
        with open(BEST_GENOME_FILE, "w") as f:
            json.dump(
                {
                    "genome": genome.to_dict(),
                    "score": score,
                    "source": source,
                    "timestamp": datetime.now().isoformat(),
                },
                f,
                indent=2,
            )
        print(f"  Saved: {BEST_GENOME_FILE} (score={score:.4f})")
    except Exception as e:
        print(f"  Save error: {e}")


def log_generation(gen: int, scored_pop: list, best_ever_score: float):
    try:
        with open(EVOLUTION_LOG, "a") as f:
            for score, genome in scored_pop:
                row = {
                    "generation": gen,
                    "score": round(score, 4),
                    "timestamp": datetime.now().isoformat(),
                    "name": genome.name,
                    **genome.to_dict(),
                }
                f.write(json.dumps(row) + "\n")
    except Exception:
        pass
    best_score = scored_pop[0][0]
    if best_score > best_ever_score:
        print(f"  🏆 NEW ALL-TIME BEST: {best_score:.4f}")
        return best_score
    return best_ever_score


def run_evolution(klines: list[dict], args):
    from strategy_genome import StrategyGenome

    base = load_best_genome()
    if base is None:
        base = StrategyGenome()
        base.name = f"random_{datetime.now().strftime('%H%M%S')}"
        print("No previous genome — created random base")
    else:
        print(f"Loaded best genome: {base.name} (will seed next generation)")

    print(f"Base genome: {base.name}")
    print(
        f"  RSI={base.rsi_period} MACD=({base.macd_fast},{base.macd_slow},{base.macd_signal}) "
        f"BB=({base.bb_period},{base.bb_std}) SR={base.sr_lookback}"
    )
    print(
        f"  weights: rsi={base.rsi_weight:.1f} macd={base.macd_weight:.1f} "
        f"bb={base.bb_weight:.1f} sr={base.sr_weight:.1f}"
    )

    best_ever_score = -999.0
    best_ever_genome = copy.deepcopy(base)
    n_generations = args.generations
    gens_no_improve = 0
    CONVERGENCE_THRESHOLD = 8

    for gen in range(1, n_generations + 1):
        start = time.time()

        population = mutate_population(base, pop_size=args.population)

        scored_pop = []
        for i, genome in enumerate(population):
            result = simulate_genome(genome, klines, holding_minutes=args.holding)
            if result is None:
                scored_pop.append((-999.0, genome))
                continue
            score = result["fitness"]
            scored_pop.append((score, genome))
            print(
                f"  [{i + 1:2d}/{len(population)}] {genome.name[:32]:32s} "
                f"fit={score:7.3f} acc={result['accuracy']:.0%} "
                f"n={result['n_trades']:3d} ret={result['avg_return']:+.5f}"
            )

        scored_pop.sort(key=lambda x: x[0], reverse=True)
        best_score, best_genome = scored_pop[0]

        best_ever_score = log_generation(gen, scored_pop, best_ever_score)
        if best_score >= best_ever_score:
            best_ever_genome = copy.deepcopy(best_genome)
            best_ever_score = best_score

        elapsed = time.time() - start
        print(
            f"\n  Gen {gen}: best={best_genome.name[:25]} score={best_score:.4f} "
            f"acc={best_genome.rsi_period}/{best_genome.macd_fast}/{best_genome.macd_slow}/{best_genome.macd_signal} "
            f"BB={best_genome.bb_period}/{best_genome.bb_std:.1f} "
            f"w=[{best_genome.rsi_weight:.1f},{best_genome.macd_weight:.1f},{best_genome.bb_weight:.1f},{best_genome.sr_weight:.1f}] "
            f"[{elapsed:.1f}s]\n"
        )

        if gen >= 3 and best_score < -80:
            print("  WARNING: Score very low — check genome params / market data")

        base = best_genome
        base.name = f"gen_{gen}_{datetime.now().strftime('%H%M%S')}"

        if best_score > best_ever_score - 1.0:
            gens_no_improve = 0
        else:
            gens_no_improve += 1
            if gens_no_improve >= CONVERGENCE_THRESHOLD:
                print(
                    f"\n  CONVERGED: No improvement for {gens_no_improve} generations — stopping early"
                )
                break

        if gen < n_generations:
            population = crossover_population(scored_pop, pop_size=args.population)

    print(f"\n=== EVOLUTION COMPLETE ===")
    print(f"Best: {best_ever_genome.name} score={best_ever_score:.4f}")
    save_genome(best_ever_genome, best_ever_score, source="evolution")

    result = simulate_genome(best_ever_genome, klines, holding_minutes=args.holding)
    if result:
        print(f"\nBest genome details:")
        print(
            f"  Accuracy: {result['accuracy']:.1%} ({result['correct']}W/{result['incorrect']}L/{result['breakeven']}BE)"
        )
        print(f"  Avg return: {result['avg_return']:+.4%}")
        print(f"  Trades: {result['n_trades']}")
        if result.get("details"):
            print(f"\n  Sample signals:")
            for d in result["details"][:5]:
                print(
                    f"    {d['dir']:6s} conf={d['conf']:.0%} score={d['score']:+.0f} "
                    f"rsi={d['rsi']:.0f} macd={d['macd']:15s} bb={d['bb']} → "
                    f"{'✓' if d['correct'] else '✗'} {d['pnl']:+.2%}"
                )

    return best_ever_genome, best_ever_score


def main():
    parser = argparse.ArgumentParser(description="BTC Signal Evolution")
    parser.add_argument(
        "--hours", type=float, default=8.0, help="Hours to run evolution"
    )
    parser.add_argument(
        "--population", type=int, default=POP_SIZE, help="Population size"
    )
    parser.add_argument("--generations", type=int, default=999, help="Max generations")
    parser.add_argument(
        "--holding", type=int, default=60, help="Holding period in minutes"
    )
    parser.add_argument(
        "--days", type=int, default=7, help="Historical Binance data days"
    )
    args = parser.parse_args()

    print("=" * 60)
    print("BTC SIGNAL EVOLUTION — Genetic Algorithm")
    print(
        f"Population: {args.population} | Holding: {args.holding}min | Data: {args.days} days"
    )
    print("=" * 60)

    print("\nFetching Binance historical data...")
    klines = fetch_binance_klines(days=args.days)
    if len(klines) < 1000:
        print(f"ERROR: Only {len(klines)} klines")
        return

    est_gens = int(args.hours * 3600 / (args.population * 2.0))
    args.generations = min(args.generations, max(est_gens, 2))
    print(
        f"Estimated generations in {args.hours}h: ~{est_gens} (running {args.generations})"
    )

    best_genome, best_score = run_evolution(klines, args)
    print(f"\nDone. Best genome saved to {BEST_GENOME_FILE}")


if __name__ == "__main__":
    main()
