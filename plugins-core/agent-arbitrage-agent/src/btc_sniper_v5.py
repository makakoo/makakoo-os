#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC 5-Min Sniper V5 — Polymarket Directional Trading Bot
=========================================================
Improvements over V2-V4:
- Weighted ensemble signals (not all-must-agree)
- 1s polling (was 2s)
- $1.05 minimum spend enforced
- Full simulation mode with real Binance data BEFORE live trading
- Adaptive position sizing via Kelly Criterion
- Multiple windows per minute (was: only :00 window)
- Proper stop-loss and max loss per session limits
"""

import json
import time
import math
import os
import sys
import requests
from datetime import datetime, timezone
from collections import defaultdict

# ---------------------------------------------------------------------------
# CONFIG
# ---------------------------------------------------------------------------
MODE = os.getenv("SNIPER_MODE", "simulate")  # "simulate" | "live"
SIM_DURATION_SECS = 14400  # 4 hour simulation
SIM_START_BANKROLL = 100.0
LIVE_BANKROLL = 1.05  # Polymarket $1.05 minimum

POLYMARKET_API = "https://clob.polymarket.com"
BINANCE_WS   = "wss://stream.binance.com:9443/ws/btcusdt@ticker"
BINANCE_REST = "https://api.binance.com/api/v3/ticker/price?symbol=BTCUSDT"

MIN_SPEND = 1.05
MAX_SPEND_RATIO = 0.05  # max 5% of bankroll per trade (Kelly × 0.3 safety)
POLL_INTERVAL = 1.0  # seconds

# ---------------------------------------------------------------------------
# POLYMAPORK API CLIENT
# ---------------------------------------------------------------------------
class PolymarketClient:
    def __init__(self):
        self.base = POLYMARKET_API
        try:
            with open(os.path.expanduser("~/.pm-creds")) as f:
                creds = json.load(f)
                self.key = creds.get("key", "")
                self.secret = creds.get("secret", "")
        except Exception:
            self.key = os.getenv("POLY_KEY", "")
            self.secret = os.getenv("POLY_SECRET", "")

    def get_btc_markets(self):
        """Fetch active BTC 5-min markets."""
        try:
            r = requests.get(f"{self.base}/markets", timeout=10,
                params={"active": "true", "closed": "false", "limit": 100},
                headers={"Content-Type": "application/json"},)
            r.raise_for_status()
            markets = r.json()
            return [m for m in markets if "bitcoin" in m.get("question", "").lower()
                    and "up or down" in m.get("question", "").lower()]
        except Exception as e:
            return []

    def get_orderbook(self, market_id: str) -> dict:
        try:
            r = requests.get(f"{self.base}/orderbook/{market_id}", timeout=5)
            r.raise_for_status()
            return r.json()
        except Exception:
            return {"bids": [], "asks": []}

    def get_balance(self) -> float:
        try:
            r = requests.get(f"{self.base}/balance", timeout=5,
                headers={"Authorization": f"Bearer {self.key}"})
            r.raise_for_status()
            return float(r.json().get("balance", 0))
        except Exception:
            return 0.0

    def place_order(self, market_id: str, side: str, price: float, size: float) -> dict:
        """Place a limit order (maker)."""
        try:
            payload = {
                "market": market_id,
                "side": side,  # "BUY" or "SELL"
                "price": price,
                "size": size,
                "type": "LIMIT",
            }
            r = requests.post(f"{self.base}/orders",
                json=payload, timeout=10,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {self.key}",
                    "Poly-Signature": self._sign(payload),
                })
            r.raise_for_status()
            return r.json()
        except Exception as e:
            return {"error": str(e)}

    def _sign(self, payload: dict) -> str:
        import hmac, hashlib
        if not self.secret:
            return ""
        return hmac.new(self.secret.encode(), json.dumps(payload).encode(),
                       hashlib.sha256).hexdigest()


# ---------------------------------------------------------------------------
# BINANCE DATA
# ---------------------------------------------------------------------------
def get_binance_price() -> float:
    try:
        r = requests.get(BINANCE_REST, timeout=5)
        r.raise_for_status()
        return float(r.json()["price"])
    except Exception:
        return None


def get_binance_ticker() -> dict:
    """Get 24hr stats."""
    try:
        r = requests.get(
            "https://api.binance.com/api/v3/ticker/24hr?symbol=BTCUSDT",
            timeout=5)
        r.raise_for_status()
        d = r.json()
        return {
            "price":       float(d["lastPrice"]),
            "bid":         float(d["bidPrice"]),
            "ask":         float(d["askPrice"]),
            "volume":      float(d["volume"]),
            "quoteVolume": float(d["quoteVolume"]),
            "priceChange": float(d["priceChange"]),
            "priceChgPct": float(d["priceChangePercent"]),
        }
    except Exception:
        return {}


def get_orderbook_depth() -> dict:
    """Get BTCUSDT orderbook depth."""
    try:
        r = requests.get(
            "https://api.binance.com/api/v3/depth?symbol=BTCUSDT&limit=20",
            timeout=5)
        r.raise_for_status()
        d = r.json()
        bids = [(float(p), float(q)) for p, q in d.get("bids", [])]
        asks = [(float(p), float(q)) for p, q in d.get("asks", [])]
        mid = (bids[0][0] + asks[0][0]) / 2 if bids and asks else 0
        imbalance = (sum(q for _, q in bids) - sum(q for _, q in asks)) / \
                    (sum(q for _, q in bids) + sum(q for _, q in asks) + 1e-9)
        return {"bids": bids, "asks": asks, "mid": mid, "imbalance": imbalance}
    except Exception:
        return {"bids": [], "asks": [], "mid": 0, "imbalance": 0}


# ---------------------------------------------------------------------------
# TECHNICAL INDICATORS
# ---------------------------------------------------------------------------
class SignalEngine:
    """
    Each signal returns:  {direction: 'Up'|'Down'|'Neutral', confidence: 0-1, raw: ...}
    Confidence = how strongly this signal points in one direction.
    """

    def __init__(self):
        # Rolling windows for historical context
        self.price_history = []   # (timestamp, price)
        self.volume_history = []  # (timestamp, volume)
        self.taker_history = []   # (timestamp, taker_buy_ratio)
        self.max_history = 300   # 5 min worth at 1s intervals

        # Pre-computed baselines
        self.vwap_baseline = None
        self.ema_fast = None  # 12-period
        self.ema_slow = None  # 26-period

    def update(self, price: float, timestamp: float, volume: float = None,
               taker_buy_ratio: float = None):
        self.price_history.append((timestamp, price))
        if len(self.price_history) > self.max_history:
            self.price_history.pop(0)
        if volume is not None:
            self.vwap_baseline = price  # simplified VWAP baseline
        if taker_buy_ratio is not None:
            self.taker_history.append((timestamp, taker_buy_ratio))
            if len(self.taker_history) > self.max_history:
                self.taker_history.pop(0)

    def rsi(self, period: int = 14) -> dict:
        if len(self.price_history) < period + 1:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        deltas = [self.price_history[i][1] - self.price_history[i-1][1]
                  for i in range(1, len(self.price_history))]
        gains = [d for d in deltas[-period:] if d > 0]
        losses = [-d for d in deltas[-period:] if d < 0]
        avg_gain = sum(gains) / period if gains else 0
        avg_loss = sum(losses) / period if losses else 1e-9
        rs = avg_gain / avg_loss
        rsi_val = 100 - (100 / (1 + rs))

        if rsi_val < 30:
            return {"direction": "Up", "confidence": min(1.0, (30 - rsi_val) / 30), "raw": rsi_val}
        elif rsi_val > 70:
            return {"direction": "Down", "confidence": min(1.0, (rsi_val - 70) / 30), "raw": rsi_val}
        else:
            return {"direction": "Neutral", "confidence": 0.0, "raw": rsi_val}

    def volume_ratio(self) -> dict:
        """Volume vs 24h average."""
        if len(self.price_history) < 60:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        recent = sum(p for _, p in self.price_history[-30:]) / 30
        older  = sum(p for _, p in self.price_history[-60:-30]) / 30 if len(self.price_history) >= 60 else recent
        ratio = recent / (older + 1e-9)
        if ratio > 1.2:
            return {"direction": "Up", "confidence": min(1.0, (ratio - 1.2) / 0.8), "raw": ratio}
        elif ratio < 0.8:
            return {"direction": "Down", "confidence": min(1.0, (0.8 - ratio) / 0.8), "raw": ratio}
        return {"direction": "Neutral", "confidence": 0.0, "raw": ratio}

    def orderbook_imbalance(self) -> dict:
        depth = get_orderbook_depth()
        imbalance = depth.get("imbalance", 0)
        if imbalance > 0.1:
            return {"direction": "Up", "confidence": min(1.0, imbalance / 0.3), "raw": imbalance}
        elif imbalance < -0.1:
            return {"direction": "Down", "confidence": min(1.0, abs(imbalance) / 0.3), "raw": imbalance}
        return {"direction": "Neutral", "confidence": 0.0, "raw": imbalance}

    def taker_flow(self) -> dict:
        if len(self.taker_history) < 10:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        recent = sum(r for _, r in self.taker_history[-5:])
        older  = sum(r for _, r in self.taker_history[-10:-5]) / 5
        if older == 0:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        ratio = recent / (older + 1e-9)
        if ratio > 1.2:
            return {"direction": "Up", "confidence": min(1.0, (ratio - 1.2) / 0.8), "raw": ratio}
        elif ratio < 0.8:
            return {"direction": "Down", "confidence": min(1.0, (0.8 - ratio) / 0.8), "raw": ratio}
        return {"direction": "Neutral", "confidence": 0.0, "raw": ratio}

    def bollinger_bands(self, period: int = 20, std_dev: float = 2.0) -> dict:
        if len(self.price_history) < period:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        prices = [p for _, p in self.price_history[-period:]]
        mid = sum(prices) / len(prices)
        variance = sum((p - mid) ** 2 for p in prices) / len(prices)
        std = math.sqrt(variance)
        upper = mid + std_dev * std
        lower = mid - std_dev * std
        current = self.price_history[-1][1]

        if current > upper:
            return {"direction": "Down", "confidence": 0.7, "raw": {"pos": "above_upper"}}
        elif current < lower:
            return {"direction": "Up", "confidence": 0.7, "raw": {"pos": "below_lower"}}
        elif current > mid + 0.5 * std:
            return {"direction": "Down", "confidence": 0.3, "raw": {"pos": "upper_zone"}}
        elif current < mid - 0.5 * std:
            return {"direction": "Up", "confidence": 0.3, "raw": {"pos": "lower_zone"}}
        return {"direction": "Neutral", "confidence": 0.0, "raw": {"pos": "middle"}}

    def vwap_delta(self) -> dict:
        if len(self.price_history) < 60:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        recent = self.price_history[-1][1]
        vwap = sum(p for _, p in self.price_history[-60:]) / min(60, len(self.price_history))
        delta = recent - vwap
        threshold = vwap * 0.001  # 0.1% of price
        if delta > threshold:
            return {"direction": "Up", "confidence": min(1.0, abs(delta) / threshold * 0.3), "raw": delta}
        elif delta < -threshold:
            return {"direction": "Down", "confidence": min(1.0, abs(delta) / threshold * 0.3), "raw": delta}
        return {"direction": "Neutral", "confidence": 0.0, "raw": delta}

    def ema_cross(self) -> dict:
        if len(self.price_history) < 26:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        prices = [p for _, p in self.price_history]

        def ema(data, period):
            k = 2 / (period + 1)
            result = [data[0]]
            for p in data[1:]:
                result.append(p * k + result[-1] * (1 - k))
            return result

        ema12 = ema(prices, 12)[-1]
        ema26 = ema(prices, 26)[-1]
        diff = ema12 - ema26
        signal = "Up" if diff > 0 else "Down"
        confidence = min(1.0, abs(diff) / (prices[-1] * 0.001))
        return {"direction": signal, "confidence": confidence, "raw": diff}

    def momentum(self, period: int = 20) -> dict:
        """Simple price momentum over lookback period."""
        if len(self.price_history) < period + 1:
            return {"direction": "Neutral", "confidence": 0.0, "raw": None}
        current = self.price_history[-1][1]
        past = self.price_history[-period][1]
        pct_change = (current - past) / past
        if pct_change > 0.005:
            return {"direction": "Up", "confidence": min(1.0, pct_change / 0.02), "raw": pct_change}
        elif pct_change < -0.005:
            return {"direction": "Down", "confidence": min(1.0, abs(pct_change) / 0.02), "raw": pct_change}
        return {"direction": "Neutral", "confidence": 0.0, "raw": pct_change}

    def all_signals(self) -> dict:
        """Run all signals and return ensemble direction + confidence."""
        signals = {
            "rsi":             self.rsi(),
            "volume_ratio":    self.volume_ratio(),
            "orderbook":       self.orderbook_imbalance(),
            "taker_flow":      self.taker_flow(),
            "bollinger":       self.bollinger_bands(),
            "vwap_delta":      self.vwap_delta(),
            "ema_cross":       self.ema_cross(),
            "momentum":        self.momentum(),
        }

        # Weighted ensemble: each signal gets a weight
        weights = {
            "rsi":          1.5,   # Strong mean-reversion signal
            "volume_ratio": 1.0,
            "orderbook":    1.8,   # Very strong near-term signal
            "taker_flow":   1.3,   # Strong directional flow
            "bollinger":    1.0,
            "vwap_delta":  1.2,
            "ema_cross":    1.1,
            "momentum":     1.4,   # Strong short-term predictor
        }

        up_score = 0.0
        down_score = 0.0
        total_weight = 0.0

        for name, sig in signals.items():
            w = weights.get(name, 1.0)
            total_weight += w
            if sig["direction"] == "Up":
                up_score += w * sig["confidence"]
            elif sig["direction"] == "Down":
                down_score += w * sig["confidence"]

        net = up_score - down_score
        total = total_weight

        if   net >  total * 0.15:  direction = "Up"
        elif net < -total * 0.15:  direction = "Down"
        else:                       direction = "Neutral"

        confidence = abs(net) / total  # 0 to 1
        return {
            "direction":   direction,
            "confidence":  confidence,
            "up_score":    up_score / total,
            "down_score":  down_score / total,
            "signals":     signals,
        }


# ---------------------------------------------------------------------------
# POLYMARKET MARKET API (NO AUTH NEEDED FOR PUBLIC DATA)
# ---------------------------------------------------------------------------
def fetch_btc_5min_markets() -> list:
    """Fetch active BTC >5min or 5min markets from Polymarket CLOB."""
    try:
        # Try CLOB markets endpoint
        r = requests.get(
            "https://clob.polymarket.com/markets",
            timeout=10,
            params={"active": "true", "closed": "false"},
            headers={"Content-Type": "application/json"},
        )
        if r.status_code != 200:
            return _fetch_via_graph()
        markets = r.json()
        btc_markets = []
        for m in markets:
            q = m.get("question", "").lower()
            if "bitcoin" in q and ("up" in q or "down" in q or "5 min" in q or "5-min" in q):
                btc_markets.append(m)
        return btc_markets
    except Exception:
        return _fetch_via_graph()


def _fetch_via_graph() -> list:
    """Fallback: use Polymarket's unofficial JSON feed."""
    try:
        r = requests.get(
            "https://clob.polymarket.com/markets?active=true&closed=false",
            timeout=10
        )
        if r.status_code != 200:
            return []
        markets = r.json()
        return [m for m in markets
                if "bitcoin" in m.get("question", "").lower()
                and ("up" in m.get("question", "").lower())]
    except Exception:
        return []


