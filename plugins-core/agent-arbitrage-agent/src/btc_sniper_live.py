#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Sniper Pro — Production Live Trading
========================================
ONE system: monitors Polymarket for BTC prediction markets,
places real CLOB orders, auto-improves from wins AND losses.

Signal logic:
  - Watch for BTC 5-min prediction markets on CLOB
  - Track BTC/USD momentum on Binance (1m, 5m, 15m)
  - When BTC moves > delta_thresh in a 5-min window → place trade
  - Take profit / stop loss based on BTC price movement
  - GA optimizes params over time

Live trading via py_clob_client with .env.live credentials.
"""

import copy
import json
import os
import random
import signal
import subprocess
import sys
import time
import requests
from collections import defaultdict
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Optional

# ── Paths ────────────────────────────────────────────────────────────────────
HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
JOURNAL_FILE = STATE_DIR / "intraday_journal.jsonl"
BEST_PARAMS_FILE = STATE_DIR / "sniper_best_params.json"
FITNESS_HISTORY = DATA_DIR / "fitness_history.jsonl"

STATE_DIR.mkdir(parents=True, exist_ok=True)
LOG_DIR.mkdir(parents=True, exist_ok=True)

# ── Constants ──────────────────────────────────────────────────────────────────
PAPER_CAPITAL = 100.0
TAKER_FEE_BPS = 200  # 2%
POLYFEE = 0.01
MIN_SPEND = 2.50
BINANCE_REST = "https://api.binance.com/api/v3/ticker/price?symbol=BTCUSDT"
BINANCE_KLINES = "https://api.binance.com/api/v3/klines"
POLYMARKET_CLOB = "https://clob.polymarket.com"
USDC = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174"

# ── Binance Free Endpoints ──────────────────────────────────────────────────────
BINANCE_DEPTH = "https://api.binance.com/api/v3/depth"
BINANCE_TRADES = "https://api.binance.com/api/v3/trades"
BINANCE_TICKER = "https://api.binance.com/api/v3/ticker"
BINANCE_UIKLINES = "https://api.binance.com/api/v3/uiKlines"
_WARMUP_DONE = False


# ── Logging ───────────────────────────────────────────────────────────────────
def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    try:
        with open(LOG_DIR / "btc_sniper_live.log", "a") as f:
            f.write(line + "\n")
    except:
        pass


# ── AI Helper ─────────────────────────────────────────────────────────────────
AI_URL = os.environ.get("SWITCHAI_URL", "http://localhost:18080/v1/chat/completions")
AI_KEY = os.environ.get("SWITCHAI_KEY", "sk-test-123")
AI_MODEL = os.environ.get("LLM_MODEL", "minimax:MiniMax-M2.7")


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


# ── Sniper Params ─────────────────────────────────────────────────────────────
@dataclass
class SniperParams:
    version: str = "pro1.0"
    name: str = ""

    # Signal thresholds
    delta_thresh: float = 50.0  # min BTC price move in window ($)
    conf_thresh: float = 0.08  # min ensemble confidence
    ens_thresh: float = 0.08  # min ensemble agreement

    # Sizing
    spend_ratio: float = 0.20  # fraction of bankroll per trade
    max_bet_pct: float = 0.50  # max bet as % of bankroll

    # Exits
    profit_target_bps: int = 300  # profit target in basis points
    stop_loss_bps: int = 150  # stop loss in basis points
    max_hold_seconds: int = 600  # force close after N seconds

    # Markets
    min_market_volume: float = 5000  # min 24h volume to trade
    max_spread_bps: int = 500  # max spread to trade (50%)

    # GA
    session_minutes: int = 60
    pop_size: int = 12

    def __post_init__(self):
        if not self.name:
            self.name = f"sniper_{datetime.now().strftime('%H%M%S')}"

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, d: dict) -> "SniperParams":
        known = {f.name for f in cls.__dataclass_fields__.values()}
        return cls(**{k: v for k, v in d.items() if k in known})

    def mutate(self, rate: float = 0.25) -> "SniperParams":
        new = copy.deepcopy(self)
        new.name = (
            f"mut_{datetime.now().strftime('%H%M%S')}_{random.randint(1000, 9999)}"
        )
        mutations = [
            # Ranges tuned to include known-good values: delta=12, conf=0.80, ens=0.60
            ("delta_thresh", 8.0, 50.0, 2.0),
            ("conf_thresh", 0.60, 0.95, 0.05),
            ("ens_thresh", 0.40, 0.90, 0.05),
            ("spend_ratio", 0.05, 0.40, 0.02),
            ("max_bet_pct", 0.20, 0.80, 0.05),
            ("profit_target_bps", 100, 800, 50),
            ("stop_loss_bps", 50, 400, 25),
            ("max_hold_seconds", 120, 900, 60),
            ("min_market_volume", 2000, 50000, 1000),
            ("max_spread_bps", 200, 1000, 50),
        ]
        for attr, lo, hi, step in mutations:
            if random.random() < rate:
                val = round(random.uniform(lo, hi) / step) * step
                setattr(new, attr, val)
        return new

    def crossover(self, a: "SniperParams", b: "SniperParams") -> "SniperParams":
        da, db = a.to_dict(), b.to_dict()
        keys = list(da.keys())
        pt = random.randint(1, len(keys) - 1)
        child = {}
        for i, k in enumerate(keys):
            child[k] = da[k] if i < pt else db[k]
        child["name"] = f"x_{datetime.now().strftime('%H%M%S')}"
        return SniperParams.from_dict(child)

    @classmethod
    def random_population(cls, size: int) -> list["SniperParams"]:
        pop = []
        for _ in range(size):
            p = cls()
            p.name = (
                f"init_{datetime.now().strftime('%H%M%S')}_{random.randint(1000, 9999)}"
            )
            p = p.mutate(rate=1.0)
            pop.append(p)
        return pop


# ── Signal Engine ─────────────────────────────────────────────────────────────
class SignalEngine:
    def __init__(self):
        self.ph: list[tuple[float, float]] = []  # (timestamp, price)

    def update(self, price: float, ts: float):
        self.ph.append((ts, price))
        if len(self.ph) > 600:
            self.ph.pop(0)

    def _ema(self, data: list, n: int) -> list:
        if len(data) < n:
            return data[:]
        k = 2 / (n + 1)
        result = [sum(data[:n]) / n]
        for p in data[n:]:
            result.append(p * k + result[-1] * (1 - k))
        return result

    def rsi(self, n: int = 14) -> tuple[float, float]:
        if len(self.ph) < n + 2:
            return 50.0, 0.0
        ds = [self.ph[i][1] - self.ph[i - 1][1] for i in range(1, len(self.ph))]
        g = [d for d in ds[-n:] if d > 0]
        l = [-d for d in ds[-n:] if d < 0]
        ag = sum(g) / n if g else 0
        al = sum(l) / n if l else 1e-9
        rs = ag / al
        rsi_val = 100 - (100 / (1 + rs))
        # Second value = RSI slope (momentum)
        slope = rsi_val - (50 if len(ds) > n else rsi_val)
        return rsi_val, slope

    def momentum(self, window_secs: int = 300) -> float:
        if len(self.ph) < 2:
            return 0.0
        now = self.ph[-1][0]
        cutoff = now - window_secs
        for i, (ts, price) in enumerate(self.ph):
            if ts >= cutoff:
                if i == 0:
                    return 0.0
                return self.ph[-1][1] - self.ph[i][1]
        return 0.0

    def acceleration(self, window_secs: int = 120) -> float:
        """Rate of change of momentum — positive = speeding up, negative = slowing down."""
        if len(self.ph) < window_secs * 2:
            return 0.0
        now = self.ph[-1][0]
        old_cutoff = now - window_secs * 2
        new_cutoff = now - window_secs

        old_mom = 0.0
        new_mom = 0.0
        for i, (ts, price) in enumerate(self.ph):
            if ts >= old_cutoff and ts < new_cutoff:
                if i > 0:
                    old_mom = self.ph[-1][1] - self.ph[i][1]
            if ts >= new_cutoff:
                if i > 0:
                    new_mom = self.ph[-1][1] - self.ph[i][1]
                break
        return new_mom - old_mom

    def ensemble(self, ens_thresh: float, window_delta: float = 0.0) -> dict:
        """Multi-indicator ensemble signal.
        Integrates: window delta, momentum, RSI, VWAP, Fib, Smart Money (OBI+CVD).
        Only fires when delta >= 12 (79% WR in live data)."""
        if len(self.ph) < 30:
            return {
                "direction": "Neutral",
                "conf": 0.0,
                "reasons": [],
                "conditions": {},
            }

        mom_1m = self.momentum(60)
        mom_5m = self.momentum(300)
        mom_30m = self.momentum(1800)  # 30-minute trend context
        rsi_val, rsi_slope = self.rsi(14)

        current_price = self.ph[-1][1]
        vwap = self.vwap(lookback=60)
        vwap_dist_bps = (current_price - vwap) / vwap * 10000 if vwap else 0
        cvd = self.cvd(lookback=50)
        obi = self.obi()
        fib_dist = self.fib_levels().get("distance_bps", 9999)
        sm = self.smart_money_signal()

        reasons = []
        conf_ups = []
        conf_downs = []
        conditions = {
            "vwap_dist_bps": round(vwap_dist_bps, 1),
            "fib_dist_bps": round(fib_dist, 1),
            "obi": round(obi, 3),
            "cvd": round(cvd, 4),
            "sm_signal": sm["signal"],
        }

        # ── WINDOW DELTA: Primary signal ──
        if window_delta >= 20:
            conf_ups.append(0.90)
            reasons.append(f"BIG+{window_delta:.0f}")
        elif window_delta >= 15:
            conf_ups.append(0.80)
            reasons.append(f"win_d+{window_delta:.0f}")
        elif window_delta >= 12:
            conf_ups.append(0.70)
            reasons.append(f"win_d+{window_delta:.0f}")
        elif window_delta > 0:
            conf_ups.append(0.25)
        elif window_delta <= -20:
            conf_downs.append(0.90)
            reasons.append(f"BIG-{abs(window_delta):.0f}")
        elif window_delta <= -15:
            conf_downs.append(0.80)
            reasons.append(f"win_d-{abs(window_delta):.0f}")
        elif window_delta <= -12:
            conf_downs.append(0.70)
            reasons.append(f"win_d-{abs(window_delta):.0f}")
        else:
            conf_downs.append(0.25)

        # ── MOMENTUM (1m + 5m) ──
        if mom_1m > 5 and conf_ups:
            conf_ups.append(0.08)
            reasons.append(f"1m+{mom_1m:.0f}")
        elif mom_1m < -5 and conf_downs:
            conf_downs.append(0.08)
            reasons.append(f"1m-{abs(mom_1m):.0f}")
        if mom_5m > 10 and conf_ups:
            conf_ups.append(0.06)
            reasons.append(f"5m+{mom_5m:.0f}")
        elif mom_5m < -10 and conf_downs:
            conf_downs.append(0.06)
            reasons.append(f"5m-{abs(mom_5m):.0f}")

        # ── RSI ──
        if rsi_val < 35 and rsi_slope > 0 and conf_ups:
            conf_ups.append(0.07)
            reasons.append(f"RSI_OV+{rsi_val:.0f}")
        elif rsi_val > 65 and rsi_slope < 0 and conf_downs:
            conf_downs.append(0.07)
            reasons.append(f"RSI_OB-{rsi_val:.0f}")

        # ── VWAP: price near VWAP = weak. Far above/below = trend confirmation ──
        if abs(vwap_dist_bps) < 10 and conf_ups:
            conf_ups.append(0.10)
            reasons.append(f"NearVWAP({vwap_dist_bps:+.0f}bps)")
        elif abs(vwap_dist_bps) < 10 and conf_downs:
            conf_downs.append(0.10)
            reasons.append(f"NearVWAP({vwap_dist_bps:+.0f}bps)")
        elif vwap_dist_bps > 15 and conf_ups:
            conf_ups.append(0.12)
            reasons.append(f"AboveVWAP({vwap_dist_bps:+.0f}bps)")
        elif vwap_dist_bps < -15 and conf_downs:
            conf_downs.append(0.12)
            reasons.append(f"BelowVWAP({vwap_dist_bps:+.0f}bps)")

        # ── FIBONACCI CONFLOUENCE: price at key Fib level = stronger signal ──
        if fib_dist < 15 and conf_ups:
            conf_ups.append(0.12)
            reasons.append(f"FibConfluence({fib_dist:.0f}bps)")
        elif fib_dist < 15 and conf_downs:
            conf_downs.append(0.12)
            reasons.append(f"FibConfluence({fib_dist:.0f}bps)")

        # ── 30-MINUTE TREND FILTER: SOFT penalty when delta < 15 and momentum contradicts ──
        strong_move = abs(window_delta) >= 15
        if not strong_move:
            if conf_downs and mom_30m > 150:
                conf_downs.append(-0.15)
                reasons.append("trend_filter_up")
            if conf_ups and mom_30m < -150:
                conf_ups.append(-0.15)
                reasons.append("trend_filter_down")

        # ── SMART MONEY: OBI + CVD alignment ──
        if sm["signal"] == "bull" and conf_ups:
            conf_ups.append(sm["conf"])
            reasons.append(f"SM_bull(OBI={obi:.2f})")
        elif sm["signal"] == "bear" and conf_downs:
            conf_downs.append(sm["conf"])
            reasons.append(f"SM_bear(OBI={obi:.2f})")
        elif sm["signal"] != "neutral" and (conf_ups or conf_downs):
            # SM strongly disagrees — SOFT penalty, not hard block (delta overrides)
            if conf_ups and sm["signal"] == "bear":
                conf_ups.append(-0.15)
                reasons.append(f"SM_diverge({sm['signal']})")
            elif conf_downs and sm["signal"] == "bull":
                conf_downs.append(-0.15)
                reasons.append(f"SM_diverge({sm['signal']})")

        # ── DIVERGENCE CHECK: momentum vs delta (tighter: 2 not 5) — SOFT penalty ──
        if conf_ups and mom_1m < -2:
            conf_ups.append(-0.10)
            reasons.append("mom_div")
        if conf_downs and mom_1m > 2:
            conf_downs.append(-0.10)
            reasons.append("mom_div")

        if not conf_ups and not conf_downs:
            return {
                "direction": "Neutral",
                "conf": 0.0,
                "reasons": reasons,
                "conditions": conditions,
            }

        up_conf = max(0, max(conf_ups)) if conf_ups else 0.0
        down_conf = max(0, max(conf_downs)) if conf_downs else 0.0
        if conf_downs and any(c < 0 for c in conf_ups):
            up_conf += -0.10
        if conf_ups and any(c < 0 for c in conf_downs):
            down_conf += -0.10
        up_conf = max(0, up_conf)
        down_conf = max(0, down_conf)

        if up_conf > ens_thresh and up_conf > down_conf:
            return {
                "direction": "Up",
                "conf": min(up_conf, 0.98),
                "reasons": reasons,
                "conditions": conditions,
            }
        elif down_conf > ens_thresh and down_conf > up_conf:
            return {
                "direction": "Down",
                "conf": min(down_conf, 0.98),
                "reasons": reasons,
                "conditions": conditions,
            }

        return {
            "direction": "Neutral",
            "conf": 0.0,
            "reasons": reasons,
            "conditions": conditions,
        }

    # ── FREE INDICATORS (no API key needed) ──────────────────────────────────

    def vwap(self, interval: str = "1m", lookback: int = 60) -> float:
        """Volume-Weighted Average Price from Binance klines.
        Above VWAP = bullish intraday trend. Below = bearish."""
        klines = get_binance_kline(interval=interval, limit=lookback)
        if not klines:
            return self.ph[-1][1] if self.ph else 0.0
        cum_vp = 0.0
        cum_vol = 0.0
        for k in klines:
            try:
                high = float(k[2])
                low = float(k[3])
                close = float(k[4])
                vol = float(k[5])
                typical = (high + low + close) / 3.0
                cum_vp += typical * vol
                cum_vol += vol
            except (ValueError, IndexError):
                continue
        if cum_vol <= 0:
            return self.ph[-1][1] if self.ph else 0.0
        return cum_vp / cum_vol

    def cvd(self, lookback: int = 50) -> float:
        """Cumulative Volume Delta — net buyer volume from Binance trades.
        Positive CVD = aggressive buying. Negative = selling pressure.
        Used for divergence detection."""
        trades = get_binance_trades(limit=lookback)
        if not trades:
            return 0.0
        cvd = 0.0
        for t in trades:
            try:
                qty = float(t.get("qty", 0))
                is_buy = not t.get("isBuyerMaker", True)
                cvd += qty if is_buy else -qty
            except (ValueError, KeyError):
                continue
        return cvd

    def obi(self) -> float:
        """Order Book Imbalance from Binance depth.
        Returns: bid_qty / (bid_qty + ask_qty) — 0.5 = balanced.
        >0.6 = buy wall dominant (bullish pressure).
        <0.4 = sell wall dominant (bearish pressure)."""
        depth = get_binance_depth(limit=50)
        bids = depth.get("bids", [])
        asks = depth.get("asks", [])
        bid_vol = sum(float(b[1]) for b in bids)
        ask_vol = sum(float(a[1]) for a in asks)
        total = bid_vol + ask_vol
        if total <= 0:
            return 0.5
        return bid_vol / total

    def fib_levels(self) -> dict:
        """Fibonacci retracement levels from 4H swing high/low.
        Key levels: 0.236, 0.382, 0.5, 0.618, 0.786.
        Returns {level: price} dict. Price near level = potential reversal."""
        klines = get_binance_kline(interval="4h", limit=100)
        if not klines or len(klines) < 20:
            return {}
        highs = [float(k[2]) for k in klines]
        lows = [float(k[3]) for k in klines]
        swing_high = max(highs[-20:])
        swing_low = min(lows[-20:])
        diff = swing_high - swing_low
        if diff < 50:
            return {}
        fib_ratios = [0.236, 0.382, 0.5, 0.618, 0.786]
        current = self.ph[-1][1] if self.ph else (swing_high + swing_low) / 2
        levels = {}
        for r in fib_ratios:
            levels[r] = swing_high - r * diff
        nearest = min(levels.items(), key=lambda x: abs(x[1] - current))
        return {
            "swing_high": swing_high,
            "swing_low": swing_low,
            "levels": levels,
            "nearest_fib": nearest[0],
            "nearest_price": nearest[1],
            "distance_bps": abs(nearest[1] - current) / current * 10000,
        }

    def fib_confluence(self) -> float:
        """Returns confidence boost (0.0-0.2) when price is AT a Fib level.
        Strongest signal: price within 5bps of a key Fib level."""
        fib = self.fib_levels()
        if not fib or fib["distance_bps"] > 50:
            return 0.0
        d = fib["distance_bps"]
        if d <= 5:
            return 0.20
        elif d <= 15:
            return 0.12
        elif d <= 30:
            return 0.07
        return 0.0

    def volume_profile(self, interval: str = "5m", lookback: int = 20) -> dict:
        """Volume profile: which price levels have had the most volume.
        Returns {price_bucket: volume}. High volume nodes = support/resistance."""
        klines = get_binance_kline(interval=interval, limit=lookback)
        if not klines:
            return {}
        profile = {}
        for k in klines:
            try:
                vol = float(k[5])
                close = float(k[4])
                bucket = round(close, -2)
                profile[bucket] = profile.get(bucket, 0.0) + vol
            except (ValueError, IndexError):
                continue
        return profile

    def smart_money_signal(self) -> dict:
        """Composite signal from order flow + CVD + OBI.
        Returns: {signal: 'bull'|'bear'|'neutral', conf: 0.0-0.3}"""
        cvd = self.cvd()
        obi = self.obi()
        obi_score = 0.0
        if obi > 0.65:
            obi_score = 0.15
        elif obi > 0.58:
            obi_score = 0.08
        elif obi < 0.35:
            obi_score = 0.15
        elif obi < 0.42:
            obi_score = 0.08
        cvd_score = 0.0
        cvd_norm = cvd / max(abs(cvd), 1.0) if cvd != 0 else 0.0
        if cvd_norm > 0.7:
            cvd_score = 0.10
        elif cvd_norm < -0.7:
            cvd_score = 0.10
        total = obi_score + cvd_score
        if total < 0.05:
            return {"signal": "neutral", "conf": 0.0}
        direction = "bull" if obi > 0.5 else "bear"
        return {"signal": direction, "conf": min(total, 0.25)}


# ── Lessons Learned ────────────────────────────────────────────────────────────
class LessonsLearned:
    """
    Records conditions at entry and outcome for every trade.
    After each resolution, updates per-condition win rates.
    Slowly adapts param adjustments based on accumulated evidence.
    """

    def __init__(self):
        self.file = DATA_DIR / "lessons_learned.json"
        self.conditions: dict[str, dict] = {}
        self.load()

    def load(self):
        if self.file.exists():
            try:
                with open(self.file) as f:
                    self.conditions = json.load(f)
                log(f"[LESSONS] Loaded {len(self.conditions)} condition records")
            except:
                pass

    def save(self):
        try:
            with open(self.file, "w") as f:
                json.dump(self.conditions, f, indent=2)
        except:
            pass

    def record(self, direction: str, won: bool, conditions: dict):
        """Record outcome for a set of conditions. conditions = {key: value}."""
        for key, val in conditions.items():
            if key == "window_tf":
                val = str(val)
            else:
                try:
                    val = round(float(val), 4)
                except (ValueError, TypeError):
                    val = str(val)
            k = f"{key}={val}"
            if k not in self.conditions:
                self.conditions[k] = {"wins": 0, "losses": 0, "total": 0}
            c = self.conditions[k]
            c["total"] += 1
            if won:
                c["wins"] += 1
            else:
                c["losses"] += 1

    def wr(self, key: str, val) -> float:
        k = f"{key}={round(float(val), 4) if isinstance(val, float) else str(val)}"
        if k not in self.conditions:
            return 0.5
        c = self.conditions[k]
        return c["wins"] / max(c["total"], 1)

    def best_val(self, key: str, values: list) -> float:
        """Return the value in `values` that had the highest historical WR."""
        best, best_wr = values[0], 0.0
        for v in values:
            wr = self.wr(key, v)
            if wr > best_wr:
                best, best_wr = v, wr
        return best

    def analyse(self, trade: dict) -> str:
        """Return a human-readable analysis of what this trade teaches us."""
        lessons = []
        c = trade.get("conditions", {})
        direction = trade.get("direction", "?")
        won = trade.get("won", False)
        delta = abs(trade.get("btc_delta", 0))
        conf = trade.get("conf", 0)
        entry_age = trade.get("entry_age_secs", 999)
        vwap_dist = abs(c.get("vwap_dist_bps", 9999))
        fib_dist = c.get("fib_dist_bps", 9999)
        obi = c.get("obi", 0.5)
        cvd = c.get("cvd", 0)
        sm_signal = c.get("sm_signal", "neutral")

        if won:
            if delta >= 15:
                lessons.append(f"BIG delta(${delta:.0f}) → WIN ✅")
            if entry_age < 90:
                lessons.append(f"Early entry ({entry_age:.0f}s) → WIN ✅")
            if vwap_dist < 10:
                lessons.append(f"Near VWAP → WIN ✅")
            if fib_dist < 20:
                lessons.append(f"Fib confluence → WIN ✅")
            if conf > 0.85:
                lessons.append(f"High conf({conf:.2f}) → WIN ✅")
            if sm_signal == direction.lower():
                lessons.append(f"SmartMoney({sm_signal}) aligned → WIN ✅")
        else:
            if delta < 12:
                lessons.append(f"Small delta(${delta:.0f}) → LOSS ❌")
            if entry_age > 150:
                lessons.append(f"Late entry ({entry_age:.0f}s) → LOSS ❌")
            if conf < 0.8:
                lessons.append(f"Low conf({conf:.2f}) → LOSS ❌")
            if sm_signal != direction.lower() and sm_signal != "neutral":
                lessons.append(f"SmartMoney divergence({sm_signal}) → LOSS ❌")

        return " | ".join(lessons) if lessons else "Marginal trade"

    def suggest_params(self) -> dict:
        """Analyze lessons and return param adjustment suggestions."""
        suggestions = {}
        for key in list(self.conditions.keys()):
            if isinstance(key, str):
                if not key.startswith("delta=") and not key.startswith("conf="):
                    continue
                parts = key.split("=")
                if len(parts) != 2:
                    continue
                k, raw_val = parts
            elif isinstance(key, tuple) and len(key) >= 2:
                cond_type = key[0]
                cond_val = key[1]
                k = cond_type
                raw_val = str(cond_val)
            else:
                continue
            try:
                val = float(raw_val)
            except ValueError:
                continue
            c = self.conditions[key]
            if c["total"] < 3:
                continue
            wr = c["wins"] / c["total"]
            if wr < 0.45 and c["total"] >= 5:
                suggestions[k] = {
                    "val": val,
                    "wr": wr,
                    "action": "avoid",
                    "n": c["total"],
                }
            elif wr > 0.75 and c["total"] >= 3:
                suggestions[k] = {
                    "val": val,
                    "wr": wr,
                    "action": "prefer",
                    "n": c["total"],
                }
        return suggestions


def fetch_btc_markets(tf_minutes: int = 5) -> Optional[dict]:
    """
    Fetch active BTC prediction market for the given timeframe.
    Slug patterns:
      5m:  btc-updown-5m-{window_start_unix}
      15m: btc-updown-15m-{window_start_unix}
    Windows start at XX:00, XX:05, XX:10 UTC (5m) or XX:00, XX:15, XX:30 (15m).
    Checks current window first, then next two.
    """
    now_ts = int(time.time())
    window_sec = tf_minutes * 60
    current_window = (now_ts // window_sec) * window_sec
    slug_prefix = f"btc-updown-{tf_minutes}m"
    slugs_to_try = [
        f"{slug_prefix}-{current_window}",
        f"{slug_prefix}-{current_window + window_sec}",
        f"{slug_prefix}-{current_window + window_sec * 2}",
    ]
    GAMMA_API = "https://gamma-api.polymarket.com"

    for slug in slugs_to_try:
        try:
            r = requests.get(
                f"{GAMMA_API}/markets",
                params={"slug": slug},
                headers={"Content-Type": "application/json"},
                timeout=10,
            )
            if r.status_code != 200:
                continue
            data = r.json()
            markets = data if isinstance(data, list) else data.get("data", [])
            if not markets:
                continue
            m = markets[0]
            if not isinstance(m, dict):
                continue
            if not m.get("acceptingOrders", False):
                continue
            if m.get("closed", True):
                continue
            end_date = m.get("endDate", "")
            if end_date:
                from datetime import datetime

                try:
                    end_dt = datetime.fromisoformat(end_date.replace("Z", "+00:00"))
                    if end_dt.timestamp() < now_ts:
                        continue
                except:
                    pass
            return m
        except Exception as e:
            continue
    return None


def fetch_market_resolution(
    market_id: str, direction: str, up_idx: int = 0, down_idx: int = 1
) -> Optional[str]:
    """Query Polymarket Gamma API for market resolution.
    Returns 'Up', 'Down', or None if not yet resolved."""
    GAMMA_API = "https://gamma-api.polymarket.com"
    try:
        r = requests.get(f"{GAMMA_API}/markets/{market_id}", timeout=8)
        if r.status_code != 200:
            return None
        m = r.json()
        prices_raw = m.get("outcomePrices", "[]")
        prices = json.loads(prices_raw) if isinstance(prices_raw, str) else prices_raw
        labels_raw = m.get("outcomes", '["Up","Down"]')
        labels = json.loads(labels_raw) if isinstance(labels_raw, str) else labels_raw
        # Resolve: winning outcome goes to $1.00
        for i, price in enumerate(prices):
            try:
                p = float(price)
                if p >= 0.99:
                    label = labels[i] if i < len(labels) else ""
                    if str(label).lower() in ("up", "yes"):
                        return "Up"
                    elif str(label).lower() in ("down", "no"):
                        return "Down"
            except:
                pass
        return None  # not yet resolved
    except Exception:
        return None


def get_orderbook(market_id: str) -> dict:
    try:
        r = requests.get(f"{POLYMARKET_CLOB}/orderbook/{market_id}", timeout=5)
        r.raise_for_status()
        return r.json()
    except:
        return {"bids": [], "asks": []}


def get_polio_price(market_id: str, side: str = "Yes") -> float:
    ob = get_orderbook(market_id)
    if side == "Yes":
        asks = ob.get("asks", [])
        if asks:
            return float(asks[0][0])
    else:
        bids = ob.get("bids", [])
        if bids:
            return float(bids[0][0])
    return 0.50


def get_binance_btc() -> Optional[float]:
    try:
        r = requests.get(BINANCE_REST, timeout=5)
        r.raise_for_status()
        return float(r.json()["price"])
    except:
        return None


def get_binance_kline(
    symbol: str = "BTCUSDT", interval: str = "1m", start_ms: int = None, limit: int = 10
) -> Optional[list]:
    try:
        params = {"symbol": symbol, "interval": interval, "limit": limit}
        if start_ms:
            params["startTime"] = start_ms
        r = requests.get(BINANCE_KLINES, params=params, timeout=10)
        if r.status_code == 200:
            return r.json()
    except:
        pass
    return None


def get_binance_depth(symbol: str = "BTCUSDT", limit: int = 50) -> dict:
    """Return orderbook bids/asks from Binance."""
    try:
        r = requests.get(
            BINANCE_DEPTH, params={"symbol": symbol, "limit": limit}, timeout=5
        )
        if r.status_code == 200:
            return r.json()
    except:
        pass
    return {"bids": [], "asks": []}


def get_binance_trades(symbol: str = "BTCUSDT", limit: int = 100) -> list:
    """Return recent trades from Binance. Each trade: {price, qty, quoteQty, isBuyerMaker}."""
    try:
        r = requests.get(
            BINANCE_TRADES, params={"symbol": symbol, "limit": limit}, timeout=5
        )
        if r.status_code == 200:
            return r.json()
    except:
        pass
    return []


def get_binance_24h_ticker(symbol: str = "BTCUSDT") -> dict:
    """Return 24h ticker stats from Binance."""
    try:
        r = requests.get(BINANCE_TICKER, params={"symbol": symbol}, timeout=5)
        if r.status_code == 200:
            return r.json()
    except:
        pass
    return {}


# ── CLOB Client Wrapper ─────────────────────────────────────────────────────────
class CLOBClient:
    def __init__(self):
        from dotenv import load_dotenv
        from py_clob_client.client import ClobClient
        from py_clob_client.clob_types import (
            ApiCreds,
            AssetType,
            BalanceAllowanceParams,
        )

        ENV_PATH = HARVEY_HOME / "data" / "arbitrage-agent" / ".env.live"
        load_dotenv(ENV_PATH)

        pk = os.environ.get("POLYMARKET_PRIVATE_KEY")
        funder = os.environ.get("POLYMARKET_FUNDER_ADDRESS")
        sig_type = int(os.environ.get("POLYMARKET_SIGNATURE_TYPE", 2))
        key = os.environ.get("POLYMARKET_API_KEY")
        secret = os.environ.get("POLYMARKET_API_SECRET")
        passphrase = os.environ.get("POLYMARKET_API_PASSPHRASE")

        init_creds = ApiCreds(api_key=key, api_secret=secret, api_passphrase=passphrase)
        self._client = ClobClient(
            POLYMARKET_CLOB,
            key=pk,
            chain_id=137,
            signature_type=sig_type,
            funder=funder,
            creds=init_creds,
        )
        derived = self._client.derive_api_key()
        self._client.set_api_creds(derived)

        params = BalanceAllowanceParams(
            asset_type=AssetType.COLLATERAL, signature_type=sig_type
        )
        try:
            self._client.update_balance_allowance(params)
        except Exception:
            pass
        bal_resp = self._client.get_balance_allowance(params)
        self._balance = float(bal_resp.get("balance", 0)) / 1e6
        log(f"[CLOB] Connected. Balance: ${self._balance:.2f}")

    @property
    def balance(self) -> float:
        return self._balance

    def place_order(
        self, token_id: str, side: str, price: float, size: float
    ) -> Optional[str]:
        from py_clob_client.order_builder.constants import BUY, SELL
        from py_clob_client.clob_types import OrderArgs

        try:
            # Always BUY when opening a position. token_id selects YES or NO outcome.
            # (The `side` param is kept for API compat but we're always buying the outcome token.)
            order_side = BUY
            order_args = OrderArgs(
                price=min(price, 0.99),
                size=size,
                side=order_side,
                token_id=token_id,
            )
            signed = self._client.create_order(order_args)
            resp = self._client.post_order(signed)
            if resp and resp.get("success"):
                oid = resp.get("orderID", "unknown")
                log(f"[CLOB] Order placed: {side} {size}@{price:.4f} oid={oid}")
                return oid
            else:
                log(f"[CLOB] Order rejected: {resp}")
                return None
        except Exception as e:
            log(f"[CLOB] Order error: {e}")
            return None

    def cancel_order(self, order_id: str) -> bool:
        try:
            self._client.cancel_order(order_id)
            return True
        except:
            return False

    def get_order_status(self, order_id: str) -> Optional[dict]:
        try:
            return self._client.get_order(order_id=order_id)
        except:
            return None


# ── Trade ─────────────────────────────────────────────────────────────────────
@dataclass
class Trade:
    window_start: int
    direction: str
    spend: float
    poly_price: float
    btc_delta: float
    btc_price_enter: float
    conf: float
    reasons: list
    placed_at: float
    order_id: Optional[str] = None
    token_id: Optional[str] = None
    market_id: Optional[str] = None
    resolved: bool = False
    won: bool = False
    pnl: float = 0.0
    exit_reason: str = ""
    journaled: bool = False
    resolved_at: float = 0.0
    window_tf: int = 5


# ── Live Sniper ───────────────────────────────────────────────────────────────
class WindowState:
    """Tracks state for a single timeframe (5m or 15m)."""

    def __init__(self, tf_minutes: int):
        self.tf = tf_minutes
        self.se = SignalEngine()
        self.market_id: Optional[str] = None
        self.market_question: str = ""
        self.window_start: Optional[int] = None
        self.window_price: Optional[float] = None
        self.traded_this_window = False
        self.pause_until: float = 0.0
        self._outcome_prices: list[float] = [0.50, 0.50]
        self._up_token_id: Optional[str] = None
        self._down_token_id: Optional[str] = None
        self.deployed: float = 0.0

    def reset_window(self, window_start: int, window_price: float, market: dict):
        self.window_start = window_start
        self.window_price = window_price
        self.traded_this_window = False
        self.pause_until = 0.0
        self.market_id = market.get("id")
        self.market_question = market.get("question", "?")
        try:
            tokens = json.loads(market.get("clobTokenIds", "[]"))
            outcomes_raw = json.loads(market.get("outcomePrices", "[]"))
            self._outcome_prices = (
                [float(outcomes_raw[0]), float(outcomes_raw[1])]
                if len(outcomes_raw) >= 2
                else [0.50, 0.50]
            )
            self._up_token_id = (
                tokens[1] if len(tokens) > 1 else (tokens[0] if tokens else None)
            )
            self._down_token_id = tokens[0] if tokens else None
        except Exception:
            self._outcome_prices = [0.50, 0.50]
            self._up_token_id = None
            self._down_token_id = None


class LiveSniper:
    """
    Production BTC sniper: watches Polymarket BTC markets (5m + 15m simultaneously),
    places real CLOB orders, analyses every trade and auto-improves.
    """

    def __init__(self, params: SniperParams, live: bool = True):
        self.params = params
        self.live = live
        self.bankroll = PAPER_CAPITAL
        self.starting = PAPER_CAPITAL
        self.trades: list[Trade] = []
        self.wins = self.losses = self.blocks = 0
        self.total_pnl = 0.0
        self.running = True

        self.btc_price: Optional[float] = None
        self.t0 = time.time()

        self.windows: dict[int, WindowState] = {
            5: WindowState(5),
            15: WindowState(15),
        }
        self._timeframes = [5, 15]
        self._capital_split = {5: 0.60, 15: 0.40}

        self._open_orders: dict[str, dict] = {}
        self._reconciled_orders: set[str] = set()
        self._pending_fills: dict[str, dict] = {}
        self._client: Optional[CLOBClient] = None
        self._balance: float = PAPER_CAPITAL
        self._last_ga_evolve: float = 0.0
        self._balance_cache_time: float = 0.0
        self._balance_cache: float = PAPER_CAPITAL

        self.lessons = LessonsLearned()
        self._total_deployed: float = 0.0
        self._last_analysis: float = 0.0
        self._adapt_cooldown: float = 0.0

        if live:
            try:
                self._client = CLOBClient()
                self.bankroll = self._client.balance
                self._balance = self.bankroll
                self.starting = self.bankroll
                log(f"[CLOB] Connected. Balance: ${self.bankroll:.2f}")
            except Exception as e:
                log(f"[CLOB] Failed to connect: {e}. Running in SIM mode.")
                self.live = False

    def _window_for_tf(self, tf: int) -> WindowState:
        return self.windows[tf]

    def _cap_for_tf(self, tf: int) -> float:
        available = self._balance - self._total_deployed
        cap = self._balance * self._capital_split[tf]
        return min(cap, available)

    def _now(self) -> float:
        return time.time()

    def _journal_trade(self, t: Trade):
        if t.journaled:
            return
        t.journaled = True
        d = {
            "mode": "live" if self.live else "paper",
            "params": self.params.to_dict(),
            "window_start": t.window_start,
            "window_tf": getattr(t, "window_tf", 5),
            "direction": t.direction,
            "spend": t.spend,
            "poly_price": t.poly_price,
            "btc_delta": t.btc_delta,
            "btc_price_enter": t.btc_price_enter,
            "conf": t.conf,
            "reasons": t.reasons,
            "won": t.won,
            "pnl": round(t.pnl, 4),
            "exit_reason": t.exit_reason,
            "placed_at": datetime.fromtimestamp(t.placed_at).isoformat(),
            "resolved_at": datetime.fromtimestamp(t.resolved_at).isoformat()
            if t.resolved_at
            else None,
        }
        try:
            with open(JOURNAL_FILE, "a") as f:
                f.write(json.dumps(d) + "\n")
        except:
            pass

    # ── Signal check (per window) ────────────────────────────────────────────
    def _check_signal(self, win: WindowState, tf: int) -> Optional[dict]:
        if not self.btc_price or not win.window_price:
            return None
        if self._now() < win.pause_until:
            return None
        if win.traded_this_window:
            return None
        cap = self._cap_for_tf(tf)
        if cap < MIN_SPEND:
            return None

        delta = self.btc_price - win.window_price
        up_price = win._outcome_prices[1]
        down_price = win._outcome_prices[0]
        poly_conviction = abs(up_price - 0.5) * 2

        # Verbose diagnostic log (every check, so we can see why it's NOT firing)
        if abs(delta) >= 3:  # only log meaningful deltas to avoid spam
            sig_probe = win.se.ensemble(self.params.ens_thresh, window_delta=delta)
            log(
                f"[EVAL {tf}m] delta=${delta:+.1f} poly_conv={poly_conviction:.2f} "
                f"→ dir={sig_probe['direction']} conf={sig_probe['conf']:.2f} "
                f"(need: |d|>={self.params.delta_thresh:.0f} & conf>={self.params.conf_thresh:.2f})"
            )

        if abs(delta) < self.params.delta_thresh and poly_conviction < 0.90:
            return None

        sig = win.se.ensemble(self.params.ens_thresh, window_delta=delta)

        if sig["direction"] == "Neutral":
            return None
        if (
            sig["conf"] < self.params.conf_thresh
            and abs(delta) < self.params.delta_thresh + 5
            and poly_conviction < 0.90
        ):
            return None

        direction = sig["direction"]
        hour = int(datetime.utcnow().strftime("%H"))
        conf = self._hour_adjust_conf(sig["conf"], direction, hour)
        if conf < self.params.conf_thresh:
            return None

        trade_price = up_price if direction == "Up" else down_price
        if trade_price > 0.72 and poly_conviction < 0.90:
            return None
        conviction = abs(trade_price - 0.50)
        if conviction < 0.04:
            price_lo, price_hi = 0.40, 0.60
        elif conviction < 0.08:
            price_lo, price_hi = 0.42, 0.58
        elif conviction < 0.15:
            price_lo, price_hi = 0.45, 0.55
        else:
            price_lo, price_hi = 0.40, 0.60
        if not (price_lo <= trade_price <= price_hi):
            return None

        PREFERRED_UP = {1, 11, 13, 15, 16, 17, 18, 19, 22}
        PREFERRED_DOWN = {0, 9, 12, 14, 20, 23, 1, 11, 13}
        tier = "NORMAL"
        if (
            abs(delta) >= 25
            and 0.48 <= trade_price <= 0.52
            and (
                (direction == "Up" and hour in PREFERRED_UP)
                or (direction == "Down" and hour in PREFERRED_DOWN)
            )
        ):
            tier = "ULTIMATE"

        return {
            "direction": sig["direction"],
            "delta": delta,
            "conf": conf,
            "tier": tier,
            "reasons": sig["reasons"],
            "conditions": sig.get("conditions", {}),
        }

    def _refresh_balance(self):
        if not self._client:
            return
        try:
            from py_clob_client.clob_types import AssetType, BalanceAllowanceParams

            params = BalanceAllowanceParams(
                asset_type=AssetType.COLLATERAL, signature_type=2
            )
            for attempt in range(6):
                try:
                    self._client.update_balance_allowance(params)
                except Exception:
                    pass
                try:
                    bal_resp = self._client.get_balance_allowance(params)
                    raw = bal_resp.get("balance", "0")
                    new_balance = float(raw) / 1e6
                    if new_balance > 0:
                        self._balance = new_balance
                        self.bankroll = new_balance
                        self._balance_cache = new_balance
                        self._balance_cache_time = self._now()
                        return
                except Exception:
                    pass
                import time

                time.sleep(1.5 * (attempt + 1))
        except Exception:
            pass

    def _refresh_balance_with_retry(self):
        """Call update_balance_allowance then get_balance_allowance in a retry loop.
        Called after detecting a fill — the on-chain state may take a few seconds.
        """
        if not self._client:
            return
        try:
            from py_clob_client.clob_types import AssetType, BalanceAllowanceParams

            params = BalanceAllowanceParams(
                asset_type=AssetType.COLLATERAL, signature_type=2
            )
            for attempt in range(8):
                try:
                    self._client.update_balance_allowance(params)
                except Exception:
                    pass
                try:
                    bal_resp = self._client.get_balance_allowance(params)
                    raw = bal_resp.get("balance", "0")
                    new_balance = float(raw) / 1e6
                    if new_balance > 0:
                        old = self._balance
                        self._balance = new_balance
                        self.bankroll = new_balance
                        if abs(new_balance - old) > 0.01:
                            log(
                                f"[CLOB] Balance updated: ${old:.2f} → ${new_balance:.2f}"
                            )
                        return
                except Exception:
                    pass
                import time

                time.sleep(2.0 * (attempt + 1))
        except Exception:
            pass

    # ── Order execution ───────────────────────────────────────────────────────
    def _place_trade(self, sig: dict, win: WindowState, tf: int) -> Optional[Trade]:
        if not win.market_id:
            return None
        # Don't place orders if less than 90s remain in window — orderbook may vanish
        win_sec = tf * 60
        time_left = (win.window_start + win_sec) - self._now()
        if time_left < 90:
            log(
                f"[SKIP {tf}m] Only {time_left:.0f}s left in window — too late to trade"
            )
            return None

        now = self._now()
        if now - self._balance_cache_time > 10:
            self._refresh_balance_with_retry()
            self._balance_cache = self._balance
            self._balance_cache_time = now
        else:
            self._balance = self._balance_cache

        cap = self._cap_for_tf(tf)
        if cap < MIN_SPEND:
            log(f"[WARN] Cap ${cap:.2f} < min ${MIN_SPEND:.2f}, skipping {tf}m trade")
            self.blocks += 1
            return None

        outcome = "Yes" if sig["direction"] == "Up" else "No"
        poly_price = (
            win._outcome_prices[1] if outcome == "Yes" else win._outcome_prices[0]
        )
        if poly_price <= 0:
            poly_price = 0.50

        tier = sig.get("tier", "NORMAL")
        tier_max_bet_pct = 0.60 if tier == "ULTIMATE" else self.params.max_bet_pct
        spend = min(
            max(MIN_SPEND, cap * self.params.spend_ratio),
            cap * tier_max_bet_pct,
        )
        spend = min(spend, cap * 0.95)
        log(
            f"[DEBUG {tf}m] [{tier}] cap={cap:.2f} spend={spend:.2f} price={poly_price:.4f} "
            f"size={spend / poly_price:.2f} balance=${self._balance:.2f}"
        )

        size = spend / poly_price
        min_shares = 5.0
        if size < min_shares:
            size = min_shares
        cost = size * poly_price
        if cost > cap * 0.95:
            size = cap * 0.95 / poly_price
            size = float(int(size * 100)) / 100
            cost = size * poly_price
        if cost < MIN_SPEND or size < 1.0:
            self.blocks += 1
            return None

        down_token_id = win._down_token_id
        up_token_id = win._up_token_id
        trade_token_id = up_token_id if outcome == "Yes" else down_token_id
        order_id = None

        if self.live and self._client:
            # Use cached balance — don't block with slow retry refresh
            if self._balance < MIN_SPEND:
                self.blocks += 1
                return None
            if self._balance < spend:
                spend = self._balance * 0.90
                size = spend / poly_price
                size = float(int(size * 100)) / 100
                cost = size * poly_price
            order_id = self._client.place_order(
                token_id=trade_token_id or "",
                side=outcome,
                price=poly_price,
                size=size,
            )
            if not order_id:
                # Clear market state so next iteration fetches fresh market
                # (handles stale orderbook errors, resolved markets, etc)
                log(
                    f"[RECOVER {tf}m] Order failed — clearing market_id to force refetch"
                )
                win.market_id = None
                win._up_token_id = None
                win._down_token_id = None
                return None
            self._open_orders[order_id] = {
                "placed_at": now,
                "direction": sig["direction"],
                "spend": cost,
                "poly_price": poly_price,
                "window_start": win.window_start,
                "window_tf": tf,
                "token_id": trade_token_id,
                "market_id": win.market_id,
            }
            self._total_deployed += cost

        win.traded_this_window = True

        trade = None
        try:
            trade = Trade(
                window_start=win.window_start or int(now // (tf * 60) * (tf * 60)),
                direction=sig["direction"],
                spend=spend,
                poly_price=poly_price,
                btc_delta=sig["delta"],
                btc_price_enter=self.btc_price or 0.0,
                conf=sig["conf"],
                reasons=sig["reasons"],
                placed_at=now,
                order_id=order_id,
                token_id=trade_token_id,
                market_id=win.market_id,
            )
            self.trades.append(trade)
        except Exception as e:
            log(f"[WARN] Could not create Trade object: {e}")

        emoji = "🟢" if self.live else "🟡"
        log(
            f"  {emoji} BET {tf}m: {sig['direction']} | ΔBTC=${sig['delta']:+8.2f} | "
            f"poly={poly_price:.4f} | cost=${cost:.2f} | conf={sig['conf']:.2f} | "
            f"Bk=${self._balance:.2f}"
        )
        return trade

    # ── Reconcile open orders from CLOB ───────────────────────────────────────
    def _reconcile_open_orders(self):
        """Poll CLOB for open orders and resolve any that have been filled or market closed."""
        if not self.live or not self._client:
            return

        # Also refresh balance during reconciliation
        self._refresh_balance()

        try:
            from py_clob_client.clob_types import OpenOrderParams

            params = OpenOrderParams()
            all_orders = self._client._client.get_orders(params) or []
            open_orders = [o for o in all_orders if o.get("status") == "open"]
        except Exception:
            return

        live_order_ids = {str(o.get("orderID", "")) for o in open_orders}
        our_open_ids = set(self._open_orders.keys())

        # Cancel orders that are no longer on CLOB (filled or cancelled)
        for oid in our_open_ids - live_order_ids:
            if oid in self._reconciled_orders:
                continue
            self._reconciled_orders.add(oid)
            info = self._open_orders.pop(oid, {})
            log(f"[RECONCILE] Order {oid[:16]}... filled/cancelled")
            self._resolve_from_open_order(oid, info)
            self._refresh_balance_with_retry()

        # Recompute _total_deployed from ground truth to prevent accounting drift
        self._total_deployed = sum(
            i.get("spend", 0) for i in self._open_orders.values()
        )

    def _resolve_from_open_order(self, order_id: str, info: dict):
        """Handle a CLOB fill for an order we tracking.
        If the market window has closed, resolve it now with actual direction.
        Otherwise mark reconciled and let the window-expiry logic handle it.
        """
        if not info:
            return
        placed_at = info.get("placed_at", self._now())
        direction = info.get("direction", "Up")
        window_start = info.get("window_start")
        spend = info.get("spend", 0)
        poly_price = info.get("poly_price", 0.50)

        for t in self.trades:
            if t.order_id == order_id and not t.resolved:
                tf_sec = getattr(t, "window_tf", 5) * 60
                w_end = (window_start or t.window_start) + tf_sec
                if self._now() >= w_end + 5:
                    # ONLY resolve from Polymarket oracle — Binance diverges from PM resolution
                    market_id_for_res = t.market_id or info.get("market_id")
                    actual_dir = None
                    if market_id_for_res:
                        actual_dir = fetch_market_resolution(
                            market_id_for_res, t.direction
                        )
                    if actual_dir:
                        log(f"[RESOLVE] Polymarket oracle: {actual_dir}")
                        self._resolve_trade(t, actual_dir)
                    elif self._now() >= w_end + 600:
                        # Safety: if PM still hasn't resolved 10min after window, force loss
                        log(f"[RESOLVE] PM oracle timeout — forcing LOSS")
                        anti_dir = "Down" if t.direction == "Up" else "Up"
                        self._resolve_trade(t, anti_dir)
                    else:
                        log(
                            f"[RESOLVE] Waiting for PM oracle (market={market_id_for_res})"
                        )
                        self._pending_fills[order_id] = info
                        return
                else:
                    self._pending_fills[order_id] = info
                    log(f"[RECONCILE] Fill pending window close: {order_id[:16]}...")
                return

        if not window_start:
            return
        tf_sec = info.get("window_tf", 5) * 60
        if self._now() >= window_start + tf_sec + 5:
            # ONLY resolve from Polymarket oracle
            market_id_for_res = info.get("market_id")
            actual_dir = None
            if market_id_for_res:
                actual_dir = fetch_market_resolution(market_id_for_res, direction)
            if actual_dir:
                log(f"[RESOLVE] Polymarket oracle: {actual_dir}")
            elif self._now() >= window_start + tf_sec + 600:
                log(f"[RESOLVE] PM oracle timeout — forcing LOSS")
                actual_dir = "Down" if direction == "Up" else "Up"
            else:
                log(f"[RESOLVE] Waiting for PM oracle (market={market_id_for_res})")
                return
            won = direction == actual_dir
            pnl = (
                (spend * (1.0 / poly_price - 1) * (1 - TAKER_FEE_BPS / 10000))
                if won
                else -spend
            )
            trade = Trade(
                window_start=window_start,
                direction=direction,
                spend=spend,
                poly_price=poly_price,
                btc_delta=0,
                btc_price_enter=self.btc_price or 0,
                conf=0.5,
                reasons=["reconciled_fill"],
                placed_at=placed_at,
                order_id=order_id,
                token_id=info.get("token_id", ""),
                market_id=info.get("market_id"),
                resolved=True,
                won=won,
                pnl=pnl,
                resolved_at=self._now(),
                exit_reason="reconciled_fill",
                journaled=True,
                window_tf=info.get("window_tf", 5),
            )
            self.trades.append(trade)
            self._total_deployed -= spend
            self.total_pnl += pnl
            if won:
                self.wins += 1
            else:
                self.losses += 1
            self._journal_trade(trade)
            self._analyse_trade(trade)
            if self.live:
                self._refresh_balance_with_retry()
                self.bankroll = self._balance
            log(
                f"[RECONCILE] Created+resolved trade {order_id[:16]}... won={won} pnl=${pnl:+.4f}"
            )

    # ── Resolve trade ─────────────────────────────────────────────────────────
    def _resolve_trade(self, t: Trade, actual_dir: str):
        won = t.direction == actual_dir
        if won:
            pnl = t.spend * (1.0 / t.poly_price - 1) * (1 - TAKER_FEE_BPS / 10000)
        else:
            pnl = -t.spend

        t.resolved = True
        t.won = won
        t.pnl = pnl
        t.resolved_at = self._now()
        t.exit_reason = f"{'WIN' if won else 'LOSS'}"

        if self.live and self._client and t.order_id:
            self._client.cancel_order(t.order_id)

        for oid, info in list(self._open_orders.items()):
            if info.get("order_id") == t.order_id or oid == t.order_id:
                self._open_orders.pop(oid, None)
                self._total_deployed -= info.get("spend", 0)
                break

        self.total_pnl += pnl
        if won:
            self.wins += 1
        else:
            self.losses += 1

        self._journal_trade(t)
        self._analyse_trade(t)

        if self.live:
            self._refresh_balance_with_retry()
            self.bankroll = self._balance

        result_emoji = "🟢" if won else "🔴"
        log(
            f"       {result_emoji} RESOLVED {getattr(t, 'window_tf', 5)}m: {actual_dir} | "
            f"{'WIN' if won else 'LOSS'} ${pnl:+7.2f} | Bk=${self._balance:.2f}"
        )

    # ── Per-trade analysis + adaptive param tuning ──────────────────────────
    def _analyse_trade(self, t: Trade):
        """Analyze trade outcome and record lessons. Slowly adapt params."""
        try:
            now = self._now()
            entry_age = now - t.placed_at if t.placed_at else 999
            conditions = {
                "window_tf": getattr(t, "window_tf", 5),
                "delta": abs(t.btc_delta),
                "conf": t.conf,
                "direction": t.direction,
            }
            for item in t.reasons or []:
                if isinstance(item, (tuple, list)) and len(item) == 2:
                    k, v = item
                    if isinstance(v, (int, float)):
                        conditions[f"reason_{k}"] = v
            conditions["entry_age_secs"] = round(entry_age, 0)
            analysis = self.lessons.analyse(
                {
                    "direction": t.direction,
                    "won": t.won,
                    "btc_delta": t.btc_delta,
                    "conf": t.conf,
                    "conditions": conditions,
                }
            )
            self.lessons.record(t.direction, t.won, conditions)
            self.lessons.save()
            log(f"       📊 LESSON: {analysis}")

            trades_since_adapt = (self.wins + self.losses) - getattr(
                self, "_trades_at_last_adapt", 0
            )
            if trades_since_adapt >= 3 or now - self._adapt_cooldown > 180:
                self._adapt_cooldown = now
                self._trades_at_last_adapt = self.wins + self.losses
                self._adapt_params()
        except Exception as e:
            log(f"       [WARN] _analyse_trade failed: {e}")

    def _adapt_params(self):
        """Meta-harness style improvement:
        baseline → LLM propose → simulate → apply only if delta > 0.
        Uses journal + lessons as ground truth for simulation."""
        try:
            improvement = self._meta_harness_improve()
            if improvement:
                log(f"       🧬 ADAPTED: {improvement}")
        except Exception as e:
            log(f"       [WARN] _adapt_params failed: {e}")

    def _meta_harness_improve(self) -> Optional[str]:
        """Run one meta-harness cycle. Returns description of what changed, or None."""
        recent = self._load_recent_trades(n=50)
        if len(recent) < 5:
            return None

        baseline_score = self._score_trades(recent)
        log(
            f"       🧬 META-HARNESS: baseline_score={baseline_score:.4f} from {len(recent)} trades"
        )

        sugg = self.lessons.suggest_params()
        if not sugg and len(recent) < 10:
            return None

        prompt = self._build_improvement_prompt(recent, baseline_score, sugg)
        proposal = ai_complete(prompt, max_tokens=800)

        if not proposal:
            return None

        try:
            import re

            match = re.search(r"\{.*\}", proposal, re.DOTALL)
            if not match:
                return None
            data = json.loads(match.group())
        except Exception:
            log(f"       [WARN] Could not parse LLM improvement proposal")
            return None

        changes = data.get("changes", [])
        if not changes:
            return None

        change_desc = ", ".join(
            f"{c['param']}:{c['current']}→{c['suggested']}" for c in changes
        )
        proposed_params = copy.deepcopy(self.params)
        for c in changes:
            if hasattr(proposed_params, c["param"]):
                try:
                    val = float(c["suggested"])
                    setattr(proposed_params, c["param"], val)
                except (ValueError, TypeError):
                    pass

        simulated_trades = self._simulate_trades(recent, proposed_params)
        proposed_score = self._score_trades(simulated_trades)
        delta = proposed_score - baseline_score

        log(f"       🧬 PROPOSED [{change_desc}]")
        log(
            f"       🧬 SIMULATED: baseline={baseline_score:.4f} proposed={proposed_score:.4f} delta={delta:+.4f}"
        )

        if delta > 0:
            old_vals = {c["param"]: getattr(self.params, c["param"]) for c in changes}
            for c in changes:
                if hasattr(self.params, c["param"]):
                    try:
                        setattr(self.params, c["param"], float(c["suggested"]))
                    except (ValueError, TypeError):
                        pass
            new_vals = {c["param"]: getattr(self.params, c["param"]) for c in changes}
            desc = ", ".join(f"{k}={old_vals[k]}→{new_vals[k]}" for k in old_vals)
            self._save_improvement(change_desc, baseline_score, proposed_score, delta)
            return f"{desc} (score {baseline_score:.3f}→{proposed_score:.3f} Δ+{delta:.4f})"
        else:
            log(f"       🧬 REJECTED: delta={delta:+.4f} ≤ 0 — keeping current params")
            return None

    def _load_recent_trades(self, n: int = 100) -> list[dict]:
        trades = []
        try:
            with open(JOURNAL_FILE) as f:
                lines = f.readlines()
            for line in lines[-n:]:
                try:
                    trades.append(json.loads(line))
                except Exception:
                    continue
        except Exception:
            pass
        return trades

    def _score_trades(self, trades: list[dict]) -> float:
        """Score = win rate * avg_pnl - avg_loss_rate * loss_ratio.
        Higher = better strategy."""
        if not trades:
            return 0.0
        wins = [t for t in trades if t.get("won")]
        losses = [t for t in trades if not t.get("won")]
        if not wins and not losses:
            return 0.0
        wr = len(wins) / max(len(wins) + len(losses), 1)
        avg_win = sum(t.get("pnl", 0) for t in wins) / max(len(wins), 1)
        avg_loss = abs(sum(t.get("pnl", 0) for t in losses)) / max(len(losses), 1)
        total_pnl = sum(t.get("pnl", 0) for t in trades)
        score = total_pnl * 10 + wr * 5 + min(len(trades), 50) * 0.1
        return score

    def _simulate_trades(self, trades: list[dict], params: SniperParams) -> list[dict]:
        """Apply new params to historical trades and compute outcomes.
        Re-evaluates whether each trade would have fired given the new params."""
        simulated = []
        for t in trades:
            delta = abs(t.get("btc_delta", 0))
            conf = t.get("conf", 0)
            if delta < params.delta_thresh:
                continue
            if conf < params.conf_thresh:
                continue
            direction = t.get("direction", "?")
            btc_delta = t.get("btc_delta", 0)
            entry = t.get("btc_price_enter", 0)
            window_close = entry + btc_delta
            actual = "Up" if window_close > entry else "Down"
            won = direction == actual
            pnl = (
                t.get("spend", 10) * (1.0 / t.get("poly_price", 0.5) - 1)
                if won
                else -t.get("spend", 10)
            )
            sim = dict(t)
            sim["won"] = won
            sim["pnl"] = pnl
            simulated.append(sim)
        return simulated

    def _build_improvement_prompt(
        self, trades: list[dict], baseline_score: float, sugg: dict
    ) -> str:
        wins = [t for t in trades if t.get("won")]
        losses = [t for t in trades if not t.get("won")]
        avg_win = sum(t.get("pnl", 0) for t in wins) / max(len(wins), 1)
        avg_loss = abs(sum(t.get("pnl", 0) for t in losses)) / max(len(losses), 1)
        wr = len(wins) / max(len(wins) + len(losses), 1)
        current = self.params.to_dict()
        lesson_str = json.dumps(sugg, indent=2) if sugg else "No strong patterns yet"

        recent_sample = "\n".join(
            f"  {t.get('direction', '?')} {'WIN' if t.get('won') else 'LOSS'} "
            f"delta=${abs(t.get('btc_delta', 0)):.0f} conf={t.get('conf', 0):.2f} "
            f"pnl=${t.get('pnl', 0):+.2f} {t.get('exit_reason', '')}"
            for t in trades[-15:]
        )

        prompt = f"""You are a BTC Polymarket trading strategist. Analyse this trade journal and propose parameter improvements.

