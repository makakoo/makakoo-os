#!/usr/bin/env python3
"""
Technical Indicator Library for BTC/ETH/SOL multi-timeframe analysis.

Indicators:
- SMA, EMA (Moving Averages)
- RSI (Relative Strength Index)
- MACD (Moving Average Convergence Divergence)
- Bollinger Bands
- Fibonacci Retracements
- Support/Resistance levels
- ATR (Average True Range)
- Volume analysis

Usage:
    from indicators import Indicator

    candles = engine.candles("btcusdt", "1m", 200)
    closes = [c.close for c in candles]

    rsi = Indicator.rsi(closes)
    macd = Indicator.macd(closes)
    bb = Indicator.bollinger_bands(closes)
    fib = Indicator.fib_retracements(closes)
    sr = Indicator.support_resistance(closes)
"""

import math
from collections import deque
from typing import List, Optional, Tuple


class Indicator:
    # ─────────────────────────────────────────────────────────────────
    # Moving Averages
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def sma(values: List[float], period: int) -> List[float]:
        if len(values) < period:
            return []
        result = []
        for i in range(period - 1, len(values)):
            result.append(sum(values[i - period + 1 : i + 1]) / period)
        return result

    @staticmethod
    def ema(values: List[float], period: int) -> List[float]:
        if len(values) < period:
            return []
        k = 2 / (period + 1)
        result = [sum(values[:period]) / period]
        for v in values[period:]:
            result.append(v * k + result[-1] * (1 - k))
        return result

    @staticmethod
    def vwap(candles) -> float:
        """Volume-Weighted Average Price."""
        total_pv = sum(c.close * c.volume for c in candles)
        total_v = sum(c.volume for c in candles)
        return total_pv / total_v if total_v else 0

    # ─────────────────────────────────────────────────────────────────
    # RSI
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def rsi(values: List[float], period: int = 14) -> List[float]:
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

    # ─────────────────────────────────────────────────────────────────
    # MACD
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def macd(
        values: List[float], fast: int = 12, slow: int = 26, signal: int = 9
    ) -> Tuple[List[float], List[float], List[float]]:
        if len(values) < slow + signal:
            return [], [], []
        ema_fast = Indicator.ema(values, fast)
        ema_slow = Indicator.ema(values, slow)

        macd_line = [ema_fast[i] - ema_slow[i] for i in range(len(ema_slow))]
        signal_line = Indicator.ema(macd_line, signal)
        histogram = []
        offset = len(macd_line) - len(signal_line)
        for i in range(len(signal_line)):
            histogram.append(macd_line[i + offset] - signal_line[i])

        return macd_line, signal_line, histogram

    # ─────────────────────────────────────────────────────────────────
    # Bollinger Bands
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def bollinger_bands(
        values: List[float], period: int = 20, std_dev: float = 2.0
    ) -> Tuple[List[float], List[float], List[float]]:
        if len(values) < period:
            return [], [], []
        sma_vals = Indicator.sma(values, period)
        upper, middle, lower = [], [], []
        for i in range(period - 1, len(values)):
            window = values[i - period + 1 : i + 1]
            mean = sma_vals[i - (period - 1)]
            sd = math.sqrt(sum((v - mean) ** 2 for v in window) / period)
            idx = i - (period - 1)
            middle.append(mean)
            upper.append(mean + std_dev * sd)
            lower.append(mean - std_dev * sd)
        return upper, middle, lower

    # ─────────────────────────────────────────────────────────────────
    # ATR (Average True Range)
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def atr(candles, period: int = 14) -> List[float]:
        if len(candles) < period + 1:
            return []
        trs = []
        for i in range(1, len(candles)):
            high = candles[i].high
            low = candles[i].low
            prev_close = candles[i - 1].close
            tr = max(high - low, abs(high - prev_close), abs(low - prev_close))
            trs.append(tr)
        return Indicator.sma(trs, period) if len(trs) >= period else []

    # ─────────────────────────────────────────────────────────────────
    # Fibonacci Retracements
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def fib_retracements(candles_or_values) -> dict:
        """Calculate Fibonacci retracement levels from a swing high/low."""
        if hasattr(candles_or_values, "__getitem__") and hasattr(
            candles_or_values[0], "close"
        ):
            values = [c.close for c in candles_or_values]
        else:
            values = candles_or_values

        if len(values) < 2:
            return {}

        high = max(values)
        low = min(values)
        diff = high - low
        levels = {
            "0.0": high,
            "23.6%": high - diff * 0.236,
            "38.2%": high - diff * 0.382,
            "50.0%": high - diff * 0.500,
            "61.8%": high - diff * 0.618,
            "78.6%": high - diff * 0.786,
            "100.0%": low,
            "161.8%": high + diff * 0.618,
            "261.8%": high + diff * 1.618,
        }
        return levels

    # ─────────────────────────────────────────────────────────────────
    # Support & Resistance
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def support_resistance(
        candles, lookback: int = 50
    ) -> Tuple[List[float], List[float]]:
        """Find support and resistance levels using pivot point detection."""
        if len(candles) < lookback:
            lookback = len(candles)

        window = candles[-lookback:]
        highs = [c.high for c in window]
        lows = [c.low for c in window]
        closes = [c.close for c in window]

        sr_levels = []

        for i in range(2, len(window) - 2):
            c = window[i]
            p = window[i - 2]
            n = window[i + 2]

            is_resistance = (
                p.high < c.high
                and n.high < c.high
                and window[i - 1].high < c.high
                and window[i + 1].high < c.high
            )
            is_support = (
                p.low > c.low
                and n.low > c.low
                and window[i - 1].low > c.low
                and window[i + 1].low > c.low
            )
            if is_resistance:
                sr_levels.append(("R", c.high))
            elif is_support:
                sr_levels.append(("S", c.low))

        resistances = sorted(set(round(r[1], 2) for r in sr_levels if r[0] == "R"))
        supports = sorted(set(round(s[1], 2) for s in sr_levels if s[0] == "S"))

        return supports, resistances

    # ─────────────────────────────────────────────────────────────────
    # Signal Generators
    # ─────────────────────────────────────────────────────────────────

    @staticmethod
    def signal_rsi(
        closes: List[float],
        period: int = 14,
        oversold: float = 30,
        overbought: float = 70,
    ) -> str:
        """RSI signal: oversold, overbought, or neutral."""
        rsi_vals = Indicator.rsi(closes, period)
        if not rsi_vals:
            return "neutral"
        r = rsi_vals[-1]
        if r < oversold:
            return "oversold"
        elif r > overbought:
            return "overbought"
        return "neutral"

    @staticmethod
    def signal_macd(closes: List[float]) -> str:
        """MACD signal: bullish, bearish, or neutral."""
        macd_line, signal_line, histogram = Indicator.macd(closes)
        if not histogram:
            return "neutral"
        h = histogram[-1]
        if h > 0 and len(histogram) > 1 and histogram[-2] <= 0:
            return "bullish_cross"
        elif h < 0 and len(histogram) > 1 and histogram[-2] >= 0:
            return "bearish_cross"
        elif h > 0:
            return "bullish"
        return "bearish"

    @staticmethod
    def signal_bb(closes: List[float], period: int = 20) -> str:
        """Bollinger Bands signal: near_upper, near_lower, or neutral."""
        upper, middle, lower = Indicator.bollinger_bands(closes, period)
        if not upper:
            return "neutral"
        cur = closes[-1]
        u, l = upper[-1], lower[-1]
        if cur >= u * 0.99:
            return "near_upper"
        elif cur <= l * 1.01:
            return "near_lower"
        return "neutral"

    @staticmethod
    def momentum_score(closes: List[float], period: int = 14) -> float:
        """Composite momentum score: -100 to +100."""
        if len(closes) < period + 1:
            return 0.0

        rsi_vals = Indicator.rsi(closes, period)
        macd_line, signal_line, histogram = Indicator.macd(closes)
        bb_signal = Indicator.signal_bb(closes)

        rsi_score = (rsi_vals[-1] - 50) * 2 if rsi_vals else 0
        macd_score = 0
        if macd_line and signal_line:
            macd_score = (
                (macd_line[-1] - signal_line[-1]) / (max(closes[-1], 1) or 1) * 10000
            )

        bb_score = 0
        if bb_signal == "near_lower":
            bb_score = 50
        elif bb_signal == "near_upper":
            bb_score = -50

        return rsi_score * 0.4 + macd_score * 0.3 + bb_score * 0.3


if __name__ == "__main__":
    import random

    print("Testing indicators...")
    prices = [100 + random.gauss(0, 1) * 2 for _ in range(200)]
    print(f"RSI: {Indicator.rsi(prices)[-1]:.1f}")
    macd, sig, hist = Indicator.macd(prices)
    print(f"MACD: {macd[-1]:.4f}, Signal: {sig[-1]:.4f}")
    bb_u, bb_m, bb_l = Indicator.bollinger_bands(prices)
    print(f"BB: upper={bb_u[-1]:.2f}, middle={bb_m[-1]:.2f}, lower={bb_l[-1]:.2f}")
    fib = Indicator.fib_retracements(prices)
    print(f"Fib 61.8%: {fib.get('61.8%', 0):.2f}")
    sr = Indicator.support_resistance(prices)
    print(f"Support levels: {sr[0]}, Resistance: {sr[1]}")
