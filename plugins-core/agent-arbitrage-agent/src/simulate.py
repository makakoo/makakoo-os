#!/usr/bin/env python3
"""
Polymarket Trading System v2 — Auto-Improvement Simulation Runner

Runs a 2-hour simulation with continuous genome evolution:
1. Populate with random strategies
2. Score each genome against live Polymarket market data
3. Evolve: mutate + crossover + elite selection
4. Repeat for N generations
5. Output best strategy + detailed results

Usage:
  python3 simulate.py [--generations 30] [--duration 120] [--population 20]
"""

import sys
import os
import json
import time
import argparse
from datetime import datetime

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from config import (
    INITIAL_CAPITAL,
    POPULATION_SIZE,
    GENERATIONS,
    SIM_DURATION_MINUTES,
    ELITE_COUNT,
    MUTATION_RATE,
    TOURNAMENT_SIZE,
    POLL_INTERVAL_SECONDS,
)
from engine import SimulationEngine
from scanner import Scanner
from strategy_genome import StrategyGenome, FitnessTracker


DATA_DIR = os.path.dirname(os.path.abspath(__file__))
HISTORY_FILE = os.path.join(DATA_DIR, "logs", "simulation_history.jsonl")
BEST_GENOME_FILE = os.path.join(DATA_DIR, "state", "best_genome.json")
MARKETS_SNAPSHOT = os.path.join(DATA_DIR, "state", "markets_snapshot.json")


def take_markets_snapshot(scanner: Scanner):
    """Take a snapshot of available markets for consistent backtesting."""
    markets = scanner.scan()
    snapshot = []
    for m in markets:
        snapshot.append(
            {
                "id": m.id,
                "question": m.question,
                "tokens": m.tokens,
                "yes_price": m.yes_price,
                "no_price": m.no_price,
                "spread_bps": m.spread_bps,
                "spread_pct": m.spread_pct,
                "mid_price": m.mid_price,
                "liquidity": m.liquidity,
                "volume_24h": m.volume_24h,
                "resolved": m.resolved,
                "market_type": m.market_type,
                "n_legs": m.n_legs,
                "best_bid_yes": m.best_bid_yes,
                "best_ask_yes": m.best_ask_yes,
                "best_bid_no": m.best_bid_no,
                "best_ask_no": m.best_ask_no,
            }
        )
    with open(MARKETS_SNAPSHOT, "w") as f:
        json.dump(snapshot, f, indent=2)
    print(f"\n📸 Market snapshot saved: {len(snapshot)} markets")
    return snapshot


def run_simulation_cycle(
    engine: SimulationEngine,
    population: list,
    duration_minutes: int,
) -> list:
    """Score all genomes in population against live market data."""
    scores = []
    for i, genome in enumerate(population):
        print(f"\n[Genome {i + 1}/{len(population)}]")
        score = engine.score_genome(genome, duration_minutes=duration_minutes)
        scores.append(score)
        time.sleep(0.5)
    return scores


def save_genome(genome: StrategyGenome, score: float):
    """Save best genome to disk."""
    data = {
        "genome": genome.to_dict(),
        "version": genome.version,
        "score": score,
        "timestamp": datetime.now().isoformat(),
    }
    with open(BEST_GENOME_FILE, "w") as f:
        json.dump(data, f, indent=2)


def main():
    parser = argparse.ArgumentParser(
        description="Polymarket Auto-Improvement Simulation"
    )
    parser.add_argument("--generations", type=int, default=GENERATIONS)
    parser.add_argument("--duration", type=int, default=SIM_DURATION_MINUTES)
    parser.add_argument("--population", type=int, default=POPULATION_SIZE)
    parser.add_argument(
        "--no-fetch", action="store_true", help="Skip live market fetch"
    )
    args = parser.parse_args()

    print("=" * 70)
    print("POLYMARKET TRADING SYSTEM v2 — AUTO-IMPROVEMENT SIMULATION")
    print("=" * 70)
    print(f"Generations : {args.generations}")
    print(f"Population  : {args.population}")
    print(f"Duration    : {args.duration} minutes")
    print(f"Initial Cap : ${INITIAL_CAPITAL}")
    print(f"Started at  : {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print("=" * 70)

    os.makedirs(os.path.join(DATA_DIR, "logs"), exist_ok=True)
    os.makedirs(os.path.join(DATA_DIR, "state"), exist_ok=True)

    engine = SimulationEngine(initial_capital=INITIAL_CAPITAL)
    tracker = FitnessTracker(HISTORY_FILE)

    print("\n🌐 Fetching live Polymarket markets...")
    try:
        if args.no_fetch:
            print("  Skipping (--no-fetch flag)")
        else:
            snapshot = take_markets_snapshot(engine.scanner)
            if not snapshot:
                print("  WARNING: No markets fetched. Using cached data if available.")
    except Exception as e:
        print(f"  ERROR fetching markets: {e}")
        print("  Continuing with empty market state...")

    print(
        f"\n🚀 Starting evolution: {args.population} genomes × {args.generations} generations"
    )
    population = StrategyGenome.random_population(args.population)

    best_overall = None
    best_overall_score = float("-inf")

    for gen in range(args.generations):
        gen_start = time.time()
        print(f"\n{'=' * 70}")
        print(f"GENERATION {gen + 1}/{args.generations}")
        print(f"{'=' * 70}")

        scores = run_simulation_cycle(engine, population, args.duration)

        scored = sorted(zip(scores, population), key=lambda x: x[0], reverse=True)
        top_score, top_genome = scored[0]

        if top_score > best_overall_score:
            best_overall_score = top_score
            best_overall = top_genome
            save_genome(best_overall, best_overall_score)
            print(f"\n🏆 NEW ALL-TIME BEST: {best_overall_score:.4f}")
            print(f"   Strategy: {best_overall.name}")
            print(
                f"   Params: spread={best_overall.spread_multiplier:.1f}x "
                f"bid={best_overall.bid_offset_bps}bps "
                f"fill={best_overall.fill_probability:.0%} "
                f"kelly={best_overall.kelly_fraction:.0%} "
                f"max_pos={best_overall.max_positions} "
                f"taker={best_overall.taker_mode}"
            )

        population = tracker.evaluate_and_select(population, scores)

        gen_time = time.time() - gen_start
        print(
            f"\n⏱️  Generation took {gen_time:.1f}s | Est. remaining: "
            f"{gen_time * (args.generations - gen - 1) / 60:.1f} min"
        )

    print(f"\n{'=' * 70}")
    print("SIMULATION COMPLETE")
    print(f"{'=' * 70}")
    print(f"Best Score:  {best_overall_score:.4f}")
    print(f"Best Genome: {best_overall.name if best_overall else 'N/A'}")
    if best_overall:
        print(f"\nBest Parameters:")
        for k, v in best_overall.to_dict().items():
            print(f"  {k}: {v}")
    print(f"\nResults saved to: {HISTORY_FILE}")
    print(f"Best genome saved to: {BEST_GENOME_FILE}")
    print(f"Started: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"Finished: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")


if __name__ == "__main__":
    main()