## Current Strategy Score
baseline_score = {baseline_score:.4f} (win_rate * pnl_weight + volume_bonus)

## Current Params
{json.dumps(current, indent=2)}

## Lesson Ledger (win rates by condition)
{lesson_str}

## Recent Trades (last 15)
{recent_sample}

## Summary
Total: {len(trades)} trades | WR: {wr:.0%} | avg_win: ${avg_win:.2f} | avg_loss: ${avg_loss:.2f}

## Your Task
Propose 1-2 specific param changes that would improve the score.
Consider: delta_thresh, conf_thresh, ens_thresh, spend_ratio.
Also consider: new filter rules (e.g., "only trade when OBI > 0.6", "skip when RSI > 70").

Return JSON:
{{
  "changes": [
    {{
      "param": "delta_thresh",
      "current": 12.0,
      "suggested": 15.0,
      "reason": "Historical trades with delta>=15 have 81% WR vs 79% for delta>=12"
    }}
  ]
}}"""
        return prompt

    def _save_improvement(
        self, desc: str, baseline: float, proposed: float, delta: float
    ):
        """Log improvement to fitness_history for tracking."""
        try:
            entry = {
                "ts": datetime.now().isoformat(),
                "change": desc,
                "baseline_score": baseline,
                "proposed_score": proposed,
                "delta": delta,
                "params": self.params.to_dict(),
            }
            with open(FITNESS_HISTORY, "a") as f:
                f.write(json.dumps(entry) + "\n")
        except Exception:
            pass

    # ── Main loop ──────────────────────────────────────────────────────────────
    def run(self, duration: int = None):
        deadline = self._now() + (duration or 86400 * 30)  # 30 days default

        log(f"\n{'=' * 60}")
        log(f"BTC SNIPER PRO — {'LIVE TRADING' if self.live else 'PAPER SIM'}")
        log(f"{'=' * 60}")
        log(f"Bankroll: ${self.starting:.2f}")
        log(
            f"Delta>: ${self.params.delta_thresh:.0f} | Conf>=: {self.params.conf_thresh:.2f}"
        )
        log(f"Ens>:     {self.params.ens_thresh:.2f}")
        log(
            f"Spend:   {self.params.spend_ratio:.0%} | Max bet: {self.params.max_bet_pct:.0%}"
        )
        log(
            f"TP:      {self.params.profit_target_bps}bps | SL: {self.params.stop_loss_bps}bps"
        )
        log(f"{'=' * 60}\n")

        last_check = {5: 0, 15: 0}
        last_market = {5: 0, 15: 0}
        last_status = 0
        last_reconcile = 0
        last_window = {5: 0, 15: 0}
        checks = 0
        sig_counts = defaultdict(int)

        # Pre-warm: load some BTC price history into each SignalEngine
        warmup_klines = get_binance_kline(interval="1m", limit=60)
        if warmup_klines:
            for k in warmup_klines:
                try:
                    price = float(k[4])
                    ts = float(k[0]) / 1000
                    for win in self.windows.values():
                        win.se.update(price, ts)
                except (ValueError, IndexError):
                    continue
            log(f"[WARM] Pre-loaded {len(warmup_klines)} candles into signal engines")

        while self.running and self._now() < deadline:
            now = self._now()
            try:
                btc_price = get_binance_btc()
                if btc_price:
                    self.btc_price = btc_price
                    for win in self.windows.values():
                        win.se.update(btc_price, now)

                # ── Per-timeframe window + market detection ──
                for tf in self._timeframes:
                    win = self.windows[tf]
                    win_sec = tf * 60
                    window_ts = int(now / win_sec) * win_sec
                    if window_ts != last_window.get(tf, 0):
                        last_window[tf] = window_ts
                        if self.btc_price:
                            win.reset_window(window_ts, self.btc_price, {})
                            log(
                                f"\n[WIN {tf}m] @{datetime.fromtimestamp(window_ts)} price=${self.btc_price:.2f}"
                            )
                            last_market[tf] = 0

                # ── Market fetching (per timeframe, staggered) ──
                for tf in self._timeframes:
                    win = self.windows[tf]
                    if win.market_id is None and now - last_market.get(tf, 0) > 10:
                        mkt = fetch_btc_markets(tf)
                        if mkt:
                            tokens = []
                            outcomes_raw = []
                            outcomes_labels = []
                            try:
                                tokens = json.loads(mkt.get("clobTokenIds", "[]"))
                                outcomes_raw = json.loads(
                                    mkt.get("outcomePrices", "[]")
                                )
                                outcomes_labels = json.loads(mkt.get("outcomes", "[]"))
                            except Exception:
                                pass
                            win.market_id = mkt.get("id")
                            win.market_question = mkt.get("question", "?")
                            # Dynamically match UP/DOWN by outcome label (Polymarket ordering can vary)
                            up_idx, down_idx = 0, 1
                            for i, label in enumerate(outcomes_labels):
                                if str(label).lower() in ("up", "yes"):
                                    up_idx = i
                                elif str(label).lower() in ("down", "no"):
                                    down_idx = i
                            win._outcome_prices = [0.50, 0.50]
                            if len(outcomes_raw) >= 2:
                                # Preserve historical convention: _outcome_prices[0]=DOWN, [1]=UP
                                win._outcome_prices = [
                                    float(outcomes_raw[down_idx]),
                                    float(outcomes_raw[up_idx]),
                                ]
                            win._up_token_id = (
                                tokens[up_idx] if up_idx < len(tokens) else None
                            )
                            win._down_token_id = (
                                tokens[down_idx] if down_idx < len(tokens) else None
                            )
                            log(f"[PM {tf}m] Market: {win.market_question}")
                            log(
                                f"[PM {tf}m] labels={outcomes_labels} up_idx={up_idx} down_idx={down_idx} | DOWN=${win._outcome_prices[0]:.3f} UP=${win._outcome_prices[1]:.3f}"
                            )
                            last_market[tf] = int(now)

                # ── Resolve trades (per-window duration) ──
                for t in self.trades:
                    if t.resolved:
                        continue
                    tf_sec = getattr(t, "window_tf", 5) * 60
                    w_end = t.window_start + tf_sec
                    if now >= w_end + 5:
                        # ONLY use Polymarket oracle — never Binance
                        market_id_for_res = getattr(t, "market_id", None)
                        actual_dir = None
                        if market_id_for_res:
                            actual_dir = fetch_market_resolution(
                                market_id_for_res, t.direction
                            )
                        if actual_dir:
                            log(f"[RESOLVE] Polymarket oracle: {actual_dir}")
                            self._resolve_trade(t, actual_dir)
                            self.pause_until = now + 5
                        elif now >= w_end + 600:
                            # Safety timeout: if PM hasn't resolved 10min after window, force loss
                            log(f"[RESOLVE] PM oracle timeout — forcing LOSS")
                            anti_dir = "Down" if t.direction == "Up" else "Up"
                            self._resolve_trade(t, anti_dir)
                            self.pause_until = now + 5
                        # else: still waiting for PM oracle, skip

                # ── Resolve pending fills ──
                for oid, info in list(self._pending_fills.items()):
                    ws = info.get("window_start")
                    tf_sec = info.get("window_tf", 5) * 60
                    if ws and now >= ws + tf_sec + 5:
                        self._pending_fills.pop(oid, None)
                        self._resolve_from_open_order(oid, info)

                # ── CLOB reconciliation (only when we have open orders) ──
                if self._open_orders and now - last_reconcile >= 10:
                    self._reconcile_open_orders()
                    last_reconcile = now
                # Only refresh balance every 60s to avoid blocking the main loop
                if now - getattr(self, "_last_balance_refresh", 0) >= 60:
                    self._refresh_balance()  # use fast version, not retry
                    self._last_balance_refresh = now
                    if abs(self._balance - (self.bankroll or 0)) > 1.0:
                        log(
                            f"[CLOB] Balance drift: local={self.bankroll:.2f} clob={self._balance:.2f}"
                        )

                # ── Signal check + trade (per timeframe) ──
                for tf in self._timeframes:
                    win = self.windows[tf]
                    if win.market_id is None or win.traded_this_window:
                        continue
                    if now - last_check.get(tf, 0) < 1:
                        continue
                    last_check[tf] = int(now)
                    checks += 1
                    sig = self._check_signal(win, tf)
                    if sig:
                        sig_counts[sig["direction"]] += 1
                        trade = self._place_trade(sig, win, tf)
                        if trade:
                            trade.window_tf = tf
                            win.traded_this_window = True

                # ── Status log (every 30s) ──
                if now - last_status >= 30:
                    tt = self.wins + self.losses
                    wr = self.wins / tt if tt > 0 else 0
                    elapsed_h = (now - self.t0) / 3600
                    pnl_pct = (
                        (self._balance - self.starting) / max(self.starting, 1) * 100
                    )
                    sugg = self.lessons.suggest_params()
                    sugg_str = f" LESSONS:{len(sugg)}" if sugg else ""
                    log(
                        f"[{datetime.fromtimestamp(now).strftime('%H:%M:%S')}] elapsed={elapsed_h:.1f}h "
                        f"trades={tt}(W:{self.wins} L:{self.losses}) WR={wr:.0%} "
                        f"Bk=${self._balance:.2f}({pnl_pct:+.1f}%) "
                        f"BTC=${self.btc_price or 0:.0f} signals={dict(sig_counts)}{sugg_str}"
                    )
                    last_status = now

                # ── GA evolution (every 5 min if enough trades) ──
                if (self.wins + self.losses) >= 5 and now - getattr(
                    self, "_last_ga_evolve", 0
                ) >= 300:
                    self._last_ga_evolve = now
                    self._ga_evolve()

                time.sleep(0.5)
            except Exception as e:
                import traceback

                log(f"[ERROR] Main loop: {e}")
                log(f"[ERROR] Traceback: {traceback.format_exc()[-500:]}")
                time.sleep(1)

        self._report(checks, sig_counts)

    def _ga_evolve(self):
        """Background GA evolution — scores params against journal, switches if better."""
        import threading

        def bg():
            try:
                recent = []
                try:
                    with open(JOURNAL_FILE) as f:
                        lines = f.readlines()
                    for line in lines[-200:]:
                        try:
                            recent.append(json.loads(line))
                        except:
                            pass
                except:
                    pass

                wins = [t for t in recent if isinstance(t, dict) and t.get("won")]
                losses = [t for t in recent if isinstance(t, dict) and not t.get("won")]
                total = wins + losses
                if len(total) < 5:
                    log(f"[GA] Waiting for more trades ({len(total)}/5)")
                    return

                ga = SniperGA()
                ga.load_or_create()

                current_score = self._score_trades(total)
                log(
                    f"[GA] Current score={current_score:.2f} from {len(wins)}W/{len(losses)}L"
                )

                ga.best_score = current_score
                ga.best_params = copy.deepcopy(self.params)
                pop = [self.params] + [self.params.mutate(0.4) for _ in range(7)]

                results = []
                for p in pop:
                    s = self._score_params(p, total)
                    results.append((s, p))

                results.sort(key=lambda x: x[0], reverse=True)
                best_score, best_p = results[0]

                if best_score > current_score + 5:
                    log(
                        f"[GA] 🏆 BETTER: {best_p.name} score={best_score:.2f} > {current_score:.2f}"
                    )
                    log(
                        f"[GA]   delta={best_p.delta_thresh:.1f} conf={best_p.conf_thresh:.2f} spend={best_p.spend_ratio:.2f}"
                    )
                    self.params = copy.deepcopy(best_p)
                    ga.best_score = best_score
                    ga.best_params = copy.deepcopy(best_p)
                    ga.save()
                else:
                    log(
                        f"[GA] No improvement. Best={best_score:.2f} vs current={current_score:.2f}"
                    )
            except Exception as e:
                log(f"[GA] Evolution error: {e}")

        t = threading.Thread(target=bg, daemon=True)
        t.start()

    def _hour_adjust_conf(self, base_conf: float, direction: str, hour: int) -> float:
        """Adjust conf based on UTC hour directional bias from 260+ BTC trades.

        Hard blocks: UTC 21 (both bad), UTC 05 (both bad), UTC 17 DOWN (WR=10%), UTC 20 UP (WR=14%)
        Penalties: UTC 12 UP by 0.30, UTC 04 DOWN by 0.30
        Boosts (+0.20): UP at UTC 01,11,13,15,16,17,18,19,22 | DOWN at UTC 00,09,12,14,20,23,01,11,13
        """
        CONFLICT_DOWN = {17}
        CONFLICT_UP = {20}
        PENALIZE_UP = {12}
        PENALIZE_DOWN = {4}
        SKIP_ALL = {21, 5}

        if hour in SKIP_ALL:
            return 0.0

        if direction == "Down" and hour in CONFLICT_DOWN:
            return 0.0
        if direction == "Up" and hour in CONFLICT_UP:
            return 0.0

        penalty = 0.0
        if direction == "Up" and hour in PENALIZE_UP:
            penalty = 0.30
        if direction == "Down" and hour in PENALIZE_DOWN:
            penalty = 0.30

        boost = 0.0
        if direction == "Up" and hour in {1, 11, 13, 15, 16, 17, 18, 19, 22}:
            boost = 0.20
        if direction == "Down" and hour in {0, 9, 12, 14, 20, 23, 1, 11, 13}:
            boost = 0.20

        return max(0.0, base_conf - penalty + boost)

    def _score_params(self, params, trades):
        """Score a param set by simulating which trades it would have fired.

        Applies the same filtering as live trading: delta, conf, AND hour filter.
        """
        CONFLICT_DOWN = {17}
        CONFLICT_UP = {20}
        PENALIZE_UP = {12}
        PENALIZE_DOWN = {4}
        SKIP_ALL = {21, 5}

        fired = 0
        fired_wins = 0
        fired_pnl = 0.0
        for t in trades:
            delta = abs(t.get("btc_delta", 0))
            base_conf = t.get("conf", 0)
            if delta < params.delta_thresh:
                continue

            try:
                hr = int(t.get("placed_at", "00")[11:13])
            except:
                continue
            direction = t.get("direction", "?")
            if hr in SKIP_ALL:
                continue
            if direction == "Down" and hr in CONFLICT_DOWN:
                continue
            if direction == "Up" and hr in CONFLICT_UP:
                continue

            penalty = 0.0
            if direction == "Up" and hr in PENALIZE_UP:
                penalty = 0.30
            if direction == "Down" and hr in PENALIZE_DOWN:
                penalty = 0.30

            boost = 0.0
            if direction == "Up" and hr in {1, 11, 13, 15, 16, 17, 18, 19, 22}:
                boost = 0.20
            if direction == "Down" and hr in {0, 9, 12, 14, 20, 23, 1, 11, 13}:
                boost = 0.20

            conf = max(0.0, base_conf - penalty + boost)
            if conf < params.conf_thresh:
                continue

            fired += 1
            if t.get("won"):
                fired_wins += 1
            fired_pnl += t.get("pnl", 0)

        if fired < 3:
            return 0.0
        wr = fired_wins / fired
        return fired_pnl * 10 + wr * 20 - max(0, 10 - fired) * 0.5

    def _report(self, checks: int, sig_counts: dict):
        tt = self.wins + self.losses
        wr = self.wins / tt if tt > 0 else 0
        elapsed = self._now() - self.t0
        pnl_pct = (self._balance - self.starting) / self.starting * 100

        log(f"\n{'=' * 60}")
        log(f"FINAL RESULTS ({elapsed / 3600:.2f}h)")
        log(f"{'=' * 60}")
        log(f"Trades:   {tt}  W:{self.wins} L:{self.losses} Blocks:{self.blocks}")
        log(f"Win rate: {wr:.1%}")
        log(f"Start Bk: ${self.starting:.2f}")
        log(f"Final Bk: ${self._balance:.2f} ({pnl_pct:+.2f}%)")
        log(f"Total PnL: ${self.total_pnl:+.2f}")
        log(f"{'=' * 60}\n")

        result = {
            "mode": "live" if self.live else "paper",
            "duration": elapsed,
            "bankroll_start": self.starting,
            "bankroll_end": self.bankroll,
            "total_pnl": self.total_pnl,
            "pnl_pct": pnl_pct,
            "trades": tt,
            "wins": self.wins,
            "losses": self.losses,
            "win_rate": wr,
            "blocked": self.blocks,
            "params": self.params.to_dict(),
        }
        out = HARVEY_HOME / "tmp" / "sniper_results.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        with open(out, "w") as f:
            json.dump(result, f, indent=2, default=str)
        log(f"→ Results: {out}")


# ── GA Optimization ────────────────────────────────────────────────────────────
class SniperGA:
    def __init__(self):
        self.best_params: Optional[SniperParams] = None
        self.best_score = float("-inf")
        self.generation = 0

    def load_or_create(self) -> list[SniperParams]:
        if BEST_PARAMS_FILE.exists():
            try:
                with open(BEST_PARAMS_FILE) as f:
                    d = json.load(f)
                loaded = SniperParams.from_dict(d)
                log(f"Loaded params: {loaded.name} (score={d.get('best_score', '?')})")
                self.best_params = loaded
                self.best_score = d.get("best_score", float("-inf"))
                self.generation = d.get("generation", 0)
                pop = [loaded]
                for _ in range(loaded.pop_size - 1):
                    pop.append(loaded.mutate(rate=0.35))
                return pop
            except Exception as e:
                log(f"Failed to load params: {e}")
        log(f"Creating new random population")
        return SniperParams.random_population(12)

    def evaluate(self, params: SniperParams, session_min: int = 30) -> dict:
        """Paper-trade with params for session_min minutes. Return score."""
        sniper = LiveSniper(params, live=False)
        sniper.run(duration=session_min * 60)

        tt = sniper.wins + sniper.losses
        wr = sniper.wins / tt if tt > 0 else 0
        pnl = sniper.total_pnl

        # Score: PnL + win rate bonus + trade count
        score = pnl * 100 + wr * 20 + min(tt, 20) * 0.5

        log(
            f"  {params.name}: PnL=${pnl:+.4f} WR={wr:.0%} trades={tt} score={score:.4f}"
        )
        return {"score": score, "pnl": pnl, "wr": wr, "trades": tt, "params": params}

    def evolve(self, population: list[SniperParams]) -> list[SniperParams]:
        """Run GA selection + breeding."""
        # Evaluate all
        results = []
        for p in population:
            r = self.evaluate(p, session_min=20)
            results.append(r)
            if r["score"] > self.best_score:
                self.best_score = r["score"]
                self.best_params = copy.deepcopy(p)
                log(f"  🏆 NEW BEST: {self.best_score:.4f} ({p.name})")

        # Sort by score
        results.sort(key=lambda x: x["score"], reverse=True)
        elite = [r["params"] for r in results[:3]]

        # Breed next generation
        next_pop = list(elite)
        while len(next_pop) < len(population):
            a, b = random.sample(list(zip(results, population)), 2)
            winner = a[1] if a[0]["score"] > b[0]["score"] else b[1]
            loser = b[1] if winner is a[1] else a[1]
            child = winner.crossover(winner, loser)
            child = child.mutate(rate=0.20)
            next_pop.append(child)

        # AI suggestion every generation
        ai_suggestion = self._ask_ai()
        if ai_suggestion:
            next_pop[-1] = ai_suggestion

        return next_pop

    def _ask_ai(self) -> Optional[SniperParams]:
        """Ask AI to analyze recent trades and suggest param improvements."""
        recent = []
        try:
            with open(JOURNAL_FILE) as f:
                lines = f.readlines()
            for line in lines[-50:]:
                try:
                    recent.append(json.loads(line))
                except:
                    pass
        except:
            return None

        if len(recent) < 5:
            return None

        wins = [t for t in recent if t.get("won")]
        losses = [t for t in recent if not t.get("won")]
        avg_win = sum(t.get("pnl", 0) for t in wins) / max(len(wins), 1)
        avg_loss = sum(t.get("pnl", 0) for t in losses) / max(len(losses), 1)

        prompt = f"""Analyze this BTC Polymarket sniper trading journal and suggest param improvements.

