#!/usr/bin/env python3
"""
Intraday Trader v6 — Polymarket BTC/ETH/SOL momentum trading.
 Real-time RTDS WebSocket prices → Polymarket CLOB order placement.
 Paper trading (virtual money, real prices, real P&L tracking).
 Auto-improvement via genetic algorithm + real-time parameter evolution.
 Supports both legacy momentum and new TA strategies (RSI/MACD/BB/breakout).
"""

from __future__ import annotations

import os, sys, json, time, signal, random, logging, itertools, threading, math
import websocket
from datetime import datetime
from pathlib import Path
from dataclasses import dataclass, field, asdict

PAPER_CAPITAL = 100.0
STATE_FILE = Path(__file__).parent / "state" / "intraday_trades.json"
JOURNAL_FILE = Path(__file__).parent / "state" / "intraday_journal.jsonl"
BEST_PARAMS = Path(__file__).parent / "state" / "best_intraday_params.json"
LOG_DIR = Path(__file__).parent / "logs"
TAKER_FEE_BPS = 5.0
MIN_CAPITAL = 2.0
SATS = 100000000  # Satoshis per BTC


# ─────────────────────────────────────────────
# Technical Indicator Functions
# ─────────────────────────────────────────────


def _rsi(values, period=14):
    if len(values) < period + 1:
        return []
    changes = [values[i] - values[i - 1] for i in range(1, len(values))]
    gains = [max(c, 0) for c in changes]
    losses = [-min(c, 0) for c in changes]
    avg_gain = sum(gains[:period]) / period
    avg_loss = sum(losses[:period]) / period
    result = [50.0]
    for i in range(period, len(changes)):
        avg_gain = (avg_gain * (period - 1) + gains[i]) / period
        avg_loss = (avg_loss * (period - 1) + losses[i]) / period
        if avg_loss == 0:
            result.append(100.0)
        else:
            rs = avg_gain / avg_loss
            result.append(100 - (100 / (1 + rs)))
    return result


def _macd(values, fast=12, slow=26, signal_period=9):
    def ema(vals, n):
        if len(vals) < n:
            return []
        k = 2 / (n + 1)
        result = [sum(vals[:n]) / n]
        for v in vals[n:]:
            result.append(v * k + result[-1] * (1 - k))
        return result

    if len(values) < slow + 1:
        return [], [], []
    ef = ema(values, fast)
    es = ema(values, slow)
    macd_line = [ef[i] - es[i] for i in range(len(es))]
    sig = ema(macd_line, signal_period)
    h = []
    off = len(macd_line) - len(sig)
    for i in range(len(sig)):
        h.append(macd_line[i + off] - sig[i])
    return macd_line, sig, h


def _bollinger(values, period=20, std_dev=2.0):
    if len(values) < period:
        return [], [], []
    result = []
    for i in range(period - 1, len(values)):
        window = values[i - period + 1 : i + 1]
        mean = sum(window) / period
        sd = math.sqrt(sum((v - mean) ** 2 for v in window) / period)
        result.append((mean + std_dev * sd, mean, mean - std_dev * sd))
    if not result:
        return [], [], []
    u, m, l = zip(*result)
    return list(u), list(m), list(l)


# ─────────────────────────────────────────────
# TA Strategy Entry Logic (for evolved strategies)
# ─────────────────────────────────────────────


