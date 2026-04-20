#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Data Fetcher — Binance klines for BTC/USDT.

Fetches OHLCV data from Binance public API (no auth needed).
Supports multiple timeframes: 1m, 5m, 15m, 1h, 4h, 1d.
"""

import requests
import time
from dataclasses import dataclass
from typing import Optional


BINANCE_API = "https://api.binance.com/api/v3/klines"

INTERVAL_MAP = {
    "1m": "1m",
    "5m": "5m",
    "15m": "15m",
    "1h": "1h",
    "4h": "4h",
    "1d": "1d",
}


@dataclass
class BTCCandle:
    open_time: int
    open: float
    high: float
    low: float
    close: float
    volume: float
    close_time: int
    quote_volume: float
    trades: int


@dataclass
class BTCFeatures:
    """Computed technical indicators from BTC candles."""

    price: float
    momentum_1m: float  # % change, last 1m candle
    momentum_5m: float  # % change, last 5m
    momentum_15m: float  # % change, last 15m
    momentum_1h: float  # % change, last 1h
    rsi_14: float  # RSI(14)
    macd_hist: float  # MACD histogram
    macd_signal: float  # MACD signal line
    bb_position: float  # BB position 0-1 (0=lower, 0.5=middle, 1=upper)
    volume_ratio: float  # recent vol / avg vol
    trend_1h: float  # 1h momentum sign: +1, 0, -1
    trend_4h: float  # 4h momentum sign


class BTCDataFetcher:
    def __init__(self, symbol: str = "BTCUSDT"):
        self.symbol = symbol
        self._cache: dict[str, list] = {}
        self._cache_time: dict[str, float] = {}
        self.cache_ttl = 30  # seconds

    def _fetch_klines(self, interval: str, limit: int = 200) -> list:
        """Fetch raw klines from Binance."""
        now = time.time()
        cache_key = f"{interval}_{limit}"

        if (
            cache_key in self._cache
            and (now - self._cache_time.get(cache_key, 0)) < self.cache_ttl
        ):
            return self._cache[cache_key]

        try:
            resp = requests.get(
                BINANCE_API,
                params={
                    "symbol": self.symbol,
                    "interval": interval,
                    "limit": limit,
                },
                timeout=10,
            )
            if resp.status_code == 200:
                raw = resp.json()
                self._cache[cache_key] = raw
                self._cache_time[cache_key] = now
                return raw
        except Exception as e:
            print(f"[BTC] Binance fetch error: {e}")
        return []

    def _parse_candle(self, raw: list) -> BTCCandle:
        return BTCCandle(
            open_time=int(raw[0]),
            open=float(raw[1]),
            high=float(raw[2]),
            low=float(raw[3]),
            close=float(raw[4]),
            volume=float(raw[5]),
            close_time=int(raw[6]),
            quote_volume=float(raw[7]),
            trades=int(raw[8]),
        )

    def get_candles(self, interval: str = "1m", limit: int = 200) -> list[BTCCandle]:
        raw = self._fetch_klines(interval, limit)
        return [self._parse_candle(r) for r in raw]

    def _compute_rsi(self, closes: list, period: int = 14) -> float:
        if len(closes) < period + 1:
            return 50.0
        deltas = [closes[i + 1] - closes[i] for i in range(len(closes) - 1)]
        gains = [d if d > 0 else 0 for d in deltas[-period:]]
        losses = [-d if d < 0 else 0 for d in deltas[-period:]]
        avg_gain = sum(gains) / period
        avg_loss = sum(losses) / period
        if avg_loss == 0:
            return 100.0
        rs = avg_gain / avg_loss
        return 100 - (100 / (1 + rs))

    def _compute_macd(
        self, closes: list, fast: int = 12, slow: int = 26, signal: int = 9
    ):
        if len(closes) < slow + signal:
            return 0.0, 0.0

        # EMA
        def ema(data, n):
            k = 2 / (n + 1)
            result = [data[0]]
            for d in data[1:]:
                result.append(d * k + result[-1] * (1 - k))
            return result

        ema_fast = ema(closes[-slow - 1 :], fast)
        ema_slow = ema(closes[-slow - 1 :], slow)
        macd_line = ema_fast[-1] - ema_slow[-1]
        macd_series = []
        for i in range(len(closes)):
            ef = ema(closes[: i + 1], fast)
            es = ema(closes[: i + 1], slow)
            if i >= slow - 1:
                macd_series.append(ef[-1] - es[-1])
        signal_line = (
            ema(macd_series[-signal:], signal)[-1] if len(macd_series) >= signal else 0
        )
        macd_hist = macd_line - signal_line
        return macd_hist, signal_line

    def _compute_bb(self, closes: list, period: int = 20, std_mult: float = 2.0):
        if len(closes) < period:
            return 0.5
        recent = closes[-period:]
        sma = sum(recent) / len(recent)
        variance = sum((c - sma) ** 2 for c in recent) / len(recent)
        std = variance**0.5
        upper = sma + std_mult * std
        lower = sma - std_mult * std
        current = closes[-1]
        if upper == lower:
            return 0.5
        return max(0.0, min(1.0, (current - lower) / (upper - lower)))

    def features(self) -> BTCFeatures:
        """Compute full technical indicator set across timeframes."""
        # Multi-timeframe closes
        c1m = self.get_candles("1m", 60)
        c5m = self.get_candles("5m", 60)
        c15m = self.get_candles("15m", 60)
        c1h = self.get_candles("1h", 200)
        c4h = self.get_candles("4h", 200)

        closes_1m = [c.close for c in c1m]
        closes_5m = [c.close for c in c5m]
        closes_15m = [c.close for c in c15m]
        closes_1h = [c.close for c in c1h]
        closes_4h = [c.close for c in c4h]

        price = closes_1m[-1] if closes_1m else 0.0

        # Momentum (% change)
        def mom(closes_list, n):
            if len(closes_list) < n + 1:
                return 0.0
            return (closes_list[-1] - closes_list[-n - 1]) / closes_list[-n - 1] * 100

        mom_1m = mom(closes_1m, 1)
        mom_5m = mom(closes_5m, 1)
        mom_15m = mom(closes_15m, 1)
        mom_1h = mom(closes_1h, 1)

        # Volume ratio (recent 5m avg / 1h avg)
        if len(c5m) >= 12 and len(c1h) >= 12:
            recent_vol = sum(c.volume for c in c5m[-12:]) / 12
            avg_vol = sum(c.volume for c in c1h[-12:]) / 12
            vol_ratio = recent_vol / max(avg_vol, 0.001)
        else:
            vol_ratio = 1.0

        # RSI on 1h
        rsi_14 = self._compute_rsi(closes_1h, 14)
        macd_hist, macd_signal = self._compute_macd(closes_1h)
        bb_pos = self._compute_bb(closes_1h)

        # Trend sign
        trend_1h = 1 if mom_1h > 0.1 else (-1 if mom_1h < -0.1 else 0)
        trend_4h = (
            1 if mom(closes_4h, 1) > 0.1 else (-1 if mom(closes_4h, 1) < -0.1 else 0)
        )

        return BTCFeatures(
            price=price,
            momentum_1m=mom_1m,
            momentum_5m=mom_5m,
            momentum_15m=mom_15m,
            momentum_1h=mom_1h,
            rsi_14=rsi_14,
            macd_hist=macd_hist,
            macd_signal=macd_signal,
            bb_position=bb_pos,
            volume_ratio=vol_ratio,
            trend_1h=trend_1h,
            trend_4h=trend_4h,
        )


if __name__ == "__main__":
    fetcher = BTCDataFetcher()
    f = fetcher.features()
    print(f"BTC: ${f.price:,.0f}")
    print(
        f"Momentum — 1m: {f.momentum_1m:+.3f}%  5m: {f.momentum_5m:+.3f}%  15m: {f.momentum_15m:+.3f}%  1h: {f.momentum_1h:+.3f}%"
    )
    print(
        f"RSI(14): {f.rsi_14:.1f}  MACD hist: {f.macd_hist:+.4f}  BB pos: {f.bb_position:.3f}"
    )
    print(
        f"Vol ratio: {f.volume_ratio:.2f}x  Trend 1h: {f.trend_1h}  Trend 4h: {f.trend_4h}"
    )