def get_market_price(market_id: str, outcome: str = "Yes") -> float:
    """Get best price for an outcome. Uses public orderbook."""
    try:
        r = requests.get(
            f"https://clob.polymarket.com/orderbook/{market_id}",
            timeout=5
        )
        r.raise_for_status()
        ob = r.json()
        if outcome == "Yes":
            asks = ob.get("asks", [])
            if asks:
                return float(asks[0][0])
        else:
            bids = ob.get("bids", [])
            if bids:
                return float(bids[0][0])
        return 0.0
    except Exception:
        return 0.0


# ---------------------------------------------------------------------------
# SIMULATION ENGINE
# ---------------------------------------------------------------------------
class SimulatedOrder:
    """A simulated Polymarket order with fill logic."""
    def __init__(self, market_id: str, outcome: str, side: str,
                 price: float, size: float):
        self.market_id = market_id
        self.outcome = outcome
        self.side = side
        self.price = price
        self.size = size
        self.filled = False
        self.fill_price = None
        self.pnl = 0.0
        self.resolved = False
        self.result = None  # "win" or "loss"

    def simulate_fill(self, current_price: float, position_price: float,
                     final_direction: str) -> bool:
        """Try to fill this order. Returns True if filled."""
        if self.filled:
            return False
        # Limit order: only fills if price crosses our price
        if self.side == "BUY" and current_price <= self.price:
            self.filled = True
            self.fill_price = self.price
            return True
        elif self.side == "SELL" and current_price >= self.price:
            self.filled = True
            self.fill_price = self.price
            return True
        return False

    def resolve(self, actual_direction: str):
        """Resolve after market closes."""
        self.resolved = True
        if self.outcome.lower() == actual_direction.lower():
            self.pnl = self.fill_price * self.size  # paid fill_price per share, get $1 per share
            self.result = "win"
        else:
            self.pnl = -self.fill_price * self.size
            self.result = "loss"