class TAStrategy:
    """TA-based strategy executor for evolved parameters."""

    @staticmethod
    def check_entry(params, history, price):
        """
        Check if we should enter a trade based on evolved TA strategy.

        Strategies:
        - breakout: momentum + volatility breakout
        - mean_reversion: RSI + Bollinger Bands
        - overbought_oversold: RSI only
        - multi_tf_confirm: RSI + MACD + momentum

        Returns: (side, reason) or None
        """
        if len(history) < 50:
            return None

        closes = [h[1] for h in history]

        strategy = params.get("strategy", "breakout")

        rsi_period = params.get("rsi_period", 14)
        rsi_vals = _rsi(closes, rsi_period)

        macd_fast = params.get("macd_fast", 12)
        macd_slow = params.get("macd_slow", 26)
        macd_line, _, hist = _macd(closes, macd_fast, macd_slow)

        bb_period = params.get("bb_period", 20)
        bb_u, bb_m, bb_l = _bollinger(closes, bb_period)

        cur = price
        mom_th = params.get("mom_th", 0.5)

        if strategy == "breakout":
            if len(closes) < 6:
                return None
            mom = (closes[-1] - closes[-6]) / closes[-6] * 100
            vol = (max(closes[-6:]) - min(closes[-6:])) / closes[-6] * 100
            if mom > mom_th and vol > mom_th * 0.3:
                return ("LONG", "breakout")
            elif mom < -mom_th and vol > mom_th * 0.3:
                return ("SHORT", "breakdown")

        elif strategy == "mean_reversion":
            if not rsi_vals or len(rsi_vals) < 5:
                return None
            rsi = rsi_vals[-1]
            rsi_oversold = params.get("rsi_oversold", 30)
            rsi_overbought = params.get("rsi_overbought", 70)
            bb_lower = bb_l[-1] if bb_l else cur * 0.98
            bb_upper = bb_u[-1] if bb_u else cur * 1.02

            if rsi < rsi_oversold and cur <= bb_lower * 1.01:
                return ("LONG", "oversold_revert")
            elif rsi > rsi_overbought and cur >= bb_upper * 0.99:
                return ("SHORT", "overbought_revert")

        elif strategy == "overbought_oversold":
            if not rsi_vals or len(rsi_vals) < 5:
                return None
            rsi = rsi_vals[-1]
            rsi_oversold = params.get("rsi_oversold", 30)
            rsi_overbought = params.get("rsi_overbought", 70)

            if rsi < rsi_oversold:
                return ("LONG", "rsi_oversold")
            elif rsi > rsi_overbought:
                return ("SHORT", "rsi_overbought")

        elif strategy == "multi_tf_confirm":
            if not rsi_vals or len(rsi_vals) < 5 or not hist:
                return None
            rsi = rsi_vals[-1]
            macd_h = hist[-1] if hist else 0
            rsi_oversold = params.get("rsi_oversold", 30)
            rsi_overbought = params.get("rsi_overbought", 70)

            if len(closes) >= 11:
                mom = (closes[-1] - closes[-11]) / closes[-11] * 100
            else:
                mom = 0

            if rsi < rsi_oversold and macd_h > 0 and mom > mom_th:
                return ("LONG", "multi_tf_bullish")
            elif rsi > rsi_overbought and macd_h < 0 and mom < -mom_th:
                return ("SHORT", "multi_tf_bearish")

        return None


# ─────────────────────────────────────────────
# RTDS WebSocket Streamer
# ─────────────────────────────────────────────


