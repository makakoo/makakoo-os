#!/usr/local/opt/python@3.11/bin/python3.11
"""
StrategyGenome v3 — Directional Intraday Trading.

Evolvable params for BTC-momentum × PM-price directional strategy.
"""

import random
import json
import copy
import dataclasses
from dataclasses import dataclass, field, asdict
from typing import Dict, List, Optional
from datetime import datetime


@dataclass
class StrategyGenome:
    version: str = "v3.0"
    name: str = ""

    # BTC momentum thresholds — entry when BTC moves these % on 15m
    bull_threshold: float = 0.05  # % BTC mom to trigger BULL signal
    bear_threshold: float = -0.05  # % BTC mom to trigger BEAR signal

    # PM momentum confirmation (YES price delta per minute)
    pm_bull_threshold: float = 0.001  # YES price rising → confirm BULL
    pm_bear_threshold: float = -0.001  # YES price falling → confirm BEAR

    # Confidence — min_entry_conf gates whether to take a signal
    base_confidence: float = 0.55
    max_confidence: float = 0.85
    min_confidence: float = 0.50  # only enter if conf >= this

    # BTC timeframe weights (composite momentum)
    primary_timeframe: str = "15m"
    use_composite_momentum: bool = False
    btc_weight_1m: float = 0.0
    btc_weight_5m: float = 0.30
    btc_weight_15m: float = 0.40
    btc_weight_1h: float = 0.30

    # BTC as filter — skip entries when 1h opposes
    use_btc_filter: bool = True
    btc_filter_threshold: float = 0.5
    btc_bear_multiplier: float = 0.50

    # Market filters
    min_volume_24h: float = 40000  # only trade high-volume markets
    max_entry_price: float = 0.70
    min_entry_price: float = 0.05

    # RSI filter (BTC)
    use_rsi_filter: bool = True
    rsi_overbought: float = 70
    rsi_oversold: float = 30

    # RSI mean reversion (DISABLED)
    use_rsi_reversion: bool = False
    rsi_reversion_buy_threshold: float = 30
    rsi_reversion_sell_threshold: float = 70

    # Position sizing
    max_position_pct: float = (
        0.15  # 15% of capital per trade (bigger = fees matter less)
    )
    min_position_pct: float = 0.05  # minimum to make fees worthwhile
    max_positions: int = 2  # max 2 concurrent
    kelly_fraction: float = 0.25

    # Exit rules — wider for PM (moves 3-10% in a few minutes)
    profit_target_pct: float = 0.06  # 6% target — covers fees + profit
    stop_loss_pct: float = 0.04  # 4% max loss
    max_hold_hours: float = 2.0  # force exit after 2h

    # Session / polling
    poll_seconds: int = 10
    session_minutes: int = 30  # 30min sessions — need time to get trades

    # Minimum trade bar — genomes below this are penalized
    min_trades_per_session: int = 3

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
        version = d_copy.pop("version", "v3.0")
        known = {f.name for f in cls.__dataclass_fields__.values()}
        for k in list(d_copy.keys()):
            if k not in known:
                d_copy.pop(k)
        return cls(version=version, **d_copy)

    def mutate(self, rate: float = 0.25) -> "StrategyGenome":
        """Create a mutated copy of this genome."""
        new = copy.deepcopy(self)
        new.name = (
            f"gen_{datetime.now().strftime('%H%M%S')}_{random.randint(1000, 9999)}"
        )

        mutations = [
            ("bull_threshold", 0.02, 0.20, 0.01),
            ("bear_threshold", -0.20, -0.02, 0.01),
            ("pm_bull_threshold", 0.0002, 0.005, 0.0002),
            ("pm_bear_threshold", -0.005, -0.0002, 0.0002),
            ("base_confidence", 0.45, 0.75, 0.05),
            ("max_confidence", 0.70, 0.95, 0.05),
            ("min_confidence", 0.30, 0.60, 0.05),
            ("btc_weight_1m", 0.0, 0.5, 0.05),
            ("btc_weight_5m", 0.0, 0.5, 0.05),
            ("btc_weight_15m", 0.0, 0.5, 0.05),
            ("btc_weight_1h", 0.0, 0.5, 0.05),
            ("btc_filter_threshold", 0.2, 2.0, 0.10),
            ("btc_bear_multiplier", 0.1, 1.0, 0.05),
            ("min_volume_24h", 20000, 100000, 5000),
            ("max_entry_price", 0.40, 0.90, 0.02),
            ("min_entry_price", 0.02, 0.30, 0.02),
            ("rsi_overbought", 60, 85, 2),
            ("rsi_oversold", 15, 45, 2),
            ("max_position_pct", 0.08, 0.25, 0.02),
            ("min_position_pct", 0.03, 0.15, 0.01),
            ("max_positions", 1, 3, 1),
            ("kelly_fraction", 0.10, 0.50, 0.05),
            ("profit_target_pct", 0.03, 0.15, 0.01),
            ("stop_loss_pct", 0.02, 0.08, 0.01),
            ("max_hold_hours", 0.5, 4.0, 0.5),
            ("poll_seconds", 5, 60, 5),
        ]

        for attr, lo, hi, step in mutations:
            if random.random() < rate:
                val = round(random.uniform(lo, hi) / step) * step
                setattr(new, attr, val)

        # Fixed: always use primary TF only (no composite) on 15m — avoid confusing signal
        new.use_composite_momentum = False
        new.primary_timeframe = "15m"
        if random.random() < rate:
            new.use_btc_filter = not self.use_btc_filter

        return new

    @classmethod
    def crossover(cls, a: "StrategyGenome", b: "StrategyGenome") -> "StrategyGenome":
        d_a = a.to_dict()
        d_b = b.to_dict()
        keys = list(d_a.keys())
        pt = random.randint(1, len(keys) - 1)
        child_dict = {}
        for i, k in enumerate(keys):
            child_dict[k] = d_a[k] if i < pt else d_b[k]
        child_dict["version"] = "v3.0"
        child_dict["name"] = f"x_{datetime.now().strftime('%H%M%S')}"
        return cls.from_dict(child_dict)

    @classmethod
    def random_population(cls, size: int) -> List["StrategyGenome"]:
        pop = []
        for _ in range(size):
            g = cls()
            g.name = (
                f"init_{datetime.now().strftime('%H%M%S')}_{random.randint(1000, 9999)}"
            )
            g = g.mutate(rate=1.0)
            pop.append(g)
        return pop
