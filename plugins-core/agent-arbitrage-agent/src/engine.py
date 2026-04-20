"""
Simulation Engine — models Polymarket trading with realistic fee structure,
order fills, maker rebates, and taker fees.

Key insight: TAKERS lose money (2% fee + spread). MAKERS earn money (0% fee + rebates).
"""

import random
import math
import json
from dataclasses import dataclass, field
from typing import Dict, List, Optional
from datetime import datetime, timedelta
from scanner import Market, Scanner
from strategy_genome import StrategyGenome


MAKER_REBATE_BPS = 1.0  # 0.01% maker rebate
TAKER_FEE_BPS = (
    20.0  # 0.2% taker fee... actually Polymarket is 0% maker / 0.5% taker on AMM
)
# Real Polymarket fees: 0% for makers, 0.5% for takers


@dataclass
class Position:
    market_id: str
    token_id: str
    side: str  # "YES" or "NO"
    entry_price: float
    size: float  # number of shares
    value: float  # cost = price * size
    timestamp: datetime
    filled: bool = False
    resolved: bool = False
    outcome: Optional[str] = None


@dataclass
class SimTrade:
    market_id: str
    question: str
    side: str
    entry_price: float
    exit_price: float
    size: float
    pnl: float
    fees: float
    timestamp: datetime
    duration_seconds: float
    strategy: str


@dataclass
class SimulationState:
    capital: float
    day_start_capital: float
    positions: List[Position]
    trades: List[SimTrade]
    daily_pnl: float
    wins: int
    losses: int
    breakeven: int
    total_fees: float
    total_rebates: float
    total_volume: float
    orders_placed: int
    orders_filled: int
    orders_cancelled: int


