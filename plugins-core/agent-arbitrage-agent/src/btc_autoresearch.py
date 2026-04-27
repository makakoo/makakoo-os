#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Autoresearch Loop — Karpathy's NEVER-STOP autoresearch pattern for BTC trading.

Runs autonomous overnight genome experiments on best_genome.json.
Each experiment: mutate → run paper session → score → keep/discard → log → repeat.

NEVER STOPS until human kills it (Ctrl+C).

Usage:
    python3 agents/arbitrage-agent/btc_autoresearch.py

Environment:
    SESSION_SECONDS  — paper session budget per experiment (default 600 = 10 min)
    MUTATION_RATE    — genome mutation probability per param (default 0.3)
    TAG              — experiment tag for this run (default YYYYMMDD)

Output:
    evolution_results.tsv — tab-separated experiment log
    best_genome.json       — updated in-place on improvements
"""

import copy
import hashlib
import json
import os
import random
import signal
import sys
import time
from datetime import datetime
from pathlib import Path

# ── Paths ────────────────────────────────────────────────────────────────────
HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
BEST_GENOME_FILE = STATE_DIR / "best_genome.json"
EVOLUTION_TSV = STATE_DIR / "evolution_results.tsv"

AGENTS_DIR = HARVEY_HOME / "agents" / "arbitrage-agent"
sys.path.insert(0, str(AGENTS_DIR))

# ── Config ────────────────────────────────────────────────────────────────────
SESSION_SECONDS = int(os.environ.get("SESSION_SECONDS", "600"))
MUTATION_RATE = float(os.environ.get("MUTATION_RATE", "0.3"))
TAG = os.environ.get("TAG", datetime.now().strftime("%Y%m%d"))
PAPER_CAPITAL = 100.0


# ── Logging ───────────────────────────────────────────────────────────────────
def log(msg: str) -> None:
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    with open(LOG_DIR / "btc_autoresearch.log", "a") as f:
        f.write(line + "\n")


def log_tsv(
    commit: str, fitness: float, capital: float, status: str, description: str
) -> None:
    """Append one row to the TSV experiment log."""
    row = f"{commit}\t{fitness:.4f}\t{capital:.4f}\t{status}\t{description}"
    with open(EVOLUTION_TSV, "a") as f:
        f.write(row + "\n")


# ── Git commit hash from genome params ───────────────────────────────────────
def genome_commit(genome_dict: dict) -> str:
    """Derive a short git-like hash from mutated param values."""
    relevant = {
        k: v
        for k, v in genome_dict.items()
        if k not in ("name", "version") and isinstance(v, (int, float, bool))
    }
    encoded = json.dumps(relevant, sort_keys=True).encode()
    return hashlib.sha1(encoded).hexdigest()[:8]


def describe_mutation(before: dict, after: dict) -> str:
    """Describe which param(s) changed between two genomes."""
    changes = []
    for k, v in after.items():
        if k in ("name", "version"):
            continue
        if k not in before or before[k] != v:
            old = before.get(k, "?")
            changes.append(f"{k}: {old}→{v}")
    if not changes:
        return "no_change"
    return "; ".join(changes[:3])  # cap description length


# ── Genome loading / saving ───────────────────────────────────────────────────
def load_best_genome():
    """Load current best genome from best_genome.json."""
    if not BEST_GENOME_FILE.exists():
        log(f"WARNING: {BEST_GENOME_FILE} not found — creating random base")
        from strategy_genome import StrategyGenome

        g = StrategyGenome()
        g.name = f"random_init_{datetime.now().strftime('%H%M%S')}"
        save_genome(g, 0.0, "init")
        return g

    with open(BEST_GENOME_FILE) as f:
        d = json.load(f)
    from strategy_genome import StrategyGenome

    return StrategyGenome.from_dict(d.get("genome", d))


def save_genome(genome, score: float, source: str = "autoresearch") -> None:
    """Save genome to best_genome.json."""
    d = {
        "genome": genome.to_dict(),
        "score": score,
        "source": source,
        "timestamp": datetime.now().isoformat(),
    }
    with open(BEST_GENOME_FILE, "w") as f:
        json.dump(d, f, indent=2)
    log(f"Saved genome score={score:.4f} source={source}")


# ── Run paper session ─────────────────────────────────────────────────────────
def run_session(genome_dict: dict, session_seconds: int = SESSION_SECONDS):
    """Call run_paper_session() from autoimprove_live.py — returns result dict or None."""
    from autoimprove_live import run_paper_session

    try:
        result = run_paper_session(genome_dict, session_seconds=session_seconds)
        return result
    except Exception as e:
        log(f"  run_paper_session CRASHED: {e}")
        return None


# ── Main loop ─────────────────────────────────────────────────────────────────
def main():
    log("=" * 60)
    log("BTC AUTORESEARCH — Karpathy NEVER-STOP loop starting")
    log(f"  Tag:        {TAG}")
    log(f"  Session:    {SESSION_SECONDS}s")
    log(f"  Mutation:   rate={MUTATION_RATE}")
    log(f"  Best genome: {BEST_GENOME_FILE}")
    log(f"  TSV log:    {EVOLUTION_TSV}")
    log("=" * 60)

    STATE_DIR.mkdir(parents=True, exist_ok=True)

    # Ensure TSV header exists
    if not EVOLUTION_TSV.exists():
        with open(EVOLUTION_TSV, "w") as f:
            f.write("commit\tfitness\tcapital\tstatus\tdescription\n")
        log(f"Created {EVOLUTION_TSV}")

    # Load current best
    current_genome = load_best_genome()
    current_score = 0.0

    # Try to get existing score from best_genome.json
    if BEST_GENOME_FILE.exists():
        with open(BEST_GENOME_FILE) as f:
            d = json.load(f)
        current_score = d.get("score", 0.0)

    log(f"Starting genome: {current_genome.name} score={current_score:.4f}")

    attempt = 0

    # ── NEVER STOP loop ───────────────────────────────────────────────────────
    while True:
        attempt += 1
        log(f"\n--- Attempt #{attempt} ---")

        # 1. Mutate
        before_dict = current_genome.to_dict()
        mutated = current_genome.mutate(rate=MUTATION_RATE)
        mutated_dict = mutated.to_dict()
        description = describe_mutation(before_dict, mutated_dict)
        commit = genome_commit(mutated_dict)

        log(f"  Mutated: {mutated.name} [{commit}] — {description}")

        # 2. Run paper session
        result = run_session(mutated_dict, session_seconds=SESSION_SECONDS)

        if result is None:
            # Crash — log and discard
            log(f"  CRASH — discarding {commit}")
            log_tsv(commit, 0.0, PAPER_CAPITAL, "crash", description)
            continue

        new_score = result.get("score", 0.0)
        new_capital = result.get("capital", PAPER_CAPITAL)
        log(
            f"  Result: score={new_score:.4f} capital=${new_capital:.4f} "
            f"pnl=${result.get('total_pnl', 0):+.4f} WR={result.get('win_rate', 0):.0%}"
        )

        # 3. Decision: keep or discard?
        if new_score > current_score:
            status = "keep"
            current_genome = mutated
            current_score = new_score
            save_genome(mutated, new_score, source="autoresearch")
            log(
                f"  🏆 KEEP — improved +{new_score - current_score:.4f} → new best={new_score:.4f}"
            )
        else:
            status = "discard"
            log(f"  DISCARD — {new_score:.4f} ≤ {current_score:.4f}")

        # 4. Log result to TSV
        log_tsv(commit, new_score, new_capital, status, description)

        # Small jitter to avoid tight loops if session_seconds is short
        time.sleep(2)


if __name__ == "__main__":
    random.seed()

    def ctrlc_handler(sig, frame):
        log("\nBTC AUTORESEARCH — caught Ctrl+C, exiting gracefully")
        sys.exit(0)

    signal.signal(signal.SIGINT, ctrlc_handler)
    signal.signal(signal.SIGTERM, ctrlc_handler)

    main()