class RTDSStreamer:
    def __init__(self, symbols=None):
        self.symbols = symbols or ["btcusdt", "ethusdt", "solusdt"]
        self.prices = {}
        self.timestamps = {}
        self.history = {}
        self.MAX_HIST = 10000
        self.running = False
        self.ws = None
        self.thread = None

    def _on_message(self, ws, message):
        try:
            d = json.loads(message)
            if d.get("topic") != "crypto_prices":
                return
            p = d.get("payload", {})
            sym = p.get("symbol", "")
            val = p.get("value")
            ts = p.get("timestamp")
            if val is None or ts is None:
                return
            price = float(val)
            ts_ms = int(ts)
            self.prices[sym] = price
            self.timestamps[sym] = ts_ms
            h = self.history.setdefault(sym, [])
            h.append((ts_ms, price))
            if len(h) > self.MAX_HIST:
                self.history[sym] = h[-self.MAX_HIST :]
        except Exception:
            pass

    def _on_open(self, ws):
        ws.send(
            json.dumps(
                {
                    "action": "subscribe",
                    "subscriptions": [{"topic": "crypto_prices", "type": "update"}],
                }
            )
        )

    def start(self):
        self.running = True
        self.ws = websocket.WebSocketApp(
            "wss://ws-live-data.polymarket.com",
            on_message=self._on_message,
        )
        self.ws.on_open = self._on_open
        self.thread = threading.Thread(target=self.ws.run_forever)
        self.thread.daemon = True
        self.thread.start()

    def stop(self):
        self.running = False
        if self.ws:
            self.ws.close()
        if self.thread:
            self.thread.join(timeout=2)

    def latest(self, sym):
        return self.prices.get(sym), self.timestamps.get(sym)

    def signal(self, sym, window=20):
        """
        Returns (price, momentum_pct, vol_pct, direction, breakout).
        momentum_pct: % change over window ticks from 60-tick SMA (trend).
        vol_pct: ATR-style volatility (% of price).
        direction: 'up' / 'down' / 'ranging'.
        breakout: 'breakout' / 'breakdown' / None.
        """
        h = self.history.get(sym, [])
        if len(h) < window + 10:
            return None, 0.0, 0.0, "ranging", None

        cur = h[-1][1]
        past = h[-(window + 1)][1]
        mom = (cur - past) / max(past, 0.001) * 100

        # ATR-style volatility
        ranges = []
        for i in range(1, min(window + 1, len(h))):
            prev = h[-(window + 1) - i][1]
            curr = h[-(window + 1) - i + 1][1]
            ranges.append(abs(curr - prev) / max(prev, 0.001) * 100)
        vol = (sum(ranges) / len(ranges)) if ranges else 0.0

        # Trend: vs 5min (300 tick) SMA
        avg5m = (
            sum(p for _, p in h[-300:]) / min(len(h[-300:]), 300)
            if len(h) >= 2
            else cur
        )
        direction = "ranging"
        if cur > avg5m * 1.002:
            direction = "up"
        elif cur < avg5m * 0.998:
            direction = "down"

        # Breakout: momentum exceeds volatility threshold
        breakout = None
        if mom > vol * 1.5 and mom > 0.001:
            breakout = "breakout"
        elif mom < -vol * 1.5 and mom < -0.001:
            breakout = "breakdown"

        return cur, round(mom, 4), round(vol, 4), direction, breakout


# ─────────────────────────────────────────────
# Data classes
# ─────────────────────────────────────────────


@dataclass
class IntradayPosition:
    market_id: str
    question: str
    token_id: str
    side: str
    entry_price: float
    size: float
    stop_loss_pct: float
    take_profit_pct: float
    max_hold_secs: int
    how: str
    opened_at: str


@dataclass
class IntradayTrade:
    id: str
    market_id: str
    question: str
    side: str
    entry_price: float
    exit_price: float
    size: float
    pnl: float
    fee: float
    opened_at: str
    closed_at: str
    duration_secs: float
    result: str
    how: str


# ─────────────────────────────────────────────
# Auto-Improvement (GBM simulation)
# ─────────────────────────────────────────────