class SimulationEngine:
    """
    Realistic simulation of Polymarket market-making + arbitrage.

    Models:
    - Maker orders: placed inside spread, get filled probabilistically
    - Taker orders: cross the spread, pay taker fee
    - Maker rebates: earn on filled volume
    - Cancel logic: orders expire after N seconds
    """

    def __init__(self, initial_capital: float = 100.0):
        self.capital = initial_capital
        self.initial_capital = initial_capital
        self.state = SimulationState(
            capital=initial_capital,
            day_start_capital=initial_capital,
            positions=[],
            trades=[],
            daily_pnl=0.0,
            wins=0,
            losses=0,
            breakeven=0,
            total_fees=0.0,
            total_rebates=0.0,
            total_volume=0.0,
            orders_placed=0,
            orders_filled=0,
            orders_cancelled=0,
        )
        self.scanner = Scanner()
        self.markets: Dict[str, Market] = {}
        self.pending_orders: List[Dict] = []
        self._tick = 0
        self._day_start = datetime.now()

    def reset(self):
        """Reset to initial capital and clear state."""
        self.capital = self.initial_capital
        self.state = SimulationState(
            capital=self.initial_capital,
            day_start_capital=self.initial_capital,
            positions=[],
            trades=[],
            daily_pnl=0.0,
            wins=0,
            losses=0,
            breakeven=0,
            total_fees=0.0,
            total_rebates=0.0,
            total_volume=0.0,
            orders_placed=0,
            orders_filled=0,
            orders_cancelled=0,
        )
        self.markets = {}
        self.pending_orders = []
        self._tick = 0
        self._day_start = datetime.now()

    def simulate_tick(self, genome: StrategyGenome, poll_interval: int = 5) -> Dict:
        """
        One simulation tick: scan markets, place orders, process fills/cancels.
        Returns dict with tick stats.
        """
        self._tick += 1
        tick_time = datetime.now()
        stats = {
            "tick": self._tick,
            "capital": self.capital,
            "pending_orders": len(self.pending_orders),
            "positions": len(self.state.positions),
            "trades": len(self.state.trades),
            "orders_placed": 0,
            "orders_filled": 0,
            "orders_cancelled": 0,
        }

        if self._tick % 5 == 1:
            self.markets = {m.id: m for m in self.scanner.scan()}

        max_pos = genome.max_positions
        if len(self.state.positions) >= max_pos:
            pass
        elif len(self.pending_orders) < genome.max_positions * 2:
            placed = self._place_orders(genome, tick_time)
            stats["orders_placed"] = placed

        filled, cancelled = self._process_orders(genome, tick_time, poll_interval)
        stats["orders_filled"] = filled
        stats["orders_cancelled"] = cancelled
        stats["capital"] = self.capital

        return stats

    def _place_orders(self, genome: StrategyGenome, tick_time: datetime) -> int:
        """Place maker orders on available markets. Returns count of orders placed."""
        placed = 0
        capital = self.capital

        for mid, market in self.markets.items():
            if market.resolved:
                continue
            if market.liquidity < genome.min_liquidity:
                continue
            if market.yes_price < 0.02 or market.yes_price > 0.98:
                continue

            spread = market.spread_pct
            if spread * 10000 < genome.min_spread_bps:
                continue

            existing = [p for p in self.state.positions if p.market_id == mid]
            if len(existing) >= 2:
                continue

            mid_price = market.mid_price

            bid_price = mid_price - (
                spread * genome.spread_multiplier * genome.bid_offset_bps / 100
            )
            ask_price = mid_price + (
                spread * genome.spread_multiplier * genome.ask_offset_bps / 100
            )
            bid_price = max(bid_price, 0.001)
            ask_price = min(ask_price, 0.999)

            max_pos_val = capital * genome.max_position_pct
            max_pos_val = min(max_pos_val, genome.max_position_size)
            max_pos_val = max(max_pos_val, genome.min_position_size)

            if genome.post_both_sides and len(existing) == 0:
                yes_possible = max(1, int(max_pos_val / bid_price))
                no_possible = max(1, int(max_pos_val / ask_price))

                for side, price, size in [
                    ("YES", bid_price, yes_possible),
                    ("NO", ask_price, no_possible),
                ]:
                    value = price * size
                    if value > capital * 0.95:
                        continue
                    token_id = market.tokens[0] if side == "YES" else market.tokens[1]
                    self.pending_orders.append(
                        {
                            "market_id": mid,
                            "question": market.question,
                            "token_id": token_id,
                            "side": side,
                            "price": price,
                            "size": size,
                            "value": value,
                            "timestamp": tick_time,
                            "expires_at": tick_time.timestamp()
                            + genome.cancel_after_seconds,
                            "genome_name": genome.name,
                        }
                    )
                    capital -= value
                    self.state.orders_placed += 1
                    placed += 1

            elif len(existing) == 0:
                side = "YES" if market.yes_price < 0.50 else "NO"
                price = bid_price if side == "YES" else ask_price
                size = max(1, int(max_pos_val / price))
                value = price * size
                if value <= capital * 0.95:
                    token_id = market.tokens[0] if side == "YES" else market.tokens[1]
                    self.pending_orders.append(
                        {
                            "market_id": mid,
                            "question": market.question,
                            "token_id": token_id,
                            "side": side,
                            "price": price,
                            "size": size,
                            "value": value,
                            "timestamp": tick_time,
                            "expires_at": tick_time.timestamp()
                            + genome.cancel_after_seconds,
                            "genome_name": genome.name,
                        }
                    )
                    capital -= value
                    self.state.orders_placed += 1
                    placed += 1

        self.capital = capital
        return placed

    def _process_orders(
        self, genome: StrategyGenome, tick_time: datetime, poll_interval: int
    ) -> tuple:
        """Process pending orders: fills or cancels. Returns (filled, cancelled)."""
        filled = 0
        cancelled = 0
        now_ts = tick_time.timestamp()

        still_pending = []
        for order in self.pending_orders:
            if now_ts >= order["expires_at"]:
                self.capital += order["value"]
                self.state.orders_cancelled += 1
                cancelled += 1
                continue

            market = self.markets.get(order["market_id"])
            if not market:
                still_pending.append(order)
                continue

            fill_prob = genome.fill_probability * genome.spread_multiplier * 0.5
            fill_prob = min(fill_prob, 0.90)

            if random.random() < fill_prob:
                rebate = order["value"] * MAKER_REBATE_BPS / 10000
                self.capital += order["value"]
                self.capital += rebate
                self.state.total_rebates += rebate
                self.state.total_volume += order["value"]

                self.state.positions.append(
                    Position(
                        market_id=order["market_id"],
                        token_id=order["token_id"],
                        side=order["side"],
                        entry_price=order["price"],
                        size=order["size"],
                        value=order["value"],
                        timestamp=tick_time,
                        filled=True,
                    )
                )
                self.state.orders_filled += 1
                filled += 1
            else:
                still_pending.append(order)

        self.pending_orders = still_pending
        return filled, cancelled

    def score_genome(
        self, genome: StrategyGenome, duration_minutes: int = 120
    ) -> float:
        """
        Run a full simulation with this genome and return the final score.

        Score = (total_rebates / initial_capital) * 100
        Rebates are the primary measurable PnL for market makers on Polymarket.
        We do NOT use capital balance as score (capital gets locked in pending orders,
        making capital-based ROI misleading for market-making strategies).
        """
        self.reset()
        ticks = (duration_minutes * 60) // 5
        poll_interval = 5

        print(f"\n  Running {duration_minutes}m simulation for {genome.name}...")
        print(
            f"  Params: spread={genome.spread_multiplier:.1f}x "
            f"bid={genome.bid_offset_bps}bps ask={genome.ask_offset_bps}bps "
            f"fill={genome.fill_probability:.0%} kelly={genome.kelly_fraction:.0%} "
            f"max_pos={genome.max_positions} both_sides={genome.post_both_sides}"
        )

        for tick in range(ticks):
            stats = self.simulate_tick(genome, poll_interval)

            if tick > 0 and tick % 240 == 0:
                elapsed_min = tick * 5 // 60
                locked = sum(o["value"] for o in self.pending_orders)
                print(
                    f"  [{elapsed_min:>3}m] Cap=${stats['capital']:.2f} "
                    f"(locked=${locked:.2f}) | Filled={stats['orders_filled']} "
                    f"Cancel={stats['orders_cancelled']} | "
                    f"Rebates=${self.state.total_rebates:.4f}"
                )

        total_rebates = self.state.total_rebates
        orders_filled = self.state.orders_filled
        orders_cancelled = self.state.orders_cancelled
        total_placed = self.state.orders_placed

        score = (total_rebates / self.initial_capital) * 100

        print(
            f"  Result: Rebates=${total_rebates:.6f} | "
            f"Capital=${self.capital:.2f} | "
            f"Filled={orders_filled}/{total_placed} "
            f"(cancel={orders_cancelled}) | "
            f"SCORE={score:.6f}"
        )

        return round(score, 6)

    def get_stats(self) -> Dict:
        return {
            "capital": self.capital,
            "initial_capital": self.initial_capital,
            "pnl": self.capital - self.initial_capital,
            "roi": (self.capital - self.initial_capital) / self.initial_capital,
            "total_trades": len(self.state.trades),
            "total_orders_placed": self.state.orders_placed,
            "total_orders_filled": self.state.orders_filled,
            "total_orders_cancelled": self.state.orders_cancelled,
            "total_rebates": self.state.total_rebates,
            "total_volume": self.state.total_volume,
            "wins": self.state.wins,
            "losses": self.state.losses,
            "breakeven": self.state.breakeven,
        }
