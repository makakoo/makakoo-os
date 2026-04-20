#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Signal Generator — Multi-timeframe technical analysis for Polymarket signals.

Architecture:
  CandleEngine (WebSocket) → Candles → Indicators (RSI/MACD/BB/Fib/SR)
  → Multi-TF composite signal → Probability of next move → Polymarket trade

Signal scoring:
  composite_score: -100 (strong bearish) to +100 (strong bullish)
  confidence: 0.0–1.0 based on indicator agreement across timeframes
  probability_up: 0.0–1.0 (implied probability BTC goes UP next)
  probability_down: 1.0 - probability_up

The Polymarket trader buys YES (bet UP) or NO (bet DOWN) based on these signals.
"""

import math
import time
from dataclasses import dataclass
from typing import Optional

from indicators import Indicator


@dataclass
class TimeframeSignal:
    timeframe: str
    direction: str  # "up", "down", "neutral"
    confidence: float  # 0.0–1.0
    rsi: float
    macd_signal: str
    bb_signal: str
    near_support: bool
    near_resistance: bool
    score: float  # -100 to +100


@dataclass
class CompositeSignal:
    """Combined signal from all timeframes."""

    direction: str  # "up", "down", "neutral"
    confidence: float  # 0.0–1.0
    probability_up: float  # 0.0–1.0
    probability_down: float  # 0.0–1.0
    score: float  # -100 to +100
    btc_price: float
    timeframe_signals: list[TimeframeSignal]
    fib_level: float
    nearest_support: float
    nearest_resistance: float
    reasoning: list[str]
    timestamp: float


class BTCSignalGenerator:
    """
    Multi-timeframe BTC technical analysis → Polymarket trade signals.

    Timeframes analyzed: 5s, 15s, 1m, 5m, 15m
    Indicators: RSI, MACD, Bollinger Bands, Fib, S/R

    Indicator params can be overridden via config dict (e.g. StrategyGenome).
    Defaults: RSI=7, MACD=(6,13,5), BB=(10,2.0), S/R lookback=20
    """

    _RSI_PERIOD = 7
    _MACD_FAST = 6
    _MACD_SLOW = 13
    _MACD_SIGNAL = 5
    _BB_PERIOD = 10
    _BB_STD = 2.0
    _SR_LOOKBACK = 20
    _MIN_PRICES = 25

    def __init__(self, min_prices: int = 25, config=None):
        self.min_prices = min_prices
        self.last_signal: Optional[CompositeSignal] = None
        self.cfg = config

    def _p(self, name: str, default):
        if self.cfg is None:
            return default
        return getattr(self.cfg, name, default)

    @property
    def RSI_PERIOD(self):
        return self._p("rsi_period", self._RSI_PERIOD)

    @property
    def MACD_FAST(self):
        return self._p("macd_fast", self._MACD_FAST)

    @property
    def MACD_SLOW(self):
        return self._p("macd_slow", self._MACD_SLOW)

    @property
    def MACD_SIGNAL(self):
        return self._p("macd_signal", self._MACD_SIGNAL)

    @property
    def BB_PERIOD(self):
        return self._p("bb_period", self._BB_PERIOD)

    @property
    def BB_STD(self):
        return self._p("bb_std", self._BB_STD)

    @property
    def SR_LOOKBACK(self):
        return self._p("sr_lookback", self._SR_LOOKBACK)

    def _analyze_prices(self, closes: list[float]) -> Optional[TimeframeSignal]:
        """Analyze raw price series and return a single composite signal."""
        if len(closes) < self.min_prices:
            return None

        cur = closes[-1]

        rsi_vals = Indicator.rsi(closes, self.RSI_PERIOD)
        rsi = rsi_vals[-1] if rsi_vals else 50.0

        macd_line, signal_line, histogram = Indicator.macd(
            closes, self.MACD_FAST, self.MACD_SLOW, self.MACD_SIGNAL
        )
        if histogram and len(histogram) >= 2:
            h_now = histogram[-1]
            h_prev = histogram[-2]
            if h_now > 0 and h_prev <= 0:
                macd_signal = "bullish_cross"
            elif h_now < 0 and h_prev >= 0:
                macd_signal = "bearish_cross"
            elif h_now > 0:
                macd_signal = "bullish"
            else:
                macd_signal = "bearish"
        else:
            macd_signal = "neutral"

        bb_upper, bb_middle, bb_lower = Indicator.bollinger_bands(
            closes, self.BB_PERIOD, self.BB_STD
        )
        if bb_upper and bb_lower:
            bb_upper_val = bb_upper[-1]
            bb_lower_val = bb_lower[-1]
            bb_mid_val = bb_middle[-1]
            band_width = bb_upper_val - bb_lower_val
            position_pct = (cur - bb_lower_val) / band_width if band_width > 0 else 0.5
            if position_pct <= 0.10:
                bb_signal = "near_lower"
            elif position_pct >= 0.90:
                bb_signal = "near_upper"
            elif cur < bb_mid_val:
                bb_signal = "below_middle"
            else:
                bb_signal = "above_middle"
        else:
            bb_signal = "neutral"

        # Support / Resistance on raw closes
        from indicators import Indicator as Ind

        lookback = min(self.SR_LOOKBACK, len(closes) - 1)
        sr_candles = [
            type("C", (), {"close": c, "high": c, "low": c})()
            for c in closes[-lookback:]
        ]
        supports, resistances = Ind.support_resistance(sr_candles, lookback=lookback)
        near_support = False
        near_resistance = False
        nearest_support = cur * 0.995
        nearest_resistance = cur * 1.005
        below_supports = [s for s in supports if s < cur]
        above_resistances = [r for r in resistances if r > cur]
        if below_supports:
            nearest_support = max(below_supports)
            near_support = (cur - nearest_support) / cur < 0.005
        if above_resistances:
            nearest_resistance = min(above_resistances)
            near_resistance = (nearest_resistance - cur) / cur < 0.005

        score = self._score(rsi, macd_signal, bb_signal, near_support, near_resistance)

        if score > 10:
            direction = "up"
        elif score < -10:
            direction = "down"
        else:
            direction = "neutral"

        confidence = min(1.0, abs(score) / 50.0)

        return TimeframeSignal(
            timeframe="raw",
            direction=direction,
            confidence=confidence,
            rsi=rsi,
            macd_signal=macd_signal,
            bb_signal=bb_signal,
            near_support=near_support,
            near_resistance=near_resistance,
            score=score,
        )

    def _score(
        self,
        rsi: float,
        macd_signal: str,
        bb_signal: str,
        near_support: bool,
        near_resistance: bool,
    ) -> float:
        rw = self._p("rsi_weight", 1.0)
        mw = self._p("macd_weight", 1.0)
        bw = self._p("bb_weight", 1.0)
        sw = self._p("sr_weight", 1.0)

        score = 0.0
        if rsi < 30:
            score += 40 * (30 - rsi) / 30 * rw
        elif rsi > 70:
            score -= 40 * (rsi - 70) / 30 * rw
        elif rsi < 45:
            score -= (45 - rsi) * 0.5 * rw
        elif rsi > 55:
            score += (rsi - 55) * 0.5 * rw

        macd_scores = {
            "bullish_cross": 30,
            "bullish": 15,
            "neutral": 0,
            "bearish": -15,
            "bearish_cross": -30,
        }
        score += macd_scores.get(macd_signal, 0) * mw

        if bb_signal == "near_lower":
            score += 20 * bw
        elif bb_signal == "near_upper":
            score -= 20 * bw
        elif bb_signal == "below_middle":
            score -= 5 * bw
        elif bb_signal == "above_middle":
            score += 5 * bw

        if near_support:
            score += 10 * sw
        if near_resistance:
            score -= 10 * sw

        return max(-100.0, min(100.0, score))

    def generate(self, price_feed) -> Optional[CompositeSignal]:
        """Generate composite signal from BTCPriceFeed raw prices."""
        price, ts = price_feed.latest()
        if price is None:
            return None

        closes = price_feed.closes(200)
        if len(closes) < self.min_prices:
            return None

        sig = self._analyze_prices(closes)
        if sig is None:
            return None

        composite_score = sig.score
        avg_rsi = sig.rsi

        raw_prob_up = (composite_score + 100) / 200
        prob_up = max(0.05, min(0.95, raw_prob_up))
        prob_down = 1.0 - prob_up

        if composite_score > 15:
            direction = "up"
        elif composite_score < -15:
            direction = "down"
        else:
            direction = "neutral"

        # Fibonacci on all closes
        fib_dict = Indicator.fib_retracements(closes)
        nearest_support = price * 0.995
        nearest_resistance = price * 1.005
        fib_level = 0.0
        if fib_dict:
            below_levels = [(k, v) for k, v in fib_dict.items() if v < price]
            above_levels = [(k, v) for k, v in fib_dict.items() if v > price]
            if below_levels:
                nearest_support = max(v for _, v in below_levels)
                fib_level = nearest_support
            if above_levels:
                nearest_resistance = min(v for _, v in above_levels)

        reasoning_parts = []
        if avg_rsi < 35:
            reasoning_parts.append(f"RSI oversold ({avg_rsi:.0f})")
        elif avg_rsi > 65:
            reasoning_parts.append(f"RSI overbought ({avg_rsi:.0f})")
        else:
            reasoning_parts.append(f"RSI={avg_rsi:.0f}")

        reasoning_parts.append(
            f"MACD={sig.macd_signal} BB={sig.bb_signal} → {direction.upper()} "
            f"score={composite_score:.0f}"
        )

        if sig.near_support:
            reasoning_parts.append(f"Near support ${nearest_support:,.0f}")
        if sig.near_resistance:
            reasoning_parts.append(f"Near resistance ${nearest_resistance:,.0f}")

        signal = CompositeSignal(
            direction=direction,
            confidence=sig.confidence,
            probability_up=prob_up,
            probability_down=prob_down,
            score=composite_score,
            btc_price=price,
            timeframe_signals=[sig],
            fib_level=fib_level,
            nearest_support=nearest_support,
            nearest_resistance=nearest_resistance,
            reasoning=reasoning_parts,
            timestamp=ts,
        )

        self.last_signal = signal
        return signal

    def should_trade(self, signal: CompositeSignal, genome) -> bool:
        if signal.direction == "neutral":
            return False
        if signal.confidence < genome.min_confidence:
            return False
        return True


if __name__ == "__main__":
    import sys

    sys.path.insert(0, str(__file__).rsplit("/", 1)[0])

    from btc_price_feed import BTCPriceFeed

    print("=== BTC Signal Generator — Live Test ===")
    feed = BTCPriceFeed().start()
    print("Waiting 10s for Binance warmup...")
    time.sleep(10)

    gen = BTCSignalGenerator(min_prices=25)

    for i in range(5):
        sig = gen.generate(feed)
        if sig:
            print(f"\n--- Signal {i + 1} ---")
            print(f"  Direction: {sig.direction} | Confidence: {sig.confidence:.0%}")
            print(f"  BTC: ${sig.btc_price:,.0f}")
            print(
                f"  P(up): {sig.probability_up:.1%} | P(down): {sig.probability_down:.1%}"
            )
            print(f"  Score: {sig.score:.0f}")
            for r in sig.reasoning:
                print(f"  → {r}")
            print("  Timeframe breakdown:")
            for ts in sig.timeframe_signals:
                print(
                    f"    {ts.timeframe}: score={ts.score:+.0f} rsi={ts.rsi:.0f} macd={ts.macd_signal} bb={ts.bb_signal}"
                )
        else:
            price, _ = feed.latest()
            closes = feed.closes(20)
            c5m = feed.candles(300, 10)
            print(
                f"  [{i + 1}] No signal — BTC=${price} prices={len(closes)} 5m_candles={len(c5m)}"
            )
        time.sleep(10)

    feed.stop()