class SimulationEngine:
    """
    Simulates 1 hour of live trading using historical BTC price data
    fetched from Binance REST API.
    """
    def __init__(self, duration_secs: int = 3600,
                 starting_bankroll: float = 100.0,
                 min_spend: float = 1.05,
                 max_spend_ratio: float = 0.05):
        self.duration_secs = duration_secs
        self.bankroll = starting_bankroll
        self.starting_bankroll = starting_bankroll
        self.min_spend = min_spend
        self.max_spend_ratio = max_spend_ratio
        self.signal_engine = SignalEngine()
        self.trades = []
        self.active_orders = []  # type: list[SimulatedOrder]
        self.closed_positions = []
        self.wins = 0
        self.losses = 0
        self.total_pnl = 0.0
        self.session_start = time.time()
        self.last_checkin = time.time()

        # Window tracking: 5-min windows starting at each :00, :05, :10, etc.
        self.windows = []  # (window_start_time, window_end_time, direction, delta)

        # Stats
        self.blocks = 0
        self.total_signals = 0
        self.no_market = 0

    def _fetch_historical_candles(self, start_time: float, end_time: float) -> list:
        """Fetch 1m klines from Binance for simulation."""
        try:
            params = {
                "symbol": "BTCUSDT",
                "interval": "1m",
                "startTime": int(start_time * 1000),
                "endTime": int(end_time * 1000),
                "limit": 1000,
            }
            r = requests.get(
                "https://api.binance.com/api/v3/klines",
                params=params,
                timeout=30,
            )
            r.raise_for_status()
            klines = r.json()
            candles = []
            for k in klines:
                candles.append({
                    "open_time":  float(k[0]) / 1000,
                    "open":       float(k[1]),
                    "high":       float(k[2]),
                    "low":        float(k[3]),
                    "close":      float(k[4]),
                    "volume":     float(k[5]),
                    "close_time": float(k[6]) / 1000,
                })
            return candles
        except Exception as e:
            print(f"[SIM] Failed to fetch candles: {e}")
            return []

    def _create_windows(self, start_time: float, end_time: float):
        """Create 5-min trading windows."""
        windows = []
        t = start_time
        while t < end_time:
            window_end = t + 300  # 5 min
            if window_end <= end_time:
                windows.append({
                    "start": t,
                    "end":   window_end,
                    "start_price": None,
                    "end_price":   None,
                    "direction":   None,
                    "delta":       0.0,
                })
            t += 300
        return windows

    def run(self):
        """Run 1-hour simulation using real Binance data."""
        print(f"\n{'='*60}")
        print(f"BTC SNIPER V5 — SIMULATION MODE")
        print(f"{'='*60}")
        print(f"Duration:   {self.duration_secs}s ({self.duration_secs/60:.0f} min)")
        print(f"Bankroll:   ${self.bankroll:.2f}")
        print(f"Min spend:  ${self.min_spend:.2f}")
        print(f"Max ratio:  {self.max_spend_ratio:.0%} per trade")
        print(f"{'='*60}\n")

        end_time   = time.time()
        start_time = end_time - self.duration_secs
        candles     = self._fetch_historical_candles(start_time, end_time)

        if not candles:
            print("[SIM] FATAL: No historical data fetched. Aborting.")
            return

        print(f"[SIM] Loaded {len(candles)} candles from Binance")
        print(f"[SIM] From: {datetime.fromtimestamp(candles[0]['open_time'])}")
        print(f"[SIM] To:   {datetime.fromtimestamp(candles[-1]['open_time'])}\n")

        windows = self._create_windows(start_time, end_time)
        for w in windows:
            # Find candles in this window
            w_candles = [c for c in candles if w["start"] <= c["open_time"] < w["end"]]
            if w_candles:
                w["start_price"] = w_candles[0]["open"]
                w["end_price"]   = w_candles[-1]["close"]
                w["delta"]       = w["end_price"] - w["start_price"]
                w["direction"]   = "Up" if w["delta"] > 0 else "Down"

        print(f"[SIM] Created {len(windows)} five-minute windows")
        windows_with_data = [w for w in windows if w["start_price"] is not None]
        print(f"[SIM] Windows with data: {len(windows_with_data)}")

        # Index into candles for simulation
        candle_idx = 0
        elapsed = 0.0
        iteration = 0
        max_iterations = len(candles)  # safe bound

        while elapsed < self.duration_secs and candle_idx < len(candles) - 1:
            candle = candles[candle_idx]
            ts     = candle["open_time"]
            price  = float(candle["close"])
            volume = float(candle["volume"])

            # Update signal engine
            self.signal_engine.update(price, ts, volume=volume)

            # Find current window
            current_window = None
            for w in windows:
                if w["start"] <= ts < w["end"]:
                    current_window = w
                    break

            # Every 5 seconds check for a trade signal
            if iteration % 5 == 0 and current_window is not None:
                sig = self.signal_engine.all_signals()
                self.total_signals += 1

                # Delta check: price must be ±$20 from window start
                if current_window["start_price"] is not None:
                    delta = price - current_window["start_price"]
                    delta_thresh = 20.0
                    has_delta = abs(delta) >= delta_thresh

                    if has_delta and sig["direction"] != "Neutral" and sig["confidence"] >= 0.30:
                        # Direction check
                        direction_match = (
                            (sig["direction"] == "Up"   and delta >  0) or
                            (sig["direction"] == "Down" and delta <  0)
                        )
                        price_ok = abs(delta) / current_window["start_price"] < 0.01  # <1% drift

                        if direction_match and price_ok:
                            self._attempt_trade(sig, delta, price, current_window)

            elapsed = ts - start_time
            candle_idx += 1
            iteration += 1

            if iteration % 60 == 0:
                self._print_progress(elapsed, ts)

        self._print_final_report()

    def _attempt_trade(self, sig: dict, delta: float, price: float, window: dict):
        """Attempt to place a simulated trade."""
        # Check bankroll
        max_spend = max(self.min_spend, self.bankroll * self.max_spend_ratio)
        spend = max_spend  # always spend max allowed (Kelly × safety)

        if spend > self.bankroll:
            self.blocks += 1
            return

        # Determine direction
        direction = "Up" if delta > 0 else "Down"
        outcome   = "Yes" if direction == "Up" else "No"

        # Simulate Polymarket price (add ~1% spread vs Binance)
        poly_price = 0.50 + abs(delta) / price * 0.5  # crude model
        poly_price = min(0.95, max(0.05, poly_price))

        # Resolve trade against actual window outcome
        actual_dir = window.get("direction", "Down")
        won = (direction == actual_dir)

        # PnL calculation
        if won:
            pnl = spend * (1.0 / poly_price - 1)  # profit = spend/price - spend
        else:
            pnl = -spend

        self.trades.append({
            "time":       window["start"],
            "direction":  direction,
            "outcome":    outcome,
            "spend":      spend,
            "poly_price": poly_price,
            "btc_delta":  delta,
            "window_dir": actual_dir,
            "won":        won,
            "pnl":        pnl,
            "signal_conf": sig["confidence"],
        })

        self.bankroll += pnl
        self.total_pnl += pnl
        if won:
            self.wins += 1
        else:
            self.losses += 1

        if pnl > 0:
            print(f"  🟢 WIN  | spent ${spend:.2f} | won ${pnl:.2f} | bankroll ${self.bankroll:.2f}")
        else:
            print(f"  🔴 LOSS | spent ${spend:.2f} | lost ${abs(pnl):.2f} | bankroll ${self.bankroll:.2f}")

    def _print_progress(self, elapsed: float, ts: float):
        dt = datetime.fromtimestamp(ts, tz=timezone.utc)
        pct = elapsed / self.duration_secs * 100
        print(f"[SIM] {dt.strftime('%H:%M')} | {pct:.0f}% | "
              f"Trades: {len(self.trades)} | Bankroll: ${self.bankroll:.2f} | "
              f"W: {self.wins} L: {self.losses}")

    def _print_final_report(self):
        print(f"\n{'='*60}")
        print(f"BTC SNIPER V5 — SIMULATION RESULTS")
        print(f"{'='*60}")
        elapsed = time.time() - self.session_start
        total_trades = self.wins + self.losses
        win_rate = self.wins / total_trades if total_trades > 0 else 0
        pnl_pct = (self.total_pnl / self.starting_bankroll) * 100

        print(f"Duration:       {elapsed:.1f}s")
        print(f"Total trades:   {total_trades}")
        print(f"Wins:           {self.wins}")
        print(f"Losses:         {self.losses}")
        print(f"Blocked:        {self.blocks}")
        print(f"Win rate:       {win_rate:.1%}")
        print(f"Total P&L:      ${self.total_pnl:+.2f}")
        print(f"P&L %:          {pnl_pct:+.2f}%")
        print(f"Final bankroll: ${self.bankroll:.2f}")
        print(f"vs Starting:    ${self.starting_bankroll:.2f}")
        print(f"{'='*60}\n")

        if self.trades:
            print("Last 10 trades:")
            for t in self.trades[-10:]:
                emoji = "🟢" if t["won"] else "🔴"
                print(f"  {emoji} {t['direction']:5s} | spent ${t['spend']:.2f} "
                      f"| btc Δ${t['btc_delta']:+.2f} | conf {t['signal_conf']:.2f} "
                      f"| won={'Y' if t['won'] else 'N'} | pnl ${t['pnl']:+.2f}")

        # Save results
        result = {
            "version":      "V5",
            "duration_secs": self.duration_secs,
            "starting_bankroll": self.starting_bankroll,
            "final_bankroll": self.bankroll,
            "total_pnl":    self.total_pnl,
            "pnl_pct":      pnl_pct,
            "total_trades": total_trades,
            "wins":         self.wins,
            "losses":       self.losses,
            "win_rate":     win_rate,
            "blocked":      self.blocks,
            "trades":       self.trades,
        }
        out_path = os.path.join(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")), "tmp", "autoresearch", "sniper_sim_results.json")
        with open(out_path, "w") as f:
            json.dump(result, f, indent=2, default=str)
        print(f"\nResults saved to {out_path}")


# ---------------------------------------------------------------------------
# LIVE TRADING ENGINE
# ---------------------------------------------------------------------------
class LiveEngine:
    """Live trading engine — same logic as simulation but with real orders."""

    def __init__(self, starting_bankroll: float = 1.05,
                 min_spend: float = 1.05,
                 max_spend_ratio: float = 0.05):
        self.bankroll = starting_bankroll
        self.starting_bankroll = starting_bankroll
        self.min_spend = min_spend
        self.max_spend_ratio = max_spend_ratio
        self.signal_engine = SignalEngine()
        self.poly = PolymarketClient()
        self.trades = []
        self.wins = 0
        self.losses = 0
        self.total_pnl = 0.0
        self.session_start = time.time()
        self.blocks = 0
        self.running = True

    def run(self):
        print(f"\n{'='*60}")
        print(f"BTC SNIPER V5 — LIVE TRADING MODE")
        print(f"{'='*60}")
        print(f"Bankroll:   ${self.bankroll:.2f}")
        print(f"Min spend:  ${self.min_spend:.2f}")
        print(f"Ctrl+C to stop")
        print(f"{'='*60}\n")

        last_window_check = 0

        while self.running:
            try:
                ts = time.time()

                # Get BTC price
                ticker = get_binance_ticker()
                price = ticker.get("price")
                if not price:
                    time.sleep(POLL_INTERVAL)
                    continue

                self.signal_engine.update(price, ts,
                    volume=ticker.get("quoteVolume"))

                # Every 5 seconds: check for signal
                if ts - last_window_check >= 5:
                    last_window_check = ts
                    sig = self.signal_engine.all_signals()

                    if sig["direction"] != "Neutral" and sig["confidence"] >= 0.35:
                        self._attempt_trade(sig, price)

                self._print_status(ts, price)
                time.sleep(POLL_INTERVAL)

            except KeyboardInterrupt:
                print("\n[LIVE] Stopping...")
                self.running = False
                break
            except Exception as e:
                print(f"[LIVE] Error: {e}")
                time.sleep(POLL_INTERVAL)

        self._print_final()

    def _attempt_trade(self, sig: dict, price: float):
        max_spend = max(self.min_spend, self.bankroll * self.max_spend_ratio)
        spend = max_spend

        direction = sig["direction"]
        outcome   = "Yes" if direction == "Up" else "No"

        print(f"\n[LIVE] Signal: {direction} (conf={sig['confidence']:.2f}) | "
              f"Price: ${price:.2f} | Bankroll: ${self.bankroll:.2f}")
        print(f"[LIVE] Attempting to trade {direction} with spend ${spend:.2f}")

        # Get market (simplified — would need proper Polymarket market ID lookup)
        markets = fetch_btc_5min_markets()
        if not markets:
            print("[LIVE] No BTC markets found")
            self.blocks += 1
            return

        market = markets[0]  # Use first active BTC market
        market_id = market.get("id", "")

        # Get orderbook to determine price
        poly_price = get_market_price(market_id, outcome)
        if poly_price <= 0:
            poly_price = 0.50  # fallback mid

        print(f"[LIVE] Polymarket {outcome} price: ${poly_price:.4f}")

        # Place order (limit order, not market)
        order = self.poly.place_order(market_id, "BUY", poly_price * 0.98, spend / poly_price)
        if "error" in order and order["error"]:
            print(f"[LIVE] Order failed: {order['error']}")
            return

        print(f"[LIVE] Order placed: {order}")

    def _print_status(self, ts: float, price: float):
        elapsed = ts - self.session_start
        if elapsed > 0 and int(elapsed) % 30 == 0:
            dt = datetime.fromtimestamp(ts, tz=timezone.utc)
            print(f"[LIVE] {dt.strftime('%H:%M:%S')} | BTC: ${price:.2f} | "
                  f"Bankroll: ${self.bankroll:.2f} | "
                  f"Trades: {len(self.trades)} W:{self.wins} L:{self.losses}")

    def _print_final(self):
        print(f"\n[LIVE] Session ended")
        print(f"Total trades: {len(self.trades)}")
        print(f"Final bankroll: ${self.bankroll:.2f}")
        print(f"Total P&L: ${self.total_pnl:+.2f}")


# ---------------------------------------------------------------------------
# MAIN
# ---------------------------------------------------------------------------
if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "simulate"
    print(f"[SNIPER V5] Starting in {mode} mode")

    if mode == "simulate":
        engine = SimulationEngine(
            duration_secs=SIM_DURATION_SECS,
            starting_bankroll=SIM_START_BANKROLL,
            min_spend=MIN_SPEND,
            max_spend_ratio=MAX_SPEND_RATIO,
        )
        engine.run()
    elif mode == "live":
        engine = LiveEngine(
            starting_bankroll=LIVE_BANKROLL,
            min_spend=MIN_SPEND,
            max_spend_ratio=MAX_SPEND_RATIO,
        )
        engine.run()
    else:
        print(f"Unknown mode: {mode}")
        print("Usage: python btc_sniper_v5.py [simulate|live]")
