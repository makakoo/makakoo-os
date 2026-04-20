#!/usr/bin/env python3
"""
Multi-Timeframe Candle Engine.

Consumes Polymarket RTDS WebSocket stream and builds candles
for multiple timeframes simultaneously (5s, 10s, 15s, 30s, 1m, 3m, 5m, 15m).

Usage:
    engine = CandleEngine(symbols=["btcusdt", "ethusdt"])
    engine.start()
    candles = engine.candles("btcusdt", timeframe="1m")
    engine.stop()
"""

import json
import threading
import time
import websocket
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Dict, List, Optional


@dataclass
class Candle:
    ts: int
    open: float
    high: float
    low: float
    close: float
    volume: float
    timeframe: str
    symbol: str


class CandleBuilder:
    """Builds candles for one symbol from tick stream."""

    TIMEFRAMES = {
        "5s": 5,
        "10s": 10,
        "15s": 15,
        "30s": 30,
        "1m": 60,
        "3m": 180,
        "5m": 300,
        "15m": 900,
    }

    def __init__(self, symbol: str):
        self.symbol = symbol
        self.ticks: List[tuple] = []
        self.candles: Dict[str, List[Candle]] = {tf: [] for tf in self.TIMEFRAMES}
        self._current: Dict[str, Optional[dict]] = {tf: None for tf in self.TIMEFRAMES}
        self._lock = threading.Lock()

    def ingest(self, ts_ms: int, price: float, size: float = 1.0):
        with self._lock:
            self.ticks.append((ts_ms, price, size))
            if len(self.ticks) > 10000:
                self.ticks = self.ticks[-5000:]

            for tf_name, tf_sec in self.TIMEFRAMES.items():
                bucket = (ts_ms // (tf_sec * 1000)) * (tf_sec * 1000)

                if (
                    self._current[tf_name] is None
                    or self._current[tf_name]["bucket"] != bucket
                ):
                    if self._current[tf_name] is not None:
                        c = self._current[tf_name]
                        self.candles[tf_name].append(
                            Candle(
                                ts=c["bucket"],
                                open=c["open"],
                                high=c["high"],
                                low=c["low"],
                                close=c["close"],
                                volume=c["volume"],
                                timeframe=tf_name,
                                symbol=self.symbol,
                            )
                        )
                        if len(self.candles[tf_name]) > 1000:
                            self.candles[tf_name] = self.candles[tf_name][-500:]

                    self._current[tf_name] = {
                        "bucket": bucket,
                        "open": price,
                        "high": price,
                        "low": price,
                        "close": price,
                        "volume": size,
                    }
                else:
                    c = self._current[tf_name]
                    c["high"] = max(c["high"], price)
                    c["low"] = min(c["low"], price)
                    c["close"] = price
                    c["volume"] += size

    def get_candles(self, timeframe: str, n: int = 100) -> List[Candle]:
        if timeframe not in self.TIMEFRAMES:
            raise ValueError(f"Unknown timeframe: {timeframe}")
        with self._lock:
            cs = list(self.candles[timeframe])
            if self._current[timeframe] is not None:
                c = self._current[timeframe]
                cs.append(
                    Candle(
                        ts=c["bucket"],
                        open=c["open"],
                        high=c["high"],
                        low=c["low"],
                        close=c["close"],
                        volume=c["volume"],
                        timeframe=timeframe,
                        symbol=self.symbol,
                    )
                )
            return cs[-n:]

    def latest_price(self) -> Optional[float]:
        with self._lock:
            return self.ticks[-1][1] if self.ticks else None


class CandleEngine:
    """
    Multi-timeframe candle engine consuming Polymarket RTDS WebSocket.

    Builds candles for: 5s, 10s, 15s, 30s, 1m, 3m, 5m, 15m
    """

    RTDS_URL = "wss://ws-live-data.polymarket.com"

    def __init__(self, symbols: List[str] = None):
        self.symbols = symbols or ["btcusdt", "ethusdt", "solusdt"]
        self.builders: Dict[str, CandleBuilder] = {
            sym: CandleBuilder(sym) for sym in self.symbols
        }
        self.ws: Optional[websocket.WebSocketApp] = None
        self._thread: Optional[threading.Thread] = None
        self._running = False
        self._price_cache: Dict[str, float] = {}
        self._ts_cache: Dict[str, int] = {}

    def _on_message(self, ws, message):
        try:
            d = json.loads(message)
            if d.get("topic") != "crypto_prices":
                return
            payload = d.get("payload", {})
            sym = payload.get("symbol", "")
            if sym not in self.builders:
                return
            ts = int(payload.get("timestamp", 0))
            price = float(payload.get("value", 0))
            size = float(payload.get("size", 1.0))
            if ts and price:
                self._price_cache[sym] = price
                self._ts_cache[sym] = ts
                self.builders[sym].ingest(ts, price, size)
        except (json.JSONDecodeError, ValueError, TypeError):
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

    def _on_error(self, ws, error):
        pass

    def _run_loop(self):
        while self._running:
            try:
                self.ws = websocket.WebSocketApp(
                    self.RTDS_URL,
                    on_message=self._on_message,
                    on_open=self._on_open,
                    on_error=self._on_error,
                )
                self.ws.run_forever(ping_interval=20, ping_timeout=10)
            except Exception:
                pass
            if self._running:
                time.sleep(3)

    def start(self):
        self._running = True
        self._thread = threading.Thread(target=self._run_loop, daemon=True)
        self._thread.start()
        time.sleep(2)
        return self

    def stop(self):
        self._running = False
        if self.ws:
            try:
                self.ws.close()
            except Exception:
                pass

    def candles(self, symbol: str, timeframe: str = "1m", n: int = 100) -> List[Candle]:
        if symbol not in self.builders:
            return []
        return self.builders[symbol].get_candles(timeframe, n)

    def price(self, symbol: str) -> Optional[float]:
        return self._price_cache.get(symbol)

    def prices(self) -> Dict[str, float]:
        return dict(self._price_cache)


if __name__ == "__main__":
    print("Testing CandleEngine...")
    engine = CandleEngine(["btcusdt", "ethusdt"]).start()
    time.sleep(10)

    for sym in ["btcusdt", "ethusdt"]:
        for tf in ["5s", "30s", "1m", "5m"]:
            cs = engine.candles(sym, tf, 5)
            if cs:
                print(f"{sym} {tf}: {len(cs)} candles, latest close={cs[-1].close:.4f}")

    engine.stop()