class AutoImprove:
    GRID = {
        "window": [10, 20, 60],
        "mom_th": [0.05, 0.1, 0.2, 0.5],
        "vol_th": [0.03, 0.1, 0.2],
        "allow_breakout": [True, False],
        "allow_breakdown": [True, False],
        "allow_dip_buy": [True, False],
        "allow_rip_sell": [True, False],
        "stop_loss_pct": [0.5, 1.0, 2.0, 3.0],
        "take_profit_pct": [0.5, 1.0, 2.0, 3.0, 5.0],
        "size_pct": [0.05, 0.10, 0.15],
        "max_hold_secs": [120, 300, 600],
        "symbols": [["btcusdt"], ["ethusdt"], ["solusdt"]],
    }

    VOL = {"btcusdt": 1.5, "ethusdt": 2.0, "solusdt": 3.0}
    BASE = {"btcusdt": 66500, "ethusdt": 1990, "solusdt": 82}

    def score_params(self, params, seed=42):
        rng = random.Random(seed)
        sym = params["symbols"][0]
        V = self.VOL.get(sym, 1.5)
        B = self.BASE.get(sym, 66500)

        # Generate 1hr path (720 ticks × 5s = 1hr simulated)
        path = [B]
        for _ in range(720):
            dW = rng.gauss(0, 1) * V * 0.1
            drift = rng.choice([-0.1, 0, 0, 0.1, -0.05, 0.05])
            p = max(B * 0.8, min(B * 1.2, path[-1] * (1 + drift / 100 + dW / 100)))
            path.append(int(p))

        capital = 100.0
        positions = []
        trades = []
        W = params["window"]
        MOM_TH = params["mom_th"]
        VOL_TH = params["vol_th"]

        for tick in range(W + 1, 720):
            window_slice = path[tick - W - 1 : tick]
            if len(window_slice) < 2:
                continue
            cur = path[tick]
            avg = sum(window_slice) / len(window_slice)
            mom = (cur - avg) / max(avg, 0.001) * 100
            vol_range = (
                (max(window_slice) - min(window_slice))
                / max(min(window_slice), 1)
                * 100
            )

            has_pos = any(p["tick"] > tick - params["max_hold_secs"] for p in positions)

            if not has_pos and len(positions) < 5:
                entry = None
                if mom < -MOM_TH and vol_range > VOL_TH and params["allow_breakdown"]:
                    entry = ("SHORT", tick)
                elif mom > MOM_TH and vol_range > VOL_TH and params["allow_breakout"]:
                    entry = ("LONG", tick)
                elif mom < -MOM_TH * 2 and params["allow_dip_buy"]:
                    entry = ("LONG", tick)
                elif mom > MOM_TH * 2 and params.get("allow_rip_sell", False):
                    entry = ("SHORT", tick)

                if entry:
                    side, entry_tick = entry
                    sz = max(1, int(capital * params["size_pct"] / cur))
                    val = sz * cur
                    if val <= capital * 0.90:
                        positions.append(
                            {
                                "tick": entry_tick,
                                "side": side,
                                "entry": cur,
                                "size": sz,
                                "value": val,
                                "stop": params["stop_loss_pct"],
                                "take": params["take_profit_pct"],
                            }
                        )
                        capital -= val

            next_pos = []
            for pos in positions:
                age = tick - pos["tick"]
                pct = (path[tick] - pos["entry"]) / pos["entry"] * 100
                if pos["side"] == "SHORT":
                    pct = -pct

                exit_now = False
                how = ""
                if pct >= pos["take"]:
                    exit_now = True
                    how = "tp"
                elif pct <= -pos["stop"]:
                    exit_now = True
                    how = "sl"
                elif age >= params["max_hold_secs"]:
                    exit_now = True
                    how = "time"

                if exit_now:
                    exit_p = path[tick]
                    fee = pos["value"] * TAKER_FEE_BPS / 10000
                    net = (pct / 100) * pos["value"] - fee
                    capital += pos["value"] + net
                    result = (
                        "win" if net > 0.01 else "loss" if net < -0.01 else "breakeven"
                    )
                    trades.append({"result": result, "pnl": net})
                else:
                    next_pos.append(pos)
            positions = next_pos

        final_pnl = capital - 100.0
        n = len(trades)
        if n == 0:
            return 0.0, 0.5, 0, 0.0
        wins = sum(1 for t in trades if t["result"] == "win")
        losses = sum(1 for t in trades if t["result"] == "loss")
        wr = wins / max(wins + losses, 1)
        score = final_pnl * wr * 20 + final_pnl * 5 + wr * 20
        return round(final_pnl, 4), round(wr, 3), n, round(score, 6)

    def run(self, generations=5, pop_size=20, verbose=True):
        keys = list(self.GRID.keys())
        vals = list(self.GRID.values())

        population = []
        for _ in range(pop_size):
            d = dict(zip(keys, [random.choice(v) for v in vals]))
            population.append(d)

        best_score = -999.0
        best_params = None
        best_pnl = 0.0
        best_wr = 0.5

        counter = [0]
        for gen in range(generations):
            scored = []
            for params in population:
                try:
                    pnl, wr, n_trades, sc = self.score_params(params, seed=gen * 9999)
                    scored.append((sc, pnl, wr, n_trades, counter[0], params))
                    counter[0] += 1
                except Exception:
                    pass
            scored.sort(reverse=True)

            top_sc, top_pnl, top_wr, top_n, _ct, top_p = scored[0]
            if top_sc > best_score:
                best_score = top_sc
                best_params = top_p
                best_pnl = top_pnl
                best_wr = top_wr

            if verbose:
                print(
                    "  Gen%-2d | score=%+7.3f pnl=$%+6.2f wr=%3.0f%% n=%2d | "
                    "win=%ds mom=%.2f%% vol=%.2f%% SL=%.1f%% TP=%.1f%% sz=%d%% hold=%ds %s"
                    % (
                        gen + 1,
                        top_sc,
                        top_pnl,
                        top_wr * 100,
                        top_n,
                        top_p["window"],
                        top_p["mom_th"],
                        top_p["vol_th"],
                        top_p["stop_loss_pct"],
                        top_p["take_profit_pct"],
                        int(top_p["size_pct"] * 100),
                        top_p["max_hold_secs"],
                        top_p["symbols"][0],
                    )
                )

            next_gen = [top_p]
            for _ in range(pop_size - 1):
                base = random.choice(scored[:5])[5]
                child = {}
                for k in keys:
                    if random.random() < 0.3:
                        child[k] = random.choice(self.GRID[k])
                    else:
                        child[k] = base[k]
                next_gen.append(child)
            population = next_gen

        if verbose:
            print(
                "\n  BEST | score=%+7.3f pnl=$%+6.2f wr=%3.0f%% | win=%ds mom=%.2f%% SL=%.1f%% TP=%.1f%% sz=%d%%"
                % (
                    best_score,
                    best_pnl,
                    best_wr * 100,
                    best_params["window"],
                    best_params["mom_th"],
                    best_params["stop_loss_pct"],
                    best_params["take_profit_pct"],
                    int(best_params["size_pct"] * 100),
                )
            )
            for k, v in best_params.items():
                print("    %s: %s" % (k, v))

        return best_params, best_score


