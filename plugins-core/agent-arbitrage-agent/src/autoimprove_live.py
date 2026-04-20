#!/usr/local/opt/python@3.11/bin/python3.11
"""
Auto-Improve Live v3 — BTC-Momentum Directional Polymarket Trading.

Strategy:
  1. Track BTC momentum across timeframes (Binance klines)
  2. Track PM price delta between polls (momentum proxy)
  3. When BTC + PM align → trade the direction
  4. BTC flat → treat all PM markets equally
  5. Genome tunes all thresholds, sizing, exit rules

Loop:
  1. Load/create population from polymarket_best_genome.json
  2. Mutate into POP_SIZE variants
  3. For each genome: run directional paper trader for SESSION_MINUTES
  4. Score by: P&L + win rate + Sharpe-like ratio
  5. Select top genomes, breed next generation
  6. Ask MiniMax for param suggestions based on trade journal
  7. Save best genome; repeat
"""

import copy
import json
import os
import random
import signal
import subprocess
import sys
import time
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Optional

# ── Paths ────────────────────────────────────────────────────────────────────

HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
BEST_GENOME_FILE = STATE_DIR / "polymarket_best_genome.json"
JOURNAL_FILE = STATE_DIR / "intraday_journal.jsonl"
EVOLUTION_LOG = DATA_DIR / "evolution_log.jsonl"
FITNESS_HISTORY = DATA_DIR / "fitness_history.jsonl"

STATE_DIR.mkdir(parents=True, exist_ok=True)
LOG_DIR.mkdir(parents=True, exist_ok=True)

# ── AI config ────────────────────────────────────────────────────────────────

AI_URL = os.environ.get("SWITCHAI_URL", "http://localhost:18080/v1/chat/completions")
AI_KEY = os.environ.get("SWITCHAI_KEY", "sk-test-123")
AI_MODEL = os.environ.get("LLM_MODEL", "minimax:MiniMax-M2.7")

POP_SIZE = int(os.environ.get("POP_SIZE", "16"))
SESSION_MINUTES = int(os.environ.get("SESSION_MINUTES", "30"))
GENERATIONS_PER_RUN = int(os.environ.get("GENERATIONS_PER_RUN", "1"))
PAPER_CAPITAL = float(os.environ.get("PAPER_CAPITAL", "100.0"))

# ── Logging ───────────────────────────────────────────────────────────────────


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    try:
        with open(LOG_DIR / "autoimprove_live.log", "a") as f:
            f.write(line + "\n")
    except:
        pass


# ── AI Helper ─────────────────────────────────────────────────────────────────


def ai_complete(prompt: str, max_tokens: int = 1200) -> str:
    payload = {
        "model": AI_MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "thinking": {"type": "disable"},
        "tools": [],
        "tool_choice": "auto",
    }
    try:
        r = subprocess.run(
            [
                "curl",
                "-s",
                "-X",
                "POST",
                AI_URL,
                "-H",
                "Content-Type: application/json",
                "-H",
                f"Authorization: Bearer {AI_KEY}",
                "-d",
                json.dumps(payload),
                "--max-time",
                "30",
            ],
            capture_output=True,
            text=True,
            timeout=35,
        )
        if r.returncode == 0:
            data = json.loads(r.stdout)
            return data["choices"][0]["message"]["content"]
    except Exception as e:
        log(f"AI error: {e}")
    return ""


# ── BTC Fetcher ───────────────────────────────────────────────────────────────

sys.path.insert(0, str(Path(__file__).parent))
from btc_data_fetcher import BTCDataFetcher, BTCFeatures
from polymarket_data_fetcher import PolymarketFetcher, PMMarket
from strategy_genome_v3 import StrategyGenome


BULL = "BULL"
BEAR = "BEAR"
NEUTRAL = "NEUTRAL"  # constants


class BTCTracker:
    """Tracks BTC momentum across timeframes. Used for both signal gen and filter."""

    def __init__(self):
        self.fetcher = BTCDataFetcher()
        self._features: Optional[BTCFeatures] = None
        self._last_fetch = 0.0

    def refresh(self) -> BTCFeatures:
        now = time.time()
        if now - self._last_fetch < 20:
            return self._features or self.fetcher.features()
        self._features = self.fetcher.features()
        self._last_fetch = now
        return self._features

    def direction(self, genome: StrategyGenome) -> tuple[str, float]:
        """Returns ('BULL'/'BEAR'/'NEUTRAL', momentum_value)."""
        f = self.refresh()
        tf = genome.primary_timeframe

        mom_map = {
            "1m": f.momentum_1m,
            "5m": f.momentum_5m,
            "15m": f.momentum_15m,
            "1h": f.momentum_1h,
        }
        mom = mom_map.get(tf, f.momentum_5m)

        if genome.use_composite_momentum:
            w = genome
            total = (
                w.btc_weight_1m + w.btc_weight_5m + w.btc_weight_15m + w.btc_weight_1h
            )
            mom = (
                f.momentum_1m * w.btc_weight_1m
                + f.momentum_5m * w.btc_weight_5m
                + f.momentum_15m * w.btc_weight_15m
                + f.momentum_1h * w.btc_weight_1h
            ) / total

        if mom > genome.bull_threshold:
            return "BULL", mom
        elif mom < genome.bear_threshold:
            return "BEAR", mom
        else:
            return "NEUTRAL", mom

    def is_oversold(self, genome: StrategyGenome) -> bool:
        f = self.refresh()
        return f.rsi_14 < genome.rsi_oversold

    def is_overbought(self, genome: StrategyGenome) -> bool:
        f = self.refresh()
        return f.rsi_14 > genome.rsi_overbought


# ── PM Price Tracker ──────────────────────────────────────────────────────────


class PMTracker:
    """
    Tracks YES price momentum using a rolling 90-second baseline.
    Delta = current_price - baseline_price (90 seconds ago).
    Delta > 0 = YES trending up (confirms BTC BULL)
    Delta < 0 = YES trending down (confirms BTC BEAR)
    """

    BASELINE_SECONDS = 90  # 90-second baseline for fast confirmation

    def __init__(self):
        self.fetcher = PolymarketFetcher()
        self._price_history: dict[
            str, list[tuple[float, float]]
        ] = {}  # market_id -> [(timestamp, price)]
        self._features: dict[str, dict] = {}

    def poll(self, markets: list[PMMarket]):
        """Poll all markets, compute momentum against 90-sec baseline."""
        now = time.time()
        cutoff = now - self.BASELINE_SECONDS

        for m in markets:
            cur_price = self.fetcher.fetch_current_price(m.yes_token)
            if cur_price <= 0:
                cur_price = m.yes_price

            if m.id not in self._price_history:
                self._price_history[m.id] = []

            history = self._price_history[m.id]
            history.append((now, cur_price))
            history[:] = [(ts, p) for ts, p in history if ts > now - 180]

            baseline_price = history[0][1]
            for ts, p in history:
                if ts >= cutoff:
                    baseline_price = p
                    break

            delta = cur_price - baseline_price
            elapsed = max(now - cutoff, 1)

            self._features[m.id] = {
                "delta": delta,
                "delta_per_sec": delta / elapsed,
                "baseline_price": baseline_price,
                "cur_price": cur_price,
                "elapsed": elapsed,
            }

    def delta(self, market_id: str) -> float:
        return self._features.get(market_id, {}).get("delta", 0.0)

    def delta_per_sec(self, market_id: str) -> float:
        return self._features.get(market_id, {}).get("delta_per_sec", 0.0)

    def current_price(self, market_id: str) -> float:
        history = self._price_history.get(market_id, [])
        return history[-1][1] if history else 0.0


# ── Directional Paper Trader ──────────────────────────────────────────────────

