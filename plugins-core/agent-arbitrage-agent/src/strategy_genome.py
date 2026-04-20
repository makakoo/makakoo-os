"""
Strategy Genome — evolvable parameter set for the trading strategy.
Mutations, crossover, and fitness evaluation.
"""

import random
import json
import copy
from dataclasses import dataclass, field, asdict
from typing import Dict, List, Optional
from datetime import datetime


@dataclass
class StrategyGenome:
    version: str = "v2.0"
    name: str = ""

    # Market selection
    min_liquidity: int = 500
    max_legs: int = 2
    min_spread_bps: int = 20
    min_volume_usd: float = 1000.0
    max_price: float = 0.95
    max_hours: float = 48.0

    # BTC Signal indicator params (used by BTCSignalGenerator)
    rsi_period: int = 7
    macd_fast: int = 6
    macd_slow: int = 13
    macd_signal: int = 5
    bb_period: int = 10
    bb_std: float = 2.0
    sr_lookback: int = 20

    # Signal scoring weights
    rsi_weight: float = 1.0
    macd_weight: float = 1.0
    bb_weight: float = 1.0
    sr_weight: float = 1.0

    # Position sizing
    max_position_pct: float = 0.10
    min_position_size: float = 0.50
    max_position_size: float = 10.0
    kelly_fraction: float = 0.25

    # Risk management
    max_daily_loss_pct: float = 0.05
    max_positions: int = 5
    cancel_after_seconds: int = 300
    min_confidence: float = 0.50

    # Market making
    fill_probability: float = 0.30
    spread_multiplier: float = 1.0
    rebate_capture_bps: int = 4

    # Order placement
    bid_offset_bps: int = 5
    ask_offset_bps: int = 5
    post_both_sides: bool = True

    # Simulation-specific
    taker_mode: bool = False

    def __post_init__(self):
        if not self.name:
            self.name = f"gen_{datetime.now().strftime('%H%M%S')}"

    def to_dict(self) -> Dict:
        d = asdict(self)
        d.pop("version", None)
        return d

    @classmethod
    def from_dict(cls, d: Dict) -> "StrategyGenome":
        d_copy = copy.deepcopy(d)
        version = d_copy.pop("version", "v2.0")
        known = {f.name for f in cls.__dataclass_fields__.values()}
        for k in list(d_copy.keys()):
            if k not in known:
                d_copy.pop(k)
        return cls(version=version, **d_copy)

    def mutate(self, rate: float = 0.2) -> "StrategyGenome":
        """Create a mutated copy of this genome."""
        new = copy.deepcopy(self)
        new.name = (
            f"gen_{datetime.now().strftime('%H%M%S')}_{random.randint(1000, 9999)}"
        )

        mutations = [
            ("min_liquidity", 100, 5000, 100),
            ("max_legs", 2, 5, 1),
            ("min_spread_bps", 10, 100, 5),
            ("min_volume_usd", 500, 50000, 500),
            ("max_price", 0.80, 0.99, 0.01),
            ("max_hours", 1.0, 200.0, 1.0),
            ("bid_offset_bps", 1, 50, 1),
            ("ask_offset_bps", 1, 50, 1),
            ("max_position_pct", 0.02, 0.30, 0.02),
            ("min_position_size", 0.10, 5.0, 0.10),
            ("max_position_size", 1.0, 50.0, 1.0),
            ("kelly_fraction", 0.05, 1.0, 0.05),
            ("max_daily_loss_pct", 0.01, 0.20, 0.01),
            ("max_positions", 1, 10, 1),
            ("cancel_after_seconds", 60, 900, 60),
            ("fill_probability", 0.05, 0.80, 0.05),
            ("spread_multiplier", 0.5, 3.0, 0.1),
            ("rebate_capture_bps", 1, 20, 1),
            ("min_confidence", 0.30, 0.80, 0.05),
            # BTC indicator params
            ("rsi_period", 4, 21, 1),
            ("macd_fast", 4, 20, 1),
            ("macd_slow", 14, 30, 1),
            ("macd_signal", 3, 15, 1),
            ("bb_period", 5, 30, 1),
            ("bb_std", 1.0, 3.0, 0.1),
            ("sr_lookback", 10, 50, 5),
            ("rsi_weight", 0.3, 2.0, 0.1),
            ("macd_weight", 0.3, 2.0, 0.1),
            ("bb_weight", 0.3, 2.0, 0.1),
            ("sr_weight", 0.0, 1.5, 0.1),
        ]

        for attr, lo, hi, step in mutations:
            if random.random() < rate:
                val = round(random.uniform(lo, hi) / step) * step
                setattr(new, attr, val)

        if random.random() < rate:
            new.post_both_sides = not self.post_both_sides

        if random.random() < rate:
            new.taker_mode = not self.taker_mode

        return new

    @classmethod
    def crossover(cls, a: "StrategyGenome", b: "StrategyGenome") -> "StrategyGenome":
        """Single-point crossover between two genomes."""
        d_a = a.to_dict()
        d_b = b.to_dict()
        keys = list(d_a.keys())
        crossover_point = random.randint(1, len(keys) - 1)
        child_dict = {}
        for i, k in enumerate(keys):
            child_dict[k] = d_a[k] if i < crossover_point else d_b[k]
        child_dict["version"] = "v2.0"
        child_dict["name"] = f"x_{datetime.now().strftime('%H%M%S')}"
        return cls.from_dict(child_dict)

    @classmethod
    def random_population(cls, size: int) -> List["StrategyGenome"]:
        """Create a random population of genomes with actual diversity."""
        pop = []
        base = cls()
        base.name = f"init_{datetime.now().strftime('%H%M%S')}"
        for _ in range(size):
            mutated = base.mutate(rate=1.0)
            mutated.name = (
                f"init_{datetime.now().strftime('%H%M%S')}_{random.randint(1000, 9999)}"
            )
            pop.append(mutated)
        return pop