# ─────────────────────────────────────────────
# Live Intraday Trader
# ─────────────────────────────────────────────


class IntradayTrader:
    def __init__(self, params, capital=PAPER_CAPITAL):
        self.params = params
        self.paper_capital = capital
        self.capital = capital
        self.positions = []
        self.trades = []
        self.streamer = RTDSStreamer()
        self._counter = 0
        self._running = False
        self.started_at = datetime.now().isoformat()
        self.fees = 0.0
        self.wins = self.losses = self.breakeven = 0

        LOG_DIR.mkdir(parents=True, exist_ok=True)
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        lf = LOG_DIR / ("intraday5_%s.log" % datetime.now().strftime("%Y%m%d_%H%M%S"))
        logging.basicConfig(
            level=logging.INFO,
            format="%(asctime)s %(message)s",
            handlers=[logging.FileHandler(lf), logging.StreamHandler()],
        )
        self.log = logging.getLogger("intraday5")

    def _id(self):
        self._counter += 1
        return "it_%s_%d" % (datetime.now().strftime("%H%M%S"), self._counter)

    def _now(self):
        return datetime.now().isoformat()

    def _load(self):
        if STATE_FILE.exists():
            with open(STATE_FILE) as f:
                d = json.load(f)
            self.capital = d.get("capital", self.paper_capital)
            self.positions = [IntradayPosition(**p) for p in d.get("positions", [])]
            self.trades = [IntradayTrade(**t) for t in d.get("trades", [])]
            self.wins = d.get("wins", 0)
            self.losses = d.get("losses", 0)
            self.breakeven = d.get("breakeven", 0)
            self.started_at = d.get("started_at", self.started_at)

    def _save(self):
        with open(STATE_FILE, "w") as f:
            json.dump(
                {
                    "capital": self.capital,
                    "positions": [asdict(p) for p in self.positions],
                    "trades": [asdict(t) for t in self.trades],
                    "wins": self.wins,
                    "losses": self.losses,
                    "breakeven": self.breakeven,
                    "started_at": self.started_at,
                    "updated_at": self._now(),
                },
                f,
                indent=2,
            )

    def _journal(self, t):
        with open(JOURNAL_FILE, "a") as f:
            f.write(json.dumps(asdict(t)) + "\n")

    def _tick_positions(self):
        sym = self.params["symbols"][0]
        price, _ = self.streamer.latest(sym)
        if price is None:
            return

        still = []
        for pos in self.positions:
            pct = (price - pos.entry_price) / pos.entry_price * 100
            if pos.side == "SHORT":
                pct = -pct
            age = (
                datetime.now() - datetime.fromisoformat(pos.opened_at)
            ).total_seconds()

            exit_now = False
            how = ""
            if pct >= pos.take_profit_pct:
                exit_now = True
                how = "tp"
            elif pct <= -pos.stop_loss_pct:
                exit_now = True
                how = "sl"
            elif age >= pos.max_hold_secs:
                exit_now = True
                how = "time"

            if exit_now:
                sz_sats = pos.size
                entry_val = sz_sats * pos.entry_price / SATS
                pnl_dollars = sz_sats * (price - pos.entry_price) / SATS if pos.side == "LONG" else sz_sats * (pos.entry_price - price) / SATS
                fee = entry_val * TAKER_FEE_BPS / 10000
                net = pnl_dollars - fee
                self.capital += entry_val + net
                self.fees += fee
                result = "win" if net > 0.01 else "loss" if net < -0.01 else "breakeven"
                if result == "win":
                    self.wins += 1
                elif result == "loss":
                    self.losses += 1
                else:
                    self.breakeven += 1
                exit_price = price if pos.side == "LONG" else price
                t = IntradayTrade(
                    id=self._id(),
                    market_id=pos.market_id,
                    question=pos.question,
                    side=pos.side,
                    entry_price=pos.entry_price,
                    exit_price=exit_price,
                    size=pos.size,
                    pnl=round(net, 4),
                    fee=round(fee, 4),
                    opened_at=pos.opened_at,
                    closed_at=self._now(),
                    duration_secs=round(age, 1),
                    result=result,
                    how=how,
                )
                self.trades.append(t)
                self._journal(t)
                self.log.info(
                    "[EXIT:%s] %s %sx $%.4f->$%.4f pnl=$%.4f %s (%s)"
                    % (
                        str(how.upper()),
                        str(pos.side),
                        str(int(pos.size)),
                        float(pos.entry_price),
                        float(price),
                        float(net),
                        str(result),
                        str(int(age)),
                    )
                )

            else:
                still.append(pos)
        self.positions = still

    # ─────────────────────────────────────────
    # TA Strategy Entry (for evolved params)
    # ─────────────────────────────────────────

    def _scan_and_entry_ta(self):
        """Use evolved TA strategy (RSI/MACD/BB) instead of legacy momentum."""
        if self.capital < MIN_CAPITAL or len(self.positions) >= 5:
            return

        sym = self.params["symbols"][0]
        price, _ = self.streamer.latest(sym)
        if price is None:
            return

        has_pos = any(p.token_id == sym for p in self.positions)
        if has_pos:
            return

        history = self.streamer.history.get(sym, [])
        entry = TAStrategy.check_entry(self.params, history, price)

        if not entry:
            return

        side, reason = entry

        dollar_size = self.capital * self.params.get("size_pct", 0.1)
        sz_sats = int(dollar_size / price * SATS)
        pos_val = sz_sats * price / SATS

        if pos_val > self.capital * 0.90 or sz_sats < 1:
            return

        self.positions.append(
            IntradayPosition(
                market_id=sym,
                question="BTC/USD TA %s" % reason,
                token_id=sym,
                side=side,
                entry_price=price,
                size=sz_sats,
                stop_loss_pct=self.params["stop_loss_pct"],
                take_profit_pct=self.params["take_profit_pct"],
                max_hold_secs=self.params["max_hold_secs"],
                how=reason,
                opened_at=self._now(),
            )
        )
        self.capital -= pos_val
        self.log.info(
            "[ENTRY:TA:%s] %s %d sats @$%.4f | cap=$%.2f"
            % (reason.upper(), side, sz_sats, price, self.capital)
        )

    def _scan_and_entry(self):
        if self.capital < MIN_CAPITAL or len(self.positions) >= 5:
            return

        sym = self.params["symbols"][0]
        price, mom, vol, direction, breakout = self.streamer.signal(
            sym, window=self.params["window"]
        )

        if price is None:
            return

        has_pos = any(p.token_id == sym for p in self.positions)
        if has_pos:
            return

        entry = None
        MOM_TH = self.params["mom_th"]
        VOL_TH = self.params["vol_th"]

        if breakout == "breakdown" and self.params.get("allow_breakdown", True):
            entry = ("SHORT", "breakdown")
        elif breakout == "breakout" and self.params.get("allow_breakout", True):
            entry = ("LONG", "breakout")
        elif (
            direction == "down"
            and mom < -MOM_TH
            and self.params.get("allow_dip_buy", True)
        ):
            entry = ("LONG", "dip_buy")
        elif (
            direction == "up"
            and mom > MOM_TH
            and self.params.get("allow_rip_sell", False)
        ):
            entry = ("SHORT", "rip_sell")

        if not entry:
            return

        side, reason = entry
        sz = max(1, int(self.capital * self.params["size_pct"] / price))
        val = sz * price
        if val > self.capital * 0.90:
            sz = max(1, int(self.capital * 0.85 / price))
            val = sz * price

        if val > self.capital * 0.90 or sz < 1:
            return

        self.positions.append(
            IntradayPosition(
                market_id=sym,
                question="BTC/USD %s mom=%.3f%% vol=%.3f%%" % (reason, mom, vol),
                token_id=sym,
                side=side,
                entry_price=price,
                size=sz,
                stop_loss_pct=self.params["stop_loss_pct"],
                take_profit_pct=self.params["take_profit_pct"],
                max_hold_secs=self.params["max_hold_secs"],
                how=reason,
                opened_at=self._now(),
            )
        )
        self.capital -= val
        self.log.info(
            "[ENTRY:%s] %s %sx @$%.4f mom=%+.3f%% vol=%.3f%% %s | cap=$%.2f"
            % (
                str(reason.upper()),
                str(side),
                str(int(sz)),
                float(price),
                float(mom),
                float(vol),
                str(self.params["symbols"][0]),
                float(self.capital),
            )
        )

    def _status(self):
        pnl = self.capital - self.paper_capital
        days = (
            datetime.now() - datetime.fromisoformat(self.started_at)
        ).total_seconds() / 86400
        daily = pnl / max(days, 0.001)
        wr = self.wins / max(self.wins + self.losses, 1) * 100
        pos_val = sum(p.size * p.entry_price / SATS for p in self.positions)
        self.log.info(
            "\n"
            + "=" * 55
            + "\n"
            + "INTRADAY v5 | "
            + datetime.now().strftime("%H:%M")
            + " | "
            + "win="
            + str(self.params["window"])
            + "s mom="
            + str(float(self.params["mom_th"]))
            + " vol="
            + str(float(self.params["vol_th"]))
            + "\n"
            + "=" * 55
            + "\n"
            + "  Capital $"
            + "%.2f" % self.capital
            + " | PnL $"
            + "%+6.4f" % pnl
            + " ($%+5.2f/day)\n"
            + "  Trades "
            + str(len(self.trades))
            + " W="
            + str(self.wins)
            + " L="
            + str(self.losses)
            + " BE="
            + str(self.breakeven)
            + " | WR="
            + "%.0f" % wr
            + "%\n"
            + "  Fees -$"
            + "%.4f" % self.fees
            + " | Pos "
            + str(len(self.positions))
            + " ($"
            + "%.2f" % pos_val
            + " locked)\n"
            + "=" * 55
        )

    def run(self, poll=5):
        self._running = True
        self._load()
        self.streamer.start()

        def stop(sig, frame):
            self.log.info("Stopping...")
            self._running = False

        signal.signal(signal.SIGINT, stop)
        signal.signal(signal.SIGTERM, stop)

        self.log.info(
            "Intraday v5 | $%.2f virtual | poll=%ds | window=%ds mom_th=%.2f%% "
            "SL=%.1f%% TP=%.1f%% sz=%d%% hold=%ds | %s"
            % (
                self.paper_capital,
                poll,
                self.params["window"],
                self.params["mom_th"],
                self.params["stop_loss_pct"],
                self.params["take_profit_pct"],
                int(self.params["size_pct"] * 100),
                self.params["max_hold_secs"],
                self.params["symbols"][0],
            )
        )

        tick = 0
        use_ta = "strategy" in self.params
        if use_ta:
            self.log.info(
                f"[TA MODE] Strategy: {self.params.get('strategy')} | TF: {self.params.get('timeframe')}"
            )
        else:
            self.log.info(
                "Intraday v5 | $%.2f virtual | poll=%ds | window=%ds mom_th=%.2f%% "
                "SL=%.1f%% TP=%.1f%% sz=%d%% hold=%ds | %s"
                % (
                    self.paper_capital,
                    poll,
                    self.params["window"],
                    self.params["mom_th"],
                    self.params["stop_loss_pct"],
                    self.params["take_profit_pct"],
                    int(self.params["size_pct"] * 100),
                    self.params["max_hold_secs"],
                    self.params["symbols"][0],
                )
            )

        while self._running:
            try:
                tick += 1
                self._tick_positions()
                if tick % 6 == 1:
                    if use_ta:
                        self._scan_and_entry_ta()
                    else:
                        self._scan_and_entry()
                if tick % 20 == 0:
                    try:
                        self._status()
                    except Exception as ex:
                        self.log.error("Status error: " + str(ex))
                if tick % 10 == 0:
                    self._save()
                time.sleep(poll)
            except Exception as e:
                self.log.error("Tick error: " + str(e))
                time.sleep(poll)

        self.streamer.stop()
        try:
            self._save()
        except Exception as ex:
            self.log.error("Save error: " + str(ex))
        try:
            self._status()
        except Exception:
            pass
        self.log.info("Stopped.")