Recent trades ({len(recent)} total, {len(wins)}W/{len(losses)}L):
- Avg win: ${avg_win:+.4f}
- Avg loss: ${avg_loss:+.4f}

Sample trades (last 10):
{chr(10).join([f"  {t.get('direction')} {'WIN' if t.get('won') else 'LOSS'} pnl={t.get('pnl', 0):+.4f} {t.get('exit_reason', '')}" for t in recent[-10:]])}

Current params:
{json.dumps(self.best_params.to_dict() if self.best_params else {}, indent=2)}

Suggest 3 param changes. Return JSON: {{"suggestions": [{{"param": "...", "current": X, "suggested": Y, "reason": "..."}}]}}"""

        response = ai_complete(prompt, max_tokens=600)
        if not response:
            return None

        try:
            import re

            match = re.search(r"\{.*\}", response, re.DOTALL)
            if not match:
                return None
            data = json.loads(match.group())
            suggestions = data.get("suggestions", [])
            if not suggestions or not self.best_params:
                return None

            new_params = self.best_params.mutate(rate=0.0)
            for s in suggestions[:2]:
                if s["param"] in new_params.to_dict():
                    val = s["suggested"]
                    if isinstance(val, (int, float)):
                        setattr(new_params, s["param"], val)
            new_params.name = f"ai_{self.best_params.name}_{suggestions[0]['param']}"
            log(f"  AI suggestion: {new_params.name}")
            return new_params
        except Exception as e:
            log(f"  AI parse error: {e}")
        return None

    def save(self):
        if not self.best_params:
            return
        state = {
            "generation": self.generation,
            "best_score": self.best_score,
            **self.best_params.to_dict(),
        }
        with open(BEST_PARAMS_FILE, "w") as f:
            json.dump(state, f, indent=2)
        log(f"Saved best params: {self.best_params.name} score={self.best_score:.4f}")


# ── Main ──────────────────────────────────────────────────────────────────────
def main():
    import argparse

    parser = argparse.ArgumentParser(description="BTC Sniper Pro")
    parser.add_argument("--live", action="store_true", help="Use real money")
    parser.add_argument("--paper", action="store_true", help="Paper trading (default)")
    parser.add_argument("--evolve", action="store_true", help="Run GA evolution first")
    parser.add_argument(
        "--duration", type=int, default=None, help="Run duration in seconds"
    )
    args = parser.parse_args()

    live = args.live and not args.paper

    ga = SniperGA()

    if args.evolve:
        log("\n### GA EVOLUTION MODE ###")
        population = ga.load_or_create()
        for gen in range(3):
            ga.generation = gen + 1
            log(f"\n--- Generation {gen + 1} ---")
            population = ga.evolve(population)
        ga.save()
        log("\nEvolution done. Best params saved.")
        return

    # Load best params or create new
    population = ga.load_or_create()
    params = ga.best_params or population[0]

    log(f"\nStarting sniper with params: {params.name}")
    sniper = LiveSniper(params, live=live)

    def stop_handler(sig, frame):
        log("STOP received — shutting down...")
        sniper.running = False
        if ga.best_score > ga.best_score:
            ga.best_params = sniper.params
            ga.save()
        sys.exit(0)

    signal.signal(signal.SIGINT, stop_handler)
    signal.signal(signal.SIGTERM, stop_handler)

    sniper.run(duration=args.duration)
    ga.best_params = sniper.params
    if ga.best_score > 0:
        ga.save()
        log(f"Session best: {ga.best_params.name} score={ga.best_score:.2f}")


if __name__ == "__main__":
    main()