class FitnessTracker:
    """Tracks genome fitness across generations."""

    def __init__(self, history_file: str):
        self.history_file = history_file
        self.generation = 0
        self.all_time_best: Optional[StrategyGenome] = None
        self.all_time_best_score = float("-inf")

    def evaluate_and_select(
        self, population: List[StrategyGenome], scores: List[float]
    ) -> List[StrategyGenome]:
        """Evaluate generation and return next population (elite + tournament winners)."""
        self.generation += 1

        scored = sorted(zip(scores, population), key=lambda x: x[0], reverse=True)

        next_pop = []

        for score, genome in scored[:3]:
            print(
                f"  Gen {self.generation} | Score: {score:>8.4f} | {genome.name} | "
                f"spread={genome.spread_multiplier:.1f}x bid={genome.bid_offset_bps}bps "
                f"fill={genome.fill_probability:.0%} "
                f"kelly={genome.kelly_fraction:.0%} pos={genome.max_positions}"
            )

        for score, genome in scored[:3]:
            if score > self.all_time_best_score:
                self.all_time_best_score = score
                self.all_time_best = copy.deepcopy(genome)
                print(f"  🏆 NEW ALL-TIME BEST: {score:.4f}")

        self._log_generation(scored)

        elite = [copy.deepcopy(g) for _, g in scored[:3]]

        while len(next_pop) < len(population):
            if len(next_pop) < 3:
                next_pop.append(copy.deepcopy(scored[len(next_pop)][1]))
            else:
                winner = self._tournament_select(scored, k=4)
                opponent = self._tournament_select(scored, k=4)
                child = StrategyGenome.crossover(winner, opponent)
                child = child.mutate(rate=0.2)
                next_pop.append(child)

        return next_pop

    def _tournament_select(self, scored: list, k: int = 4):
        """Select one genome via tournament selection."""
        tournament = random.sample(scored, min(k, len(scored)))
        return max(tournament, key=lambda x: x[0])[1]

    def _log_generation(self, scored):
        try:
            with open(self.history_file, "a") as f:
                for score, genome in scored:
                    row = {
                        "generation": self.generation,
                        "score": score,
                        "timestamp": datetime.now().isoformat(),
                        **genome.to_dict(),
                    }
                    f.write(json.dumps(row) + "\n")
        except Exception:
            pass
