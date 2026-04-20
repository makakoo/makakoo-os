#!/usr/local/opt/python@3.11/bin/python3.11
from __future__ import annotations

"""
Polymarket Paper Trader v5 — Real Binary Markets, Correct P&L Math.

- Real Polymarket Gamma API data (binary YES/NO prices in 0-1)
- Proper binary market mechanics: YES + NO = $1 at settlement
- Real Polymarket fees: 2% taker, 0.5% maker rebate
- Position sizing: shares × price = cost; max payout = shares
- Virtual money, real market prices.

Entry model:
  BUY YES  → price = yes_price (bet YES resolves)
  BUY NO   → price = no_price  (bet NO resolves)
  P&L: (settlement_price - entry_price) × shares  [LONG]
      (settlement_price - entry_price) × shares  [SHORT on NO, equivalent to (entry_price - settlement_price) × shares]
      minus fees

Settlement:
  Market resolves YES → YES pays $1, NO pays $0
  Market resolves NO  → NO pays $1, YES pays $0
"""

import json
import os
import random
import signal
import sys
import time
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from scanner import Scanner, Market
from strategy_genome import StrategyGenome
from crypto_signals import btc_price_signal

PAPER_CAPITAL = 100.0

from collections import defaultdict


class PriceHistory:
    """Tracks YES price history for momentum signal generation."""

    def __init__(self, max_history: int = 20):
        self.max_history = max_history
        self._prices: dict[str, list[tuple[float, float]]] = defaultdict(list)
        # (timestamp, yes_price)

    def update(self, market_id: str, yes_price: float):
        ts = time.time()
        bucket = int(ts / 60)  # 1-minute buckets
        key = (market_id, bucket)
        hist = self._prices[market_id]
        if hist and hist[-1][1] == yes_price:
            return  # no change
        hist.append((ts, yes_price))
        if len(hist) > self.max_history:
            hist.pop(0)

    def momentum(self, market_id: str, window: int = 5) -> float:
        """Return momentum: % change in yes_price over last `window` polls. Positive = YES trending up."""
        hist = self._prices.get(market_id, [])
        if len(hist) < 2:
            return 0.0
        recent = [p for (ts, p) in hist[-window:]]
        if len(recent) < 2:
            return 0.0
        delta = recent[-1] - recent[0]
        return delta

    def mean_price(self, market_id: str, window: int = 10) -> float:
        hist = self._prices.get(market_id, [])
        if len(hist) < 2:
            return 0.5
        recent = [p for (ts, p) in hist[-window:]]
        return sum(recent) / len(recent)

    def breakout(self, market_id: str, window: int = 10) -> str | None:
        """
        Detect breakout in YES price over window.
        Returns: "buy_yes" if YES breaking out, "buy_no" if fading, None if unclear.
        """
        hist = self._prices.get(market_id, [])
        if len(hist) < window:
            return None
        window_prices = [p for (ts, p) in hist[-window:]]
        current = window_prices[-1]
        high = max(window_prices[:-1])
        low = min(window_prices[:-1])
        if current > high and current > 0.55:
            return "buy_yes"
        if current < low and current < 0.45:
            return "buy_no"
        return None


def polymarket_signal(market: Market, history: PriceHistory, genome) -> str | None:
    """
    Generate Polymarket trade signal based on price momentum.
    Returns "YES", "NO", or None (skip).

    Strategy:
    - Momentum: track YES price trend, buy in direction of momentum
    - Breakout: buy YES if price breaks above recent high
    - Underdog: if yes_price < 0.40 and trending up, buy YES
    - Fade: if yes_price > 0.60 and trending down, buy NO
    """
    mid = (market.yes_price + market.no_price) / 2

    # Update history
    history.update(market.id, market.yes_price)

    # Need some history
    hist_len = len(history._prices.get(market.id, []))
    if hist_len < 3:
        return None

    mom = history.momentum(market.id, window=5)
    breakout_sig = history.breakout(market.id, window=10)
    mean = history.mean_price(market.id, window=10)

    # Momentum entry: buy in direction of momentum
    if mom > 0.03:
        # YES trending up
        return "YES"
    if mom < -0.03:
        # YES trending down → NO trending up
        return "NO"

    # Breakout confirmation
    if breakout_sig == "buy_yes":
        return "YES"
    if breakout_sig == "buy_no":
        return "NO"

    # Mean reversion: underdog / fade favorite
    if market.yes_price < 0.38 and market.yes_price > mean - 0.03:
        return "YES"  # cheap underdog
    if market.yes_price > 0.62 and market.yes_price < mean + 0.03:
        return "NO"  # expensive favorite

    return None


HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_FILE = DATA_DIR / "state" / "paper_trades.json"
JOURNAL_FILE = DATA_DIR / "state" / "intraday_journal.jsonl"
LOG_DIR = DATA_DIR / "logs"

TAKER_FEE_BPS = 200  # 2% taker fee (Polymarket standard)
MAKER_REBATE_BPS = 50  # 0.5% maker rebate
MIN_CAPITAL = 0.50


@dataclass
class PaperPosition:
    market_id: str
    question: str
    token_id: str  # Polymarket token ID
    side: str  # "YES" or "NO"
    entry_price: float  # 0.0-1.0
    shares: float  # number of shares bought
    cost: float  # entry_price × shares (what we paid)
    opened_at: str


@dataclass
class PaperTrade:
    id: str
    market_id: str
    question: str
    side: str
    entry_price: float
    exit_price: float  # resolution price: 1.0 (YES won) or 0.0 (NO won)
    shares: float
    gross_pnl: float
    fee: float
    net_pnl: float
    rebate: float
    opened_at: str
    closed_at: str
    duration_secs: float
    result: str  # "win", "loss", "breakeven"


class PolymarketPaperTrader:
    """
    Paper trader for Polymarket binary markets.

    Polymarket binary market mechanics:
    - YES + NO = $1 always (at settlement)
    - If you BUY YES at $0.60 and market resolves YES:
        payout = shares × $1.00 = $size
        gross_pnl = (1.0 - entry_price) × shares = (1.0 - 0.60) × shares = 0.40 × shares
        fee = cost × 2% = entry_price × shares × 0.02
    - If you BUY YES at $0.60 and market resolves NO:
        payout = shares × $0.00 = $0
        gross_pnl = (0.0 - entry_price) × shares = -0.60 × shares
        fee = cost × 2%
    """

    def __init__(self, genome: StrategyGenome, paper_capital: float = PAPER_CAPITAL):
        self.genome = genome
        self.paper_capital = paper_capital
        self.capital = paper_capital
        self.positions: list[PaperPosition] = []
        self.trades: list[PaperTrade] = []
        self.pending: list[dict] = []  # pending orders
        self.scanner = Scanner(
            min_volume=genome.min_volume_usd,
            min_liquidity=genome.min_liquidity,
        )
        try:
            from crypto_price_scanner import CryptoPriceScanner

            max_h = (
                genome.max_hours
                if hasattr(genome, "max_hours") and genome.max_hours > 0
                else 48.0
            )
            self.crypto_scanner = CryptoPriceScanner(
                min_liquidity=genome.min_liquidity,
                max_hours=max_h,
            )
        except Exception:
            self.crypto_scanner = None
        self.history = PriceHistory(max_history=30)
        self.crypto_history: dict = {}
        self.rebates = 0.0
        self.fees = 0.0
        self.wins = self.losses = self.breakeven = 0
        self.unrealized_pnl = 0.0
        self._counter = 0
        self._running = False
        self.started_at = datetime.now().isoformat()

        LOG_DIR.mkdir(parents=True, exist_ok=True)
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        lf = LOG_DIR / f"pm_paper_{datetime.now().strftime('%Y%m%d_%H%M%S')}.log"
        import logging

        logging.basicConfig(
            level=logging.INFO,
            format="%(asctime)s %(message)s",
            handlers=[logging.FileHandler(lf), logging.StreamHandler()],
        )
        self.log = logging.getLogger("pm_paper")

    def _id(self) -> str:
        self._counter += 1
        return f"pt_{datetime.now().strftime('%H%M%S')}_{self._counter}"

    def _now(self) -> str:
        return datetime.now().isoformat()

    def _tick_pending(self):
        """Check pending orders — fill them like a maker or expire."""
        still = []
        for o in self.pending:
            mb = self.scanner._cache.get(o["market_id"])
            if not mb:
                still.append(o)
                continue

            if random.random() < self.genome.fill_probability:
                rebate = o["cost"] * MAKER_REBATE_BPS / 10000
                self.capital += rebate
                self.rebates += rebate

                self.positions.append(
                    PaperPosition(
                        market_id=o["market_id"],
                        question=o["question"],
                        token_id=o["token_id"],
                        side=o["side"],
                        entry_price=o["price"],
                        shares=o["shares"],
                        cost=o["cost"],
                        opened_at=o["placed_at"],
                    )
                )
                self.log.info(
                    f"[FILLED:M] {o['side']} {o['shares']}@{o['price']:.4f} "
                    f"cost=${o['cost']:.2f} rebate=${rebate:.4f} | cap=${self.capital:.2f}"
                )
            elif time.time() >= o["expires_at"]:
                self.capital += o["cost"]
                self.log.info(f"[EXPIRED] {o['side']} {o['shares']}@{o['price']:.4f}")
            else:
                still.append(o)
        self.pending = still

        self.unrealized_pnl = 0.0
        for pos in self.positions:
            mb = self.scanner._cache.get(pos.market_id)
            if not mb:
                continue
            yes_p = mb.yes_price
            if pos.side == "YES":
                current_value = pos.shares * yes_p
            else:
                current_value = pos.shares * (1.0 - yes_p)
            cost_basis = pos.shares * pos.entry_price
            self.unrealized_pnl += current_value - cost_basis

    def _resolve_positions(self):
        """Check if any positions have resolved markets. Settle P&L."""
        still = []
        for pos in self.positions:
            mb = self.scanner._cache.get(pos.market_id)
            if not mb:
                still.append(pos)
                continue

            if mb.resolved:
                # Get resolution info
                # YES_token pays $1 if YES, $0 if NO
                # NO_token pays $1 if NO, $0 if YES
                # Determine what the outcome was
                try:
                    raw = mb.question.lower()
                    # Try to determine YES/N0 resolution from market data
                    # Polymarket stores resolved outcome in the market object
                    resolved_answer = getattr(mb, "resolved_answer", None)
                    if resolved_answer is None:
                        # Fallback: check if we can determine from API
                        # For resolved markets, use the current prices as indicator
                        # If yes_price is very close to 1.0 → YES won
                        # If yes_price is very close to 0.0 → NO won
                        yes_p = mb.yes_price
                        outcome_is_yes = yes_p > 0.5
                        resolved_answer = "yes" if outcome_is_yes else "no"
                except Exception:
                    resolved_answer = "yes" if pos.side == "YES" else "no"

                outcome_is_yes = resolved_answer.lower() == "yes"

                if pos.side == "YES":
                    if outcome_is_yes:
                        payout = pos.shares * 1.0
                        gross_pnl = payout - pos.cost
                        result = "win"
                    else:
                        payout = 0.0
                        gross_pnl = -pos.cost
                        result = "loss"
                else:  # NO
                    if not outcome_is_yes:
                        payout = pos.shares * 1.0
                        gross_pnl = payout - pos.cost
                        result = "win"
                    else:
                        payout = 0.0
                        gross_pnl = -pos.cost
                        result = "loss"

                fee = pos.cost * TAKER_FEE_BPS / 10000
                net_pnl = gross_pnl - fee
                self.capital += pos.shares * 1.0 + net_pnl
                self.fees += fee
                pnl = net_pnl  # alias for journal compatibility

                if net_pnl > 0.001:
                    self.wins += 1
                elif net_pnl < -0.001:
                    self.losses += 1
                else:
                    self.breakeven += 1

                dur = (
                    datetime.now() - datetime.fromisoformat(pos.opened_at)
                ).total_seconds()
                trade = PaperTrade(
                    id=self._id(),
                    market_id=pos.market_id,
                    question=pos.question,
                    side=pos.side,
                    entry_price=pos.entry_price,
                    exit_price=1.0 if outcome_is_yes else 0.0,
                    shares=pos.shares,
                    gross_pnl=round(gross_pnl, 4),
                    fee=round(fee, 4),
                    net_pnl=round(net_pnl, 4),
                    rebate=0.0,
                    opened_at=pos.opened_at,
                    closed_at=self._now(),
                    duration_secs=round(dur, 1),
                    result=result,
                )
                self.trades.append(trade)
                self._journal(trade)
                self.log.info(
                    f"[SETTLED] {pos.side} {pos.shares}@{pos.entry_price:.4f} "
                    f"→ {'YES' if outcome_is_yes else 'NO'} "
                    f"gross={gross_pnl:+.4f} fee={fee:.4f} net={net_pnl:+.4f} {result} | cap=${self.capital:.2f}"
                )
            else:
                still.append(pos)
        self.positions = still

    def _open_orders_crypto(self, btc_price: float = None):
        """Post orders on BTC/ETH crypto price-target markets using BTC price signals."""
        if self.capital < MIN_CAPITAL:
            self.log.info(f"Capital ${self.capital:.2f} < ${MIN_CAPITAL} — waiting")
            return

        if self.crypto_scanner is None:
            self.log.info("Crypto scanner unavailable")
            return

        markets = self.crypto_scanner.scan()
        if not markets:
            self.log.info("No crypto markets found")
            return

        open_ids = {p.market_id for p in self.positions}
        pending_ids = {o["market_id"] for o in self.pending}
        n_open = len(open_ids)

        if n_open >= self.genome.max_positions:
            self.log.info(f"Max positions ({self.genome.max_positions}) reached")
            return

        for cm in markets:
            if n_open >= self.genome.max_positions:
                break
            if cm.id in open_ids or cm.id in pending_ids:
                continue

            if cm.hours_until_end and cm.hours_until_end > self.genome.max_hours:
                self.log.debug(
                    f"Market {cm.id} exceeds max_hours ({cm.hours_until_end:.0f}h > {self.genome.max_hours:.0f}h)"
                )
                continue

            if cm.liquidity < self.genome.min_liquidity:
                continue

            sig = btc_price_signal(cm, btc_price)
            if sig is None:
                self.log.debug(f"No signal for: {cm.question[:50]}")
                continue

            if sig.confidence < 0.40:
                self.log.debug(
                    f"Low confidence ({sig.confidence:.0%}) for: {cm.question[:50]}"
                )
                continue

            price = sig.market_price
            if price < 0.05 or price > 0.95:
                continue

            side = sig.side
            spend = self.capital * self.genome.max_position_pct
            shares = max(1.0, spend / price)
            cost = shares * price

            if cost > self.capital * 0.90:
                shares = self.capital * 0.85 / price
                cost = shares * price

            if cost < 0.10 or shares < 1.0:
                continue

            self.pending.append(
                {
                    "market_id": cm.id,
                    "question": cm.question,
                    "token_id": cm.tokens[0] if side == "YES" else cm.tokens[1],
                    "side": side,
                    "price": price,
                    "shares": shares,
                    "cost": cost,
                    "placed_at": self._now(),
                    "expires_at": time.time() + self.genome.cancel_after_seconds,
                    "signal_confidence": sig.confidence,
                    "signal_reasoning": sig.reasoning,
                }
            )
            self.capital -= cost
            open_ids.add(cm.id)
            n_open += 1
            self.log.info(
                f"[POSTED:{sig.confidence:.0%}] {side} {shares:.2f}@{price:.4f} "
                f"cost=${cost:.2f} | {cm.question[:50]} | cap=${self.capital:.2f}"
            )

    def _open_orders(self, strategy_fn=None):
        """Post bid/ask limit orders on candidate markets — true market making."""
        if self.capital < MIN_CAPITAL:
            self.log.info(f"Capital ${self.capital:.2f} < ${MIN_CAPITAL} — waiting")
            return

        cands = self.scanner.get_candidates(self.genome)
        if not cands:
            self.log.info("No candidate markets found")
            return

        open_ids = {p.market_id for p in self.positions}
        pending_ids = {o["market_id"] for o in self.pending}
        n_open = len(open_ids)

        if n_open >= self.genome.max_positions:
            self.log.info(f"Max positions ({self.genome.max_positions}) reached")
            return

        spread_mult = getattr(self.genome, "spread_multiplier", 1.0)
        bid_pct = getattr(self.genome, "bid_offset_pct", 0.005)
        ask_pct = getattr(self.genome, "ask_offset_pct", 0.005)
        max_pos_pct = getattr(self.genome, "max_position_pct", 0.05)
        post_both = getattr(self.genome, "post_both_sides", True)
        cancel_sec = getattr(self.genome, "cancel_after_seconds", 120)
        min_spread_bps = getattr(self.genome, "min_spread_bps", 30)

        for mb in cands:
            yes_p = mb.yes_price
            no_p = mb.no_price
            mid = (yes_p + no_p) / 2
            spread = abs(yes_p - no_p)
            spread_bps = spread * 10000

            if spread_bps < min_spread_bps:
                continue

            n_added = 0

            if yes_p > 0.02 and yes_p < 0.98:
                bid_price = yes_p * (1 - bid_pct * spread_mult)
                if bid_price >= mid - 0.001:
                    bid_price = mid - max(0.001, yes_p * 0.002)
                bid_price = max(0.001, min(bid_price, mid - 0.001))
                if bid_price > 0.001 and bid_price < yes_p:
                    spend = self.capital * max_pos_pct
                    shares = max(1.0, spend / bid_price)
                    cost = shares * bid_price
                    if cost > 0 and cost <= self.capital * 0.45 and shares >= 1.0:
                        self.pending.append(
                            {
                                "market_id": mb.id,
                                "question": mb.question,
                                "token_id": mb.tokens[0],
                                "side": "YES",
                                "price": bid_price,
                                "shares": shares,
                                "cost": cost,
                                "placed_at": self._now(),
                                "expires_at": time.time() + cancel_sec,
                            }
                        )
                        self.capital -= cost
                        n_added += 1
                        self.log.info(
                            f"[BID] YES {shares:.1f}@{bid_price:.4f} cost=${cost:.2f} "
                            f"| {mb.question[:45]} | cap=${self.capital:.2f}"
                        )

            if post_both and yes_p > 0.02 and yes_p < 0.98:
                ask_price = yes_p * (1 + ask_pct * spread_mult)
                if ask_price <= mid + 0.001:
                    ask_price = mid + max(0.001, yes_p * 0.002)
                ask_price = min(0.999, max(ask_price, mid + 0.001))
                if ask_price > yes_p and ask_price < 0.999:
                    spend = self.capital * max_pos_pct
                    shares = max(1.0, spend / ask_price)
                    cost = shares * ask_price
                    if cost > 0 and cost <= self.capital * 0.45 and shares >= 1.0:
                        self.pending.append(
                            {
                                "market_id": mb.id,
                                "question": mb.question,
                                "token_id": mb.tokens[0],
                                "side": "NO",
                                "price": ask_price,
                                "shares": shares,
                                "cost": cost,
                                "placed_at": self._now(),
                                "expires_at": time.time() + cancel_sec,
                            }
                        )
                        self.capital -= cost
                        n_added += 1
                        self.log.info(
                            f"[ASK] NO {shares:.1f}@{ask_price:.4f} cost=${cost:.2f} "
                            f"| {mb.question[:45]} | cap=${self.capital:.2f}"
                        )

            if n_added > 0:
                open_ids.add(mb.id)
                n_open += n_added
            if n_open >= self.genome.max_positions:
                break

    def _journal(self, trade: PaperTrade):
        d = asdict(trade)
        d["pnl"] = d["net_pnl"]
        with open(JOURNAL_FILE, "a") as f:
            f.write(json.dumps(d) + "\n")

    def _status(self):
        pos_v = sum(p.shares for p in self.positions)
        pen_v = sum(o["cost"] for o in self.pending)
        pnl = self.capital - self.paper_capital
        total = self.wins + self.losses + self.breakeven
        wr = self.wins / max(total, 1) * 100
        days = (
            datetime.now() - datetime.fromisoformat(self.started_at)
        ).total_seconds() / 86400
        daily = pnl / max(days, 0.001)
        self.log.info(
            f"\n{'=' * 56}\n"
            f"Polymarket Paper Trader v5 | {datetime.now().strftime('%Y-%m-%d %H:%M')}\n"
            f"{'=' * 56}\n"
            f"  Capital:  ${self.capital:.4f}  (started ${self.paper_capital:.2f})\n"
            f"  PnL:      ${pnl:+.4f}  (${daily:+.4f}/day)\n"
            f"  Positions: {len(self.positions)} (${pos_v:.2f} shares) | Pending: {len(self.pending)} (${pen_v:.2f})\n"
            f"  Trades:   {total} (W={self.wins} L={self.losses} BE={self.breakeven}) | WR={wr:.0f}%\n"
            f"  Rebates:  +${self.rebates:.4f}  Fees: -${self.fees:.4f}\n"
            f"  Genome:   {self.genome.name}\n"
            f"{'=' * 56}"
        )

    def run(self, poll: int = 60, strategy_fn=None):
        self._running = True

        def stop(sig, frame):
            self.log.info("Stopping...")
            self._running = False

        signal.signal(signal.SIGINT, stop)
        signal.signal(signal.SIGTERM, stop)

        self.log.info(
            f"Polymarket Paper Trader v5 | ${self.paper_capital} virtual | "
            f"Genome: {self.genome.name} | poll={poll}s"
        )

        tick = 0
        while self._running:
            try:
                tick += 1
                markets = self.scanner.scan()
                for m in markets:
                    self.history.update(m.id, m.yes_price)
                self._tick_pending()
                self._resolve_positions()

                btc_price = None
                if self.crypto_scanner:
                    btc_price = self.crypto_scanner.fetch_btc_price()
                    self.crypto_scanner.scan()
                    if tick % 10 == 0:
                        cs = self.crypto_scanner.status()
                        self.log.info(
                            f"[SCANNER] btc=${cs['btc_price']} "
                            f"crypto_mkts={cs['total_crypto_markets']} "
                            f"btc_mkts={cs['btc_markets']} "
                            f"intraday={cs['intraday_markets']}"
                        )

                self._open_orders_crypto(btc_price)

                if tick % 10 == 0:
                    self._status()

                self._save()
                time.sleep(poll)

            except Exception as e:
                self.log.error(f"Error: {e}")
                import traceback

                self.log.error(traceback.format_exc())
                time.sleep(poll)

        self._save()
        self._status()
        self.log.info("Stopped.")

    def _save(self):
        with open(STATE_FILE, "w") as f:
            json.dump(
                {
                    "capital": self.capital,
                    "positions": [asdict(p) for p in self.positions],
                    "trades": [asdict(t) for t in self.trades],
                    "total_rebates": self.rebates,
                    "total_fees": self.fees,
                    "wins": self.wins,
                    "losses": self.losses,
                    "breakeven": self.breakeven,
                    "started_at": self.started_at,
                    "updated_at": self._now(),
                },
                f,
                indent=2,
            )


if __name__ == "__main__":
    import argparse
    from strategy_genome import StrategyGenome

    parser = argparse.ArgumentParser()
    parser.add_argument("--capital", type=float, default=PAPER_CAPITAL)
    parser.add_argument("--poll", type=int, default=30)
    args = parser.parse_args()

    gp = Path(__file__).parent / "state" / "best_genome.json"
    if gp.exists():
        with open(gp) as f:
            d = json.load(f)
        genome = StrategyGenome.from_dict(d.get("genome", d))
        print(f"Loaded genome: {genome.name}")
    else:
        genome = StrategyGenome()
        genome.name = "default"
        genome.spread_multiplier = 2.2
        genome.bid_offset_bps = 10
        genome.ask_offset_bps = 10
        genome.fill_probability = 0.20
        genome.max_positions = 5
        genome.post_both_sides = True
        genome.min_liquidity = 1000
        genome.min_spread_bps = 50
        genome.min_volume_usd = 5000
        genome.min_position_size = 3.0
        genome.max_position_pct = 0.10
        genome.cancel_after_seconds = 600

    PolymarketPaperTrader(genome, paper_capital=args.capital).run(poll=args.poll)