TAKER_FEE_BPS = 200  # 2%
MAKER_REBATE_BPS = 50  # 0.5%


@dataclass
class Trade:
    id: str
    market_id: str
    question: str
    token: str  # Polymarket token ID that was bought (YES or NO)
    yes_token: str  # Always the YES token for this market
    side: str
    entry_price: float
    current_price: float
    shares: float
    cost: float
    opened_at: float  # unix timestamp
    last_update: float  # unix timestamp
    exited: bool = False
    exit_price: float = 0.0
    exit_reason: str = ""
    gross_pnl: float = 0.0
    net_pnl: float = 0.0
    fee: float = 0.0
    result: str = ""


class DirectionalPaperTrader:
    """
    Runs a directional trading session for one genome.

    Signal logic:
    - Get BTC direction (BULL/BEAR/NEUTRAL)
    - Get PM price deltas for top markets
    - Generate signals: BUY YES when BTC=BULL + PM delta positive
                        BUY NO when BTC=BEAR + PM delta negative
                        or BTC=NEUTRAL + strong PM momentum
    - Manage positions with profit targets / stop losses
    - Track P&L, win rate, rebates, fees
    """

    def __init__(self, genome: StrategyGenome, capital: float = PAPER_CAPITAL):
        self.genome = genome
        self.capital = capital
        self.start_capital = capital
        self.trades: list[Trade] = []
        self.wins = self.losses = self.breakeven = 0
        self.total_fees = 0.0
        self.total_rebates = 0.0
        self.btc = BTCTracker()
        self.pm_tracker = PMTracker()
        self._running = False
        self._start_time = 0.0
        self._counter = 0
        self._session_rets: list[float] = []
        self._signals_debug_logged = False

    def _id(self) -> str:
        self._counter += 1
        return f"{datetime.now().strftime('%H%M%S')}_{self._counter}"

    def _now_ts(self) -> float:
        return time.time()

    def _sig_id(self) -> str:
        return f"sig_{self._id()}"

    # ── Signal generation ────────────────────────────────────────────────────

    def _generate_signals(self, markets: list[PMMarket]) -> list[dict]:
        """Generate trade signals from BTC + PM data."""
        btc_dir, btc_mom = self.btc.direction(self.genome)
        btc_f = self.btc.refresh()
        g = self.genome
        signals = []

        if not self._signals_debug_logged:
            log(
                f"  [DEBUG] genome={g.name} use_rsi_rev={g.use_rsi_reversion} rsi_thresh={g.rsi_reversion_buy_threshold} rsi={btc_f.rsi_14:.1f} bull={g.bull_threshold} bear={g.bear_threshold} min_conf={g.min_confidence} max_pos={g.max_positions} min_vol={g.min_volume_24h}"
            )
            self._signals_debug_logged = True

        # Debug: show PM deltas when BTC is directional
        debug_btc = getattr(self, "_last_debug_btc_dir", None)
        if debug_btc is None or debug_btc != btc_dir:
            self._last_debug_btc_dir = btc_dir
            top_deltas = []
            for m in markets[:5]:
                if (
                    m.volume_24h >= g.min_volume_24h
                    and g.min_entry_price <= m.yes_price <= g.max_entry_price
                ):
                    pd = self.pm_tracker.delta(m.id)
                    top_deltas.append(f"{m.yes_price:.3f}@{pd:+.4f}")
            log(
                f"  [PM DEBUG] BTC={btc_dir} {btc_mom:+.3f}% — top PM deltas: {' | '.join(top_deltas)}"
            )

        for m in markets:
            # Filters
            if m.yes_price < g.min_entry_price or m.yes_price > g.max_entry_price:
                continue
            if m.volume_24h < g.min_volume_24h:
                continue

            pm_delta = self.pm_tracker.delta(m.id)
            pm_dps = self.pm_tracker.delta_per_sec(m.id)  # delta per second

            # Scale PM delta to comparable units (per minute)
            pm_delta_per_min = pm_dps * 60

            # Entry price for paper trade
            entry_price = m.yes_price
            cost_budget = self.capital * g.max_position_pct
            shares = max(1.0, cost_budget / entry_price)
            cost = shares * entry_price

            # Already have position in this market?
            open_ids = {t.market_id for t in self.trades if not t.exited}
            if m.id in open_ids:
                continue
            if len(open_ids) >= g.max_positions:
                break

            confidence = 0.0
            side = None
            reason = ""

            # Skip if BTC momentum is too weak (noise)
            if abs(btc_mom) < 0.03:
                continue

            # 1h momentum filter: skip if 1h opposes our trade (don't fight major trend)
            btc_1h = btc_f.momentum_1h
            if btc_dir == "BULL" and btc_1h < -0.15:
                continue  # 1h strongly down, skip BUY YES
            if btc_dir == "BEAR" and btc_1h > 0.15:
                continue  # 1h strongly up, skip BUY NO

            # ── BTC BULL: buy YES ────────────────────────────────────────────
            if btc_dir == "BULL":
                btc_mult = min(1.5, abs(btc_mom) / max(g.bull_threshold, 0.01))
                base = g.base_confidence * btc_mult

                # PM confirms: YES price rising
                if pm_delta > g.pm_bull_threshold:
                    conf = min(g.max_confidence, base + abs(pm_delta) * 200)
                    side = "YES"
                    reason = f"BTC BULL +{btc_mom:.3f}% + PM YES +{pm_delta:.4f}"
                # PM contradicts → skip
                elif pm_delta < -g.pm_bull_threshold:
                    continue
                # BTC up, PM flat → BTC must be strong (this is the main entry)
                else:
                    # BTC-only entry: boost confidence when BTC momentum is strong
                    btc_strength = min(1.5, abs(btc_mom) / g.bull_threshold)
                    conf = min(
                        g.max_confidence, g.base_confidence * 0.85 * btc_strength
                    )
                    side = "YES"
                    reason = f"BTC BULL +{btc_mom:.3f}% [BTC leading]"

            # ── BTC BEAR: buy NO ─────────────────────────────────────────────
            elif btc_dir == "BEAR":
                btc_mult = min(1.5, abs(btc_mom) / max(abs(g.bear_threshold), 0.01))
                base = g.base_confidence * btc_mult

                # PM confirms: YES price falling
                if pm_delta < -abs(g.pm_bear_threshold):
                    conf = min(g.max_confidence, base + abs(pm_delta) * 200)
                    side = "NO"
                    entry_price = m.no_price
                    shares = max(1.0, cost_budget / entry_price)
                    cost = shares * entry_price
                    reason = f"BTC BEAR {btc_mom:.3f}% + PM YES {pm_delta:.4f} → NO"
                # PM contradicts → skip
                elif pm_delta > abs(g.pm_bear_threshold):
                    continue
                # BTC down, PM flat → BTC must be strong
                else:
                    btc_strength = min(1.5, abs(btc_mom) / abs(g.bear_threshold))
                    conf = min(
                        g.max_confidence, g.base_confidence * 0.85 * btc_strength
                    )
                    side = "NO"
                    entry_price = m.no_price
                    shares = max(1.0, cost_budget / entry_price)
                    cost = shares * entry_price
                    reason = f"BTC BEAR {btc_mom:.3f}% [BTC leading]"

            # ── BTC NEUTRAL — skip ──────────────────────────────────────────
            else:
                continue

            if side is None or conf < g.min_confidence:
                continue
            if cost < 0.10 or shares < 1.0:
                continue
            if cost > self.capital * 0.95:
                continue

            signals.append(
                {
                    "id": self._sig_id(),
                    "market_id": m.id,
                    "question": m.question,
                    "token": m.yes_token if side == "YES" else m.no_token,
                    "yes_token": m.yes_token,
                    "side": side,
                    "entry_price": entry_price,
                    "shares": shares,
                    "cost": cost,
                    "confidence": conf,
                    "reason": reason,
                    "btc_dir": str(btc_dir),
                    "btc_mom": btc_mom,
                    "pm_delta": pm_delta,
                }
            )

        return signals

    # ── Position management ───────────────────────────────────────────────────

    def _get_current_yes_price(self, trade: Trade, fallback_market) -> float:
        """Get current YES price for an open position, always fresh."""
        m_id = trade.market_id
        from_prev = self.pm_tracker.current_price(m_id)
        if from_prev > 0:
            return from_prev
        # Not in poll results — fetch fresh using YES token
        pm_f = self.pm_tracker.fetcher
        fresh = pm_f.fetch_current_price(trade.yes_token)
        if fresh and fresh > 0:
            return fresh
        # Fallback: use market object passed in
        if fallback_market:
            return fallback_market.yes_price
        return trade.entry_price

    def _tick_positions(self, markets_by_id: dict[str, PMMarket], poll_interval: float):
        """Check all open positions for exit conditions."""
        now = self._now_ts()
        g = self.genome

        for trade in self.trades:
            if trade.exited:
                continue

            m = markets_by_id.get(trade.market_id)
            current_yes = self._get_current_yes_price(trade, m)
            current_price = current_yes if trade.side == "YES" else (1.0 - current_yes)

            # Unrealized P&L
            if trade.side == "YES":
                entry_val = trade.shares * trade.entry_price
                cur_val = trade.shares * current_price
            else:
                entry_val = trade.shares * trade.entry_price
                cur_val = trade.shares * current_price  # NO pays 1 on win

            pnl_pct = (cur_val - entry_val) / max(entry_val, 0.01)
            trade.current_price = current_price

            exited = False
            exit_reason = ""
            exit_price = current_price

            # Time-based exit
            hold_hours = (now - trade.opened_at) / 3600
            if hold_hours > g.max_hold_hours:
                exited = True
                exit_reason = f"max_hold_{hold_hours:.1f}h"
                # For unresolved markets, we use unrealized pnl as the exit pnl
                trade.gross_pnl = cur_val - entry_val
                trade.exit_price = current_price

            # Profit target
            if not exited and pnl_pct >= g.profit_target_pct:
                exited = True
                exit_reason = f"profit_target_{pnl_pct:.2%}"
                trade.gross_pnl = entry_val * g.profit_target_pct
                trade.exit_price = trade.entry_price * (1 + g.profit_target_pct)

            # Stop loss
            if not exited and pnl_pct <= -g.stop_loss_pct:
                exited = True
                exit_reason = f"stop_loss_{pnl_pct:.2%}"
                trade.gross_pnl = entry_val * (-g.stop_loss_pct)
                trade.exit_price = trade.entry_price * (1 - g.stop_loss_pct)

            if exited:
                trade.exited = True
                trade.last_update = now
                fee = trade.cost * TAKER_FEE_BPS / 10000
                trade.fee = fee
                trade.net_pnl = trade.gross_pnl - fee
                self.capital += trade.shares * trade.exit_price - trade.fee
                self.total_fees += fee
                self._session_rets.append(trade.net_pnl / max(self.start_capital, 1))

                if trade.net_pnl > 0.001:
                    self.wins += 1
                    trade.result = "win"
                elif trade.net_pnl < -0.001:
                    self.losses += 1
                    trade.result = "loss"
                else:
                    self.breakeven += 1
                    trade.result = "breakeven"

                self._journal_trade(trade)
                log(
                    f"  [EXIT:{exit_reason}] {trade.side} {trade.shares:.1f}@{trade.entry_price:.4f} "
                    f"pnl={trade.net_pnl:+.4f} | cap=${self.capital:.4f} {trade.result}"
                )

    def _journal_trade(self, trade: Trade):
        d = {
            "genome": self.genome.name,
            "trade_id": trade.id,
            "market_id": trade.market_id,
            "question": trade.question,
            "side": trade.side,
            "entry_price": trade.entry_price,
            "exit_price": trade.exit_price,
            "shares": trade.shares,
            "cost": trade.cost,
            "gross_pnl": round(trade.gross_pnl, 4),
            "net_pnl": round(trade.net_pnl, 4),
            "fee": round(trade.fee, 4),
            "exit_reason": trade.exit_reason,
            "result": trade.result,
            "opened_at": datetime.fromtimestamp(trade.opened_at).isoformat(),
            "closed_at": datetime.fromtimestamp(trade.last_update).isoformat(),
            "duration_secs": round(trade.last_update - trade.opened_at, 1),
        }
        try:
            with open(JOURNAL_FILE, "a") as f:
                f.write(json.dumps(d) + "\n")
        except:
            pass

    # ── Main session loop ─────────────────────────────────────────────────────

    def run_session(self) -> dict:
        """Run one trading session. Returns score dict."""
        self._running = True
        self._start_time = self._now_ts()
        session_end = self._start_time + (self.genome.session_minutes * 60)
        poll_interval = self.genome.poll_seconds

        log(
            f"  [{self.genome.name}] Starting session — "
            f"capital=${self.capital:.2f} poll={poll_interval}s "
            f"session={self.genome.session_minutes}min "
            f"btc_filter={self.genome.use_btc_filter} "
            f"primary_tf={self.genome.primary_timeframe}"
        )

        tick = 0
        while self._running and self._now_ts() < session_end:
            # Get current markets
            all_markets = self.pm_tracker.fetcher.get_top_markets(
                min_volume=self.genome.min_volume_24h * 0.5, limit=20
            )
            if not all_markets:
                time.sleep(poll_interval)
                tick += 1
                continue

            markets_by_id = {m.id: m for m in all_markets}

            # Poll PM prices (updates deltas)
            self.pm_tracker.poll(all_markets)

            # Refresh BTC data
            btc_f = self.btc.refresh()

            # Check exits first
            self._tick_positions(markets_by_id, poll_interval)

            # Generate and take signals
            open_count = sum(1 for t in self.trades if not t.exited)
            if open_count < self.genome.max_positions:
                signals = self._generate_signals(all_markets)
                for sig in signals:
                    if (
                        sum(1 for t in self.trades if not t.exited)
                        >= self.genome.max_positions
                    ):
                        break
                    if sig["cost"] > self.capital * 0.95:
                        continue
                    if sig["cost"] < 0.10:
                        continue

                    trade = Trade(
                        id=sig["id"],
                        market_id=sig["market_id"],
                        question=sig["question"],
                        token=sig["token"],
                        yes_token=sig["yes_token"],
                        side=sig["side"],
                        entry_price=sig["entry_price"],
                        current_price=sig["entry_price"],
                        shares=sig["shares"],
                        cost=sig["cost"],
                        opened_at=self._now_ts(),
                        last_update=self._now_ts(),
                    )
                    self.trades.append(trade)
                    self.capital -= sig["cost"]

                    log(
                        f"  [ENTRY {sig['confidence']:.0%}] {sig['side']} "
                        f"{sig['shares']:.1f}@{sig['entry_price']:.4f} cost=${sig['cost']:.2f} "
                        f"{sig['reason'][:60]} | cap=${self.capital:.4f}"
                    )

            # Periodic status
            if tick % 5 == 0 and tick > 0:
                btc_dir, btc_mom = self.btc.direction(self.genome)
                # Mark-to-market: include value of open positions
                open_val = 0.0
                for t in self.trades:
                    if not t.exited:
                        mkt = t.current_price if t.current_price > 0 else t.entry_price
                        open_val += t.shares * mkt
                total_val = self.capital + open_val
                pnl = total_val - self.start_capital
                total = self.wins + self.losses + self.breakeven
                wr = self.wins / max(total, 1) * 100
                log(
                    f"  [{self.genome.name}] tick={tick} "
                    f"BTC={btc_dir} {btc_mom:+.3f}% "
                    f"pos={sum(1 for t in self.trades if not t.exited)} "
                    f"WR={wr:.0f}% PnL=${pnl:+.4f} val=${total_val:.4f} cap=${self.capital:.4f}"
                )

            time.sleep(poll_interval)
            tick += 1

        # Force close all positions at session end (mark-to-market)
        self._force_close_all()
        return self._score()

    def _force_close_all(self):
        """Close all open positions at current prices."""
        if not any(not t.exited for t in self.trades):
            return
        log(f"  [{self.genome.name}] Force-closing all positions...")
        for trade in self.trades:
            if trade.exited:
                continue
            trade.exited = True
            trade.exit_reason = "session_end"
            trade.last_update = self._now_ts()
            fee = trade.cost * TAKER_FEE_BPS / 10000
            trade.fee = fee
            # Use current price from tracker as proxy for exit
            pm_delta = self.pm_tracker.delta(trade.market_id)
            cur_price = self.pm_tracker.current_price(trade.market_id)
            if cur_price <= 0:
                cur_price = trade.current_price
            if trade.side == "NO":
                cur_price = 1.0 - cur_price
            trade.current_price = cur_price
            trade.gross_pnl = (cur_price - trade.entry_price) * trade.shares
            trade.net_pnl = trade.gross_pnl - fee
            trade.result = (
                "win"
                if trade.net_pnl > 0.001
                else ("loss" if trade.net_pnl < -0.001 else "breakeven")
            )
            self.capital += trade.shares * cur_price - trade.fee
            self.total_fees += fee
            self._session_rets.append(trade.net_pnl / max(self.start_capital, 1))
            if trade.net_pnl > 0.001:
                self.wins += 1
            elif trade.net_pnl < -0.001:
                self.losses += 1
            else:
                self.breakeven += 1
            self._journal_trade(trade)

    def _score(self) -> dict:
        """
        Score genome performance — WR FIRST, then volume, then PnL.

        WR is the primary signal because:
        - Polymarket 2% taker fee means WR < 50% = guaranteed losses over time
        - A 60% WR with 2:1 win/loss ratio is massively profitable
        - Even 55% WR with balanced wins/losses covers fees
        """
        total = self.wins + self.losses + self.breakeven
        wr = self.wins / max(total, 1) if total > 0 else 0.0
        pnl = self.capital - self.start_capital
        pnl_pct = pnl / self.start_capital

        # Sharpe-like: mean return / std (if we have multiple trades)
        if len(self._session_rets) >= 2:
            import statistics

            mean_ret = sum(self._session_rets) / len(self._session_rets)
            std_ret = (
                statistics.stdev(self._session_rets)
                if len(self._session_rets) > 1
                else 0.001
            )
            sharpe = mean_ret / std_ret if std_ret > 0 else 0.0
        else:
            sharpe = 0.0

        # WR-first scoring: WR is 2x more important than PnL
        # WR component: 0-100 (100 for perfect WR)
        # PnL component: scaled so 10% PnL = 50 pts
        # Trade volume: reward genomes that find 5+ quality trades
        # Trade count penalty: < 3 trades is suspicious (over-fitting to 1-2 trades)
        wr_score = wr * 100.0
        pnl_score = max(pnl_pct, 0) * 500.0  # 5% PnL = 25 pts
        sharpe_score = sharpe * 10.0
        volume_bonus = min(total, 8) * 3.0  # reward up to 8 trades

        # Penalty for too few trades (quality over quantity, but zero = suspicious)
        if total < 3:
            trade_penalty = (3 - total) * 8.0
        else:
            trade_penalty = 0.0

        # Penalty for negative PnL (we want to win AND make money)
        pnl_penalty = max(pnl, 0) * 0.0 - abs(min(pnl, 0)) * 5.0

        score = (
            wr_score
            + pnl_score
            + sharpe_score
            + volume_bonus
            - trade_penalty
            + pnl_penalty
        )
        score = max(score, 0.0)  # floor at 0

        log(
            f"  [{self.genome.name}] SESSION DONE — "
            f"PnL=${pnl:+.4f} ({pnl_pct:+.2%}) WR={wr:.0%} Sharpe={sharpe:+.2f} "
            f"trades={total} W={self.wins} L={self.losses} B={self.breakeven} "
            f"final_cap=${self.capital:.4f} score={score:.4f} "
            f"(WR={wr_score:.1f}+PnL={pnl_score:.1f}+Sharpe={sharpe_score:.1f}+Vol={volume_bonus:.1f})"
        )

        return {
            "genome_name": self.genome.name,
            "score": round(score, 4),
            "pnl": round(pnl, 4),
            "pnl_pct": round(pnl_pct, 4),
            "win_rate": round(wr, 4),
            "sharpe": round(sharpe, 4),
            "trades": total,
            "wins": self.wins,
            "losses": self.losses,
            "breakeven": self.breakeven,
            "total_fees": round(self.total_fees, 4),
            "final_capital": round(self.capital, 4),
            "genome": self.genome.to_dict(),
        }