# ─────────────────────────────────────────────
# Main entry point
# ─────────────────────────────────────────────

if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--capital", type=float, default=PAPER_CAPITAL)
    parser.add_argument("--poll", type=int, default=5)
    parser.add_argument("--improve", action="store_true")
    parser.add_argument("--gens", type=int, default=5)
    parser.add_argument("--pop", type=int, default=20)
    ns = parser.parse_args()

    if ns.improve:
        print("=" * 60)
        print("AUTO-IMPROVE | " + str(ns.gens) + " gens x " + str(ns.pop) + " pop")
        print("=" * 60)
        ai = AutoImprove()
        best_params, best_score = ai.run(generations=ns.gens, pop_size=ns.pop)
        with open(BEST_PARAMS, "w") as f:
            json.dump(
                {
                    "params": best_params,
                    "score": best_score,
                    "timestamp": datetime.now().isoformat(),
                },
                f,
                indent=2,
            )
        print("\nSaved to: " + str(BEST_PARAMS))
        print("Starting trader in 2s...")
        time.sleep(2)

    if BEST_PARAMS.exists():
        with open(BEST_PARAMS) as f:
            d = json.load(f)
        params = d["params"]
        print("Loaded best: score=" + str(d["score"]) + " | " + str(params))
    else:
        params = {
            "window": 20,
            "mom_th": 0.1,
            "vol_th": 0.1,
            "allow_breakout": True,
            "allow_breakdown": True,
            "allow_dip_buy": True,
            "allow_rip_sell": False,
            "stop_loss_pct": 2.0,
            "take_profit_pct": 3.0,
            "size_pct": 0.10,
            "max_hold_secs": 600,
            "symbols": ["btcusdt"],
        }
        print("Using default params: " + str(params))

    IntradayTrader(params, capital=ns.capital).run(poll=ns.poll)