# ── Live Directional Executor (Real Money) ─────────────────────────────────────


class LiveDirectionalExecutor:
    """
    Live execution layer for the directional BTC×PM strategy.
    Uses py_clob_client with credentials from data/arbitrage-agent/.env.live

    Polymarket CLOB mechanics:
    - BUY YES → you get YES tokens. To close: SELL same qty of YES tokens.
    - BUY NO  → you get NO tokens. To close: SELL same qty of NO tokens.
    - Positions resolve at market expiry → pays $1 per token if your outcome wins.
    """

    TAKER_FEE_BPS = 200  # 2%

    def __init__(self, genome: StrategyGenome, capital: Optional[float] = None):
        self.genome = genome
        self.capital = (
            capital if capital is not None else 0.0
        )  # will be fetched from CLOB on init
        self.start_capital = None  # set after connecting
        self.trades: list[Trade] = []
        self.wins = self.losses = self.breakeven = 0
        self.total_fees = 0.0
        self.total_rebates = 0.0
        self.btc = BTCTracker()
        self.pm_tracker = PMTracker()
        self._running = False
        self._start_time = 0.0
        self._counter = 0
        self._session_rets: list[float] = []

        self.client = None
        self._open_orders: dict[str, dict] = {}  # order_id -> {trade, placed_at}
        self._filled_positions: dict[str, dict] = {}  # token_id -> {trade, shares}
        self._log_file = None

        self._init_client()

    def _init_client(self):
        from dotenv import load_dotenv
        from py_clob_client.client import ClobClient
        from py_clob_client.clob_types import (
            ApiCreds,
            AssetType,
            BalanceAllowanceParams,
        )

        ENV_PATH = os.path.join(
            os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
            "data",
            "arbitrage-agent",
            ".env.live",
        )
        load_dotenv(ENV_PATH)

        host = "https://clob.polymarket.com"
        pk = os.environ.get("POLYMARKET_PRIVATE_KEY")
        funder = os.environ.get("POLYMARKET_FUNDER_ADDRESS")
        sig_type = int(os.environ.get("POLYMARKET_SIGNATURE_TYPE", 2))
        key = os.environ.get("POLYMARKET_API_KEY")
        secret = os.environ.get("POLYMARKET_API_SECRET")
        passphrase = os.environ.get("POLYMARKET_API_PASSPHRASE")

        log(f"[LIVE] Initializing CLOB client...")
        try:
            init_creds = ApiCreds(
                api_key=key, api_secret=secret, api_passphrase=passphrase
            )
            self.client = ClobClient(
                host,
                key=pk,
                chain_id=137,
                signature_type=sig_type,
                funder=funder,
                creds=init_creds,
            )

            # Derive fresh credentials (required — stored API keys may be stale)
            derived = self.client.derive_api_key()
            self.client.set_api_creds(derived)
            log(f"[LIVE] ✅ API credentials derived from wallet.")

            if self.client.get_ok() != "OK":
                log(f"[LIVE] ❌ CLOB auth check failed: {self.client.get_ok()}")
                self.client = None
                return

            # Get USDC balance
            params = BalanceAllowanceParams(
                asset_type=AssetType.COLLATERAL, signature_type=sig_type
            )
            bal_resp = self.client.get_balance_allowance(params)
            balance = float(bal_resp.get("balance", 0)) / 1e6
            log(f"[LIVE] ✅ CLOB connected. USDC balance: ${balance:,.2f}")
            if balance < 1.0:
                log(f"[LIVE] ⚠️  Balance ${balance:.2f} is very low. Trades may fail.")

            self.capital = balance
            self.start_capital = balance

            LOG_FILE = (
                LOG_DIR
                / f"live_trades_{datetime.now().strftime('%Y%m%d_%H%M%S')}.jsonl"
            )
            self._log_file = LOG_FILE
            log(f"[LIVE] Logging live trades to: {LOG_FILE}")

        except Exception as e:
            log(f"[LIVE] ❌ Failed to init CLOB client: {e}")
            import traceback

            log(f"[LIVE] Trace: {traceback.format_exc()}")
            self.client = None

    def get_balance(self) -> float:
        if not self.client:
            return 0.0
        try:
            from py_clob_client.clob_types import AssetType, BalanceAllowanceParams

            params = BalanceAllowanceParams(
                asset_type=AssetType.COLLATERAL, signature_type=2
            )
            resp = self.client.get_balance_allowance(params)
            return float(resp.get("balance", 0)) / 1e6
        except Exception:
            return self.capital  # fallback to tracked capital

    def _id(self) -> str:
        self._counter += 1
        return f"L{datetime.now().strftime('%H%M%S')}_{self._counter}"

    def _now_ts(self) -> float:
        return time.time()

    def _live_log(self, msg: str):
        log(msg)
        if self._log_file:
            try:
                with open(self._log_file, "a") as f:
                    f.write(f"{datetime.now().isoformat()} {msg}\n")
            except:
                pass

    # ── Order execution ─────────────────────────────────────────────────────────

    def _place_order(
        self, token_id: str, side: str, price: float, size: float, trade: Trade
    ) -> bool:
        """Place a CLOB order. Returns True if placed (not necessarily filled)."""
        if not self.client:
            return False

        try:
            from py_clob_client.order_builder.constants import BUY, SELL
            from py_clob_client.clob_types import OrderArgs

            order_side = BUY if side == "YES" else SELL
            order_args = OrderArgs(
                price=min(price, 0.99),
                size=size,
                side=order_side,
                token_id=token_id,
            )
            signed = self.client.create_order(order_args)
            resp = self.client.post_order(signed)

            if resp and resp.get("success"):
                oid = resp.get("orderID", "unknown")
                self._open_orders[oid] = {
                    "trade": trade,
                    "side": side,
                    "token_id": token_id,
                    "size": size,
                    "price": price,
                    "placed_at": self._now_ts(),
                }
                self._live_log(
                    f"  [LIVE:OPEN] {side} {size:.2f}@{price:.4f} token={token_id[:16]}... oid={oid}"
                )
                return True
            else:
                self._live_log(f"  [LIVE:REJECT] order resp={resp}")
                return False
        except Exception as e:
            self._live_log(f"  [LIVE:ERROR] placing order: {e}")
            return False

    def _cancel_order(self, order_id: str) -> bool:
        if not self.client:
            return False
        try:
            resp = self.client.delete_order(order_id)
            self._live_log(f"  [LIVE:CANCEL] oid={order_id} resp={resp}")
            return True
        except Exception as e:
            self._live_log(f"  [LIVE:CANCEL:ERROR] {e}")
            return False

    def _poll_fills(self):
        """Check open orders for fills."""
        if not self.client or not self._open_orders:
            return

        still_open = {}
        for oid, info in list(self._open_orders.items()):
            try:
                status = self.client.get_order(order_id=oid)
                if not status:
                    still_open[oid] = info
                    continue

                filled_qty = float(status.get("size", "0") or 0)
                status_str = status.get("status", "").upper()

                if status_str in ("FILLED", "FILLED_FULL"):
                    trade = info["trade"]
                    trade.shares = filled_qty if filled_qty > 0 else info["size"]
                    trade.cost = trade.shares * trade.entry_price
                    trade.current_price = trade.entry_price
                    trade.opened_at = info["placed_at"]
                    trade.last_update = self._now_ts()
                    self._filled_positions[info["token_id"]] = {
                        "trade": trade,
                        "shares": trade.shares,
                    }
                    self._live_log(
                        f"  [LIVE:FILLED] {trade.side} {trade.shares:.2f}@{trade.entry_price:.4f} oid={oid}"
                    )
                    # Deduct cost from capital
                    self.capital -= trade.cost

                elif status_str in ("CANCELLED", "EXPIRED", "REVOKED"):
                    trade = info["trade"]
                    self._live_log(f"  [LIVE:CANCELLED] oid={oid} status={status_str}")
                    # Return cost to capital (already handled)
                else:
                    # Still open
                    still_open[oid] = info

            except Exception as e:
                self._live_log(f"  [LIVE:POLL:ERROR] oid={oid}: {e}")
                still_open[oid] = info

        self._open_orders = still_open

    def _tick_live_positions(self, markets_by_id: dict[str, PMMarket]):
        """Check filled positions for SL/TP triggers."""
        if not self._filled_positions:
            return

        for token_id, info in list(self._filled_positions.items()):
            trade = info["trade"]
            if trade.exited:
                continue

            m = markets_by_id.get(trade.market_id)
            cur_price = self._get_current_price(token_id, trade.side, m)
            if cur_price <= 0:
                continue

            # Calculate unrealized P&L
            if trade.side == "YES":
                pnl_pct = (cur_price - trade.entry_price) / trade.entry_price
            else:  # NO
                pnl_pct = (trade.entry_price - cur_price) / trade.entry_price

            trade.current_price = cur_price
            exited = False
            exit_reason = ""

            # Time exit
            hold_hours = (self._now_ts() - trade.opened_at) / 3600
            if hold_hours > self.genome.max_hold_hours:
                exited = True
                exit_reason = f"max_hold_{hold_hours:.1f}h"

            # Profit target
            if not exited and pnl_pct >= self.genome.profit_target_pct:
                exited = True
                exit_reason = f"TP_{pnl_pct:.2%}"

            # Stop loss
            if not exited and pnl_pct <= -self.genome.stop_loss_pct:
                exited = True
                exit_reason = f"SL_{pnl_pct:.2%}"

            if exited:
                trade.exited = True
                trade.last_update = self._now_ts()
                trade.exit_reason = exit_reason
                trade.exit_price = cur_price

                # Close the position on CLOB
                self._close_position(trade, cur_price)

                fee = trade.cost * self.TAKER_FEE_BPS / 10000
                trade.fee = fee
                trade.gross_pnl = (cur_price - trade.entry_price) * trade.shares
                if trade.side == "NO":
                    trade.gross_pnl = (trade.entry_price - cur_price) * trade.shares
                trade.net_pnl = trade.gross_pnl - fee
                self.total_fees += fee

                self.capital += trade.shares * cur_price - fee
                self._session_rets.append(
                    trade.net_pnl / max(self.start_capital or 1, 1)
                )

                if trade.net_pnl > 0.001:
                    self.wins += 1
                    trade.result = "win"
                elif trade.net_pnl < -0.001:
                    self.losses += 1
                    trade.result = "loss"
                else:
                    self.breakeven += 1
                    trade.result = "breakeven"

                self._journal_trade(trade)
                self._live_log(
                    f"  [LIVE:CLOSE:{exit_reason}] {trade.side} {trade.shares:.2f} "
                    f"entry={trade.entry_price:.4f} exit={cur_price:.4f} "
                    f"pnl={trade.net_pnl:+.4f} cap=${self.capital:.4f} {trade.result}"
                )

                del self._filled_positions[token_id]

    def _close_position(self, trade: Trade, current_price: float):
        """Place closing order on CLOB."""
        close_side = "NO" if trade.side == "YES" else "YES"
        close_token = (
            trade.yes_token
            if close_side == "YES"
            else self._get_no_token(trade.market_id)
        )

        if not close_token:
            self._live_log(f"  [LIVE:CLOSE:ERROR] no close token for {trade.market_id}")
            return

        self._place_order(
            token_id=close_token,
            side=close_side,
            price=current_price,
            size=trade.shares,
            trade=trade,
        )

    def _get_current_price(
        self, token_id: str, side: str, market: Optional[PMMarket]
    ) -> float:
        """Get current price for a token."""
        if market:
            if side == "YES":
                pm_f = self.pm_tracker.fetcher
                price = pm_f.fetch_current_price(token_id)
                if price > 0:
                    return price
                return market.yes_price
            else:
                pm_f = self.pm_tracker.fetcher
                no_price = pm_f.fetch_current_price(market.yes_token)
                if no_price > 0:
                    return 1.0 - no_price
                return market.no_price
        return 0.0

    def _get_no_token(self, market_id: str) -> str:
        """Get NO token for a market from cache."""
        for m in self.pm_tracker.fetcher._market_cache.values():
            if m.id == market_id:
                return m.no_token
        return ""

    def _journal_trade(self, trade: Trade):
        d = {
            "mode": "live",
            "genome": self.genome.name,
            "trade_id": trade.id,
            "market_id": trade.market_id,
            "question": trade.question,
            "side": trade.side,
            "entry_price": trade.entry_price,
            "exit_price": trade.exit_price,
            "shares": trade.shares,
            "cost": trade.cost,
            "gross_pnl": round(trade.gross_pnl, 4),
            "net_pnl": round(trade.net_pnl, 4),
            "fee": round(trade.fee, 4),
            "exit_reason": trade.exit_reason,
            "result": trade.result,
            "opened_at": datetime.fromtimestamp(trade.opened_at).isoformat(),
            "closed_at": datetime.fromtimestamp(trade.last_update).isoformat(),
            "duration_secs": round(trade.last_update - trade.opened_at, 1),
        }
        try:
            with open(JOURNAL_FILE, "a") as f:
                f.write(json.dumps(d) + "\n")
        except:
            pass

    # ── Signal generation (reuse from paper trader) ────────────────────────────

    def _generate_signals(self, markets: list[PMMarket]) -> list[dict]:
        """Identical signal generation to DirectionalPaperTrader."""
        return self._paper_trader_signals(markets)

    def _paper_trader_signals(self, markets: list[PMMarket]) -> list[dict]:
        """Copy of DirectionalPaperTrader._generate_signals for live mode."""
        btc_dir, btc_mom = self.btc.direction(self.genome)
        btc_f = self.btc.refresh()
        g = self.genome
        signals = []

        for m in markets:
            if m.yes_price < g.min_entry_price or m.yes_price > g.max_entry_price:
                continue
            if m.volume_24h < g.min_volume_24h:
                continue

            pm_delta = self.pm_tracker.delta(m.id)
            pm_dps = self.pm_tracker.delta_per_sec(m.id)

            entry_price = m.yes_price
            cost_budget = max(self.capital, 1.0) * g.max_position_pct
            shares = max(1.0, cost_budget / entry_price)
            cost = shares * entry_price

            open_ids = {t.market_id for t in self.trades if not t.exited}
            live_token_ids = set(self._filled_positions.keys())
            if m.id in open_ids or m.yes_token in live_token_ids:
                continue
            if len(open_ids) + len(live_token_ids) >= g.max_positions:
                break

            confidence = 0.0
            side = None
            reason = ""

            if abs(btc_mom) < 0.03:
                continue

            btc_1h = btc_f.momentum_1h
            if btc_dir == "BULL" and btc_1h < -0.15:
                continue
            if btc_dir == "BEAR" and btc_1h > 0.15:
                continue

            if btc_dir == "BULL":
                btc_mult = min(1.5, abs(btc_mom) / max(g.bull_threshold, 0.01))
                base = g.base_confidence * btc_mult

                if pm_delta > g.pm_bull_threshold:
                    conf = min(g.max_confidence, base + abs(pm_delta) * 200)
                    side = "YES"
                    reason = f"LIVE BTC BULL +{btc_mom:.3f}% + PM YES +{pm_delta:.4f}"
                elif pm_delta < -g.pm_bull_threshold:
                    continue
                else:
                    btc_strength = min(1.5, abs(btc_mom) / g.bull_threshold)
                    conf = min(
                        g.max_confidence, g.base_confidence * 0.85 * btc_strength
                    )
                    side = "YES"
                    reason = f"LIVE BTC BULL +{btc_mom:.3f}% [BTC leading]"

            elif btc_dir == "BEAR":
                btc_mult = min(1.5, abs(btc_mom) / max(abs(g.bear_threshold), 0.01))
                base = g.base_confidence * btc_mult

                if pm_delta < -abs(g.pm_bear_threshold):
                    conf = min(g.max_confidence, base + abs(pm_delta) * 200)
                    side = "NO"
                    entry_price = m.no_price
                    shares = max(1.0, cost_budget / entry_price)
                    cost = shares * entry_price
                    reason = (
                        f"LIVE BTC BEAR {btc_mom:.3f}% + PM YES {pm_delta:.4f} → NO"
                    )
                elif pm_delta > abs(g.pm_bear_threshold):
                    continue
                else:
                    btc_strength = min(1.5, abs(btc_mom) / abs(g.bear_threshold))
                    conf = min(
                        g.max_confidence, g.base_confidence * 0.85 * btc_strength
                    )
                    side = "NO"
                    entry_price = m.no_price
                    shares = max(1.0, cost_budget / entry_price)
                    cost = shares * entry_price
                    reason = f"LIVE BTC BEAR {btc_mom:.3f}% [BTC leading]"

            else:
                continue

            if side is None or conf < g.min_confidence:
                continue
            if cost < 0.50 or shares < 1.0:
                continue
            if cost > self.capital * 0.95:
                continue

            signals.append(
                {
                    "id": self._id(),
                    "market_id": m.id,
                    "question": m.question,
                    "token": m.yes_token if side == "YES" else m.no_token,
                    "yes_token": m.yes_token,
                    "no_token": m.no_token,
                    "side": side,
                    "entry_price": entry_price,
                    "shares": shares,
                    "cost": cost,
                    "confidence": conf,
                    "reason": reason,
                    "btc_dir": str(btc_dir),
                    "btc_mom": btc_mom,
                    "pm_delta": pm_delta,
                }
            )

        return signals

    # ── Main live loop ─────────────────────────────────────────────────────────

    def run_live(self, poll: int = 10):
        """Run live trading until interrupted."""
        self._running = True
        self._start_time = self._now_ts()

        if not self.client:
            log("[LIVE] No CLOB client — cannot run live. Exiting.")
            return {"error": "no_client"}

        log(
            f"[LIVE] Starting live trading — genome={self.genome.name} "
            f"poll={poll}s max_pos={self.genome.max_positions}"
        )

        tick = 0
        while self._running:
            try:
                # Fetch markets
                all_markets = self.pm_tracker.fetcher.get_top_markets(
                    min_volume=self.genome.min_volume_24h * 0.5, limit=20
                )
                if not all_markets:
                    time.sleep(poll)
                    tick += 1
                    continue

                markets_by_id = {m.id: m for m in all_markets}

                # Poll PM prices
                self.pm_tracker.poll(all_markets)

                # Refresh BTC
                self.btc.refresh()

                # Poll open orders for fills
                self._poll_fills()

                # Check live positions for SL/TP
                self._tick_live_positions(markets_by_id)

                # Generate and execute new signals
                open_count = len([t for t in self.trades if not t.exited]) + len(
                    self._filled_positions
                )
                if open_count < self.genome.max_positions:
                    signals = self._generate_signals(all_markets)
                    for sig in signals:
                        if len(self._filled_positions) >= self.genome.max_positions:
                            break

                        trade = Trade(
                            id=sig["id"],
                            market_id=sig["market_id"],
                            question=sig["question"],
                            token=sig["token"],
                            yes_token=sig["yes_token"],
                            side=sig["side"],
                            entry_price=sig["entry_price"],
                            current_price=sig["entry_price"],
                            shares=sig["shares"],
                            cost=sig["cost"],
                            opened_at=self._now_ts(),
                            last_update=self._now_ts(),
                        )
                        self.trades.append(trade)

                        # Place the CLOB order
                        placed = self._place_order(
                            token_id=sig["token"],
                            side=sig["side"],
                            price=sig["entry_price"],
                            size=sig["shares"],
                            trade=trade,
                        )
                        if not placed:
                            # Order rejected — remove trade and refund capital
                            self.trades.remove(trade)
                        else:
                            self._live_log(
                                f"  [LIVE:SIGNAL:{sig['confidence']:.0%}] {sig['side']} "
                                f"{sig['shares']:.1f}@{sig['entry_price']:.4f} "
                                f"{sig['reason'][:60]} | cap=${self.capital:.4f}"
                            )

                # Status every 10 ticks
                if tick % 10 == 0 and tick > 0:
                    btc_dir, btc_mom = self.btc.direction(self.genome)
                    bal = self.get_balance()
                    total_val = self.capital + sum(
                        t.shares * t.current_price for t in self.trades if not t.exited
                    )
                    pnl = total_val - (self.start_capital or 0)
                    total = self.wins + self.losses + self.breakeven
                    wr = self.wins / max(total, 1) * 100
                    self._live_log(
                        f"[LIVE:STATUS] tick={tick} BTC={btc_dir} {btc_mom:+.3f}% "
                        f"open_orders={len(self._open_orders)} positions={len(self._filled_positions)} "
                        f"WR={wr:.0f}% PnL=${pnl:+.4f} balance=${bal:.4f}"
                    )

                time.sleep(poll)
                tick += 1

            except Exception as e:
                self._live_log(f"[LIVE:ERROR] tick={tick}: {e}")
                import traceback

                self._live_log(traceback.format_exc())
                time.sleep(poll)

        # ── Shutdown: cancel open orders ──────────────────────────────────────
        log("[LIVE] Shutting down — cancelling open orders...")
        for oid in list(self._open_orders.keys()):
            self._cancel_order(oid)
        self._open_orders.clear()

        total = self.wins + self.losses + self.breakeven
        pnl = self.capital - (self.start_capital or 0)
        log(
            f"[LIVE] FINAL — trades={total} W={self.wins} L={self.losses} B={self.breakeven} "
            f"PnL=${pnl:+.4f} final_cap=${self.capital:.4f}"
        )

        return {
            "mode": "live",
            "pnl": round(pnl, 4),
            "wins": self.wins,
            "losses": self.losses,
            "breakeven": self.breakeven,
            "total_fees": round(self.total_fees, 4),
            "final_capital": round(self.capital, 4),
        }


# ── Evolution Engine ───────────────────────────────────────────────────────────


class EvolutionEngine:
    def __init__(self):
        self.generation = 0
        self.population: list[StrategyGenome] = []
        self.best_genome: Optional[StrategyGenome] = None
        self.best_score = float("-inf")
        self.session_results: list[dict] = []

    def load_or_create_population(self) -> list[StrategyGenome]:
        """Load previous best genome, create random population."""
        if BEST_GENOME_FILE.exists():
            try:
                with open(BEST_GENOME_FILE) as f:
                    d = json.load(f)
                loaded = StrategyGenome.from_dict(d.get("genome", d))
                log(
                    f"Loaded best genome: {loaded.name} (score={d.get('best_score', 'unknown')})"
                )
                self.best_genome = loaded
                self.generation = d.get("generation", 0)
                self.best_score = d.get("best_score", float("-inf"))
                # Create population from loaded best + mutations
                pop = [loaded]
                for _ in range(POP_SIZE - 1):
                    pop.append(loaded.mutate(rate=0.35))
                return pop
            except Exception as e:
                log(f"Failed to load genome: {e}")

        log(f"Creating new random population (size={POP_SIZE})")
        return StrategyGenome.random_population(POP_SIZE)

    def run_generation(
        self, population: list[StrategyGenome]
    ) -> tuple[list[StrategyGenome], list[dict]]:
        """Run one full generation: evaluate all genomes, breed next pop."""
        self.generation += 1
        results = []

        # Apply env overrides to all genomes before evaluating
        for g in population:
            g.session_minutes = SESSION_MINUTES
            g.poll_seconds = max(
                10, min(30, SESSION_MINUTES // 4)
            )  # 4-6 ticks per session minimum

        log(f"\n{'=' * 60}")
        log(f"GENERATION {self.generation} | Population: {len(population)}")
        log(f"{'=' * 60}")

        # Shuffle for fair evaluation order
        eval_order = list(population)
        random.shuffle(eval_order)

        for i, genome in enumerate(eval_order):
            log(f"\n  --- Genome {i + 1}/{len(eval_order)}: {genome.name} ---")
            trader = DirectionalPaperTrader(genome, capital=PAPER_CAPITAL)
            result = trader.run_session()
            result["generation"] = self.generation
            results.append(result)
            self._log_result(result)

            # Track best
            if self.best_genome is None or result["score"] > self.best_score:
                self.best_score = result["score"]
                self.best_genome = copy.deepcopy(genome)
                log(f"  🏆 NEW ALL-TIME BEST: {self.best_score:.4f} ({genome.name})")

        # Sort by score
        scored = sorted(
            zip(results, population), key=lambda x: x[0]["score"], reverse=True
        )

        # Select elite — WR must be > 0 to be considered
        # Priority: high WR first, then by score
        elite = [copy.deepcopy(g) for _, g in scored if _[0]["win_rate"] > 0][:5]
        if len(elite) < 2:
            log(f"  ⚠️  No genomes with WR > 0! Taking top 2 by score anyway.")
            elite = [copy.deepcopy(g) for _, g in scored[:2]]

        log(f"\n  TOP 3 by WR+Score:")
        for r, g in scored[:3]:
            log(
                f"    {r['score']:+.4f} | PnL={r['pnl']:+.4f} WR={r['win_rate']:.0%} trades={r['trades']} | {g.name}"
            )

        # Breed next population — only from WR-positive genomes
        next_pop = list(elite)
        while len(next_pop) < len(population):
            if len(next_pop) < len(elite):
                next_pop.append(copy.deepcopy(elite[len(next_pop)]))
            else:
                # Tournament selection: pick 3 random, breed best 2
                contenders = random.sample(
                    list(zip(results, population)), min(5, len(population))
                )
                sorted_contenders = sorted(
                    contenders, key=lambda x: x[0]["score"], reverse=True
                )
                a_g, b_g = sorted_contenders[0][1], sorted_contenders[1][1]
                child = a_g.crossover(a_g, b_g)
                child = child.mutate(rate=0.20)
                next_pop.append(child)

        return next_pop, results

    def _log_result(self, result: dict):
        try:
            with open(FITNESS_HISTORY, "a") as f:
                row = {
                    "generation": result["generation"],
                    "genome_name": result["genome_name"],
                    "score": result["score"],
                    "pnl": result["pnl"],
                    "win_rate": result["win_rate"],
                    "sharpe": result["sharpe"],
                    "trades": result["trades"],
                    "timestamp": datetime.now().isoformat(),
                }
                f.write(json.dumps(row) + "\n")
        except:
            pass

    def ask_ai_for_tuning(self) -> Optional[StrategyGenome]:
        """Ask MiniMax to analyze recent trades and suggest genome improvements."""
        recent_trades = []
        try:
            with open(JOURNAL_FILE) as f:
                lines = f.readlines()
            recent = lines[-50:] if len(lines) > 50 else lines
            for line in recent:
                try:
                    recent_trades.append(json.loads(line))
                except:
                    pass
        except:
            pass

        if len(recent_trades) < 3:
            return None

        # Summarize trade stats
        wins = [t for t in recent_trades if t.get("result") == "win"]
        losses = [t for t in recent_trades if t.get("result") == "loss"]
        avg_win = sum(t.get("net_pnl", 0) for t in wins) / max(len(wins), 1)
        avg_loss = sum(t.get("net_pnl", 0) for t in losses) / max(len(losses), 1)

        prompt = f"""Analyze this Polymarket intraday trading journal and suggest genome parameter improvements.

Recent trades ({len(recent_trades)} total, {len(wins)}W/{len(losses)}L):
- Avg win: ${avg_win:+.4f}
- Avg loss: ${avg_loss:+.4f}

Sample trades (last 10):
{chr(10).join([f"  {t.get('side')} {t.get('result')} {t.get('exit_reason')} pnl={t.get('net_pnl', 0):+.4f} {t.get('reason', '')[:60]}" for t in recent_trades[-10:]])}

Current genome params:
{json.dumps(self.best_genome.to_dict() if self.best_genome else {}, indent=2)}

Suggest 3-5 specific param changes (with values) that would improve P&L.
Be specific: parameter name, current value, suggested new value, and reasoning.
Return as JSON: {{"suggestions": [{{"param": "...", "current": X, "suggested": Y, "reason": "..."}}]}}"""

        response = ai_complete(prompt, max_tokens=800)
        if not response:
            return None

        try:
            # Try to parse JSON from response
            import re

            match = re.search(r"\{.*\}", response, re.DOTALL)
            if match:
                data = json.loads(match.group())
                suggestions = data.get("suggestions", [])
                if suggestions and self.best_genome:
                    log(f"  AI suggested {len(suggestions)} param changes:")
                    for s in suggestions:
                        log(
                            f"    {s['param']}: {s['current']} → {s['suggested']} ({s['reason']})"
                        )
                    # Apply first suggestion as a mutated genome
                    sug = suggestions[0]
                    new_genome = self.best_genome.mutate(rate=0.0)
                    if sug["param"] in new_genome.to_dict():
                        setattr(new_genome, sug["param"], sug["suggested"])
                        new_genome.name = f"ai_{self.best_genome.name}_{sug['param']}"
                        return new_genome
        except Exception as e:
            log(f"  AI parse error: {e}")

        return None

    def save(self):
        """Save best genome and state."""
        if self.best_genome is None:
            return
        state = {
            "generation": self.generation,
            "best_score": self.best_score,
            "genome": self.best_genome.to_dict(),
            "saved_at": datetime.now().isoformat(),
        }
        with open(BEST_GENOME_FILE, "w") as f:
            json.dump(state, f, indent=2)
        log(f"Saved best genome: {self.best_genome.name} score={self.best_score:.4f}")


# ── Main Loop ─────────────────────────────────────────────────────────────────


def run_live_mode(genome: StrategyGenome):
    """Run live trading with the given genome until interrupted."""
    executor = LiveDirectionalExecutor(genome)
    return executor.run_live(poll=genome.poll_seconds)


def run_paper_mode():
    """Run paper trading evolution loop."""
    engine = EvolutionEngine()

    for gen_i in range(GENERATIONS_PER_RUN):
        log(f"\n{'=' * 60}")
        log(f" GENERATION {gen_i + 1}/{GENERATIONS_PER_RUN}")
        log(f"{'=' * 60}")

        if gen_i == 0:
            population = engine.load_or_create_population()
        else:
            population = engine.population

        next_pop, results = engine.run_generation(population)
        engine.population = next_pop

        ai_genome = engine.ask_ai_for_tuning()
        if ai_genome:
            engine.population[-1] = ai_genome
            log(f"  AI genome inserted: {ai_genome.name}")

        engine.save()

        best_name = engine.best_genome.name if engine.best_genome else "none"
        log(
            f"\n  Generation {engine.generation} complete. "
            f"Best score: {engine.best_score:.4f} ({best_name})"
        )

    engine.save()


def main():
    import argparse

    parser = argparse.ArgumentParser(description="BTC Directional Polymarket Trading")
    parser.add_argument(
        "--live",
        action="store_true",
        help="Run LIVE trading with real money (uses .env.live credentials)",
    )
    parser.add_argument(
        "--genome",
        type=str,
        default=None,
        help="Path to genome JSON file (default: polymarket_best_genome.json)",
    )
    parser.add_argument(
        "--poll",
        type=int,
        default=None,
        help="Override poll interval in seconds",
    )
    args = parser.parse_args()

    if args.live:
        log(f"\n{'#' * 60}")
        log(f"AUTOIMPROVE LIVE v3 — LIVE TRADING MODE")
        log(f"#{'#' * 60}")
        log(f"Using credentials from: data/arbitrage-agent/.env.live")

        genome_path = args.genome or BEST_GENOME_FILE
        if not os.path.exists(genome_path):
            log(f"❌ No genome found at {genome_path}. Exiting.")
            sys.exit(1)

        with open(genome_path) as f:
            d = json.load(f)
        genome = StrategyGenome.from_dict(d.get("genome", d))
        log(f"Loaded genome: {genome.name}")

        if args.poll:
            genome.poll_seconds = args.poll

        log(f"Starting live trading with genome: {genome.name}")
        log(
            f"Params: bull={genome.bull_threshold} bear={genome.bear_threshold} "
            f"min_conf={genome.min_confidence} max_pos={genome.max_positions}"
        )

        def stop_handler(sig, frame):
            log("STOP signal received — shutting down live executor...")
            sys.exit(0)

        signal.signal(signal.SIGINT, stop_handler)
        signal.signal(signal.SIGTERM, stop_handler)

        result = run_live_mode(genome)
        log(f"Live trading stopped: {result}")

    else:
        log(f"\n{'#' * 60}")
        log(f"AUTOIMPROVE LIVE v3 — BTC Directional Polymarket Trading (PAPER)")
        log(f"#{'#' * 60}")
        log(f"POP_SIZE={POP_SIZE} SESSION_MINUTES={SESSION_MINUTES}")
        log(f"Best genome: {BEST_GENOME_FILE}")
        log(f"Journal: {JOURNAL_FILE}")

        def stop_handler(sig, frame):
            log("STOP signal received — saving and exiting...")
            sys.exit(0)

        signal.signal(signal.SIGINT, stop_handler)
        signal.signal(signal.SIGTERM, stop_handler)

        run_paper_mode()


if __name__ == "__main__":
    main()
