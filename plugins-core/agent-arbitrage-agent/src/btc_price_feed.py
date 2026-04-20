#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Fast Data Feed — Binance + Binance WebSocket for real-time BTC analysis.

Binance: free API, no auth needed, reliable BTC/USDT price + klines
Binance WebSocket: real-time trade stream for live tick updates

Price history stored in rolling window for indicator calculation.
"""

import json
import threading
import time
from collections import deque
from dataclasses import dataclass
from typing import Optional

import requests


@dataclass
class PriceTick:
    price: float
    timestamp: float
    source: str  # "binance_rest", "binance_ws"


BINANCE_REST = "https://api.binance.com"
BINANCE_WS = "wss://stream.binance.com:9443/ws"


def warmup_klines(n: int = 50) -> list[dict]:
    """Fetch last N 1m klines from Binance — gives instant indicator warmup."""
    try:
        r = requests.get(
            f"{BINANCE_REST}/api/v3/klines",
            params={"symbol": "BTCUSDT", "interval": "1m", "limit": n},
            timeout=10,
        )
        if r.status_code == 200:
            data = r.json()
            result = []
            for k in data:
                result.append(
                    {
                        "ts": k[0] / 1000.0,
                        "open": float(k[1]),
                        "high": float(k[2]),
                        "low": float(k[3]),
                        "close": float(k[4]),
                        "volume": float(k[5]),
                    }
                )
            return result
    except Exception:
        pass
    return []


class BTCPriceFeed:
    """
    Rolling price history with indicator-ready data.
    - Binance REST klines for warmup (50 candles instantly)
    - Binance WebSocket trade stream for real-time updates
    - Rolling window: last 200 ticks
    - Auto-builds OHLCV candles at any interval
    """

    def __init__(self, ws_interval: int = 5):
        self.ws_interval = ws_interval
        self.prices: deque[PriceTick] = deque(maxlen=200)
        self._running = False
        self._lock = threading.Lock()
        self._ws_thread: Optional[threading.Thread] = None
        self._latest_price: Optional[float] = None
        self._latest_ts: float = 0.0

    def start(self):
        klines = warmup_klines(50)
        with self._lock:
            for k in klines:
                self.prices.append(PriceTick(k["close"], k["ts"], "binance_rest"))
            if klines:
                self._latest_price = klines[-1]["close"]
                self._latest_ts = klines[-1]["ts"]

        self._running = True
        self._ws_thread = threading.Thread(target=self._ws_loop, daemon=True)
        self._ws_thread.start()
        return self

    def stop(self):
        self._running = False
        if self._ws_thread:
            self._ws_thread.join(timeout=5)

    def _ws_loop(self):
        import websocket

        ws_url = f"{BINANCE_WS}/btcusdt@trade"

        while self._running:
            try:

                def on_message(ws, message):
                    try:
                        d = json.loads(message)
                        if d.get("e") != "trade":
                            return
                        price = float(d["p"])
                        ts = d["T"] / 1000.0
                        with self._lock:
                            self.prices.append(PriceTick(price, ts, "binance_ws"))
                            self._latest_price = price
                            self._latest_ts = ts
                    except Exception:
                        pass

                ws = websocket.WebSocketApp(
                    ws_url,
                    on_message=on_message,
                    on_error=lambda *args: None,
                    on_close=lambda *args: None,
                )
                ws.run_forever(ping_interval=30, ping_timeout=10)
            except Exception:
                pass
            if self._running:
                time.sleep(2)

    def latest(self) -> tuple[Optional[float], float]:
        with self._lock:
            if not self.prices:
                return None, 0.0
            return self._latest_price, self._latest_ts

    def closes(self, n: int = 200) -> list[float]:
        with self._lock:
            ticks = list(self.prices)[-n:]
            return [t.price for t in ticks]

    def candles(self, interval_seconds: int, n: int = 200) -> list[dict]:
        """
        Build OHLCV candles from price ticks at any interval.
        Returns list of {open, high, low, close, volume, ts} dicts.
        """
        with self._lock:
            ticks = list(self.prices)

        if not ticks:
            return []

        result = []
        current_bucket_ts = None
        bucket = None

        for tick in ticks:
            bucket_ts = int(tick.timestamp / interval_seconds) * interval_seconds

            if bucket_ts != current_bucket_ts:
                if bucket:
                    result.append(bucket)
                current_bucket_ts = bucket_ts
                bucket = {
                    "ts": bucket_ts,
                    "open": tick.price,
                    "high": tick.price,
                    "low": tick.price,
                    "close": tick.price,
                    "volume": 1.0,
                }
            else:
                bucket["high"] = max(bucket["high"], tick.price)
                bucket["low"] = min(bucket["low"], tick.price)
                bucket["close"] = tick.price
                bucket["volume"] += 1.0

        if bucket:
            result.append(bucket)

        return result[-n:]


if __name__ == "__main__":
    print("=== BTC Price Feed Test (Binance) ===")
    feed = BTCPriceFeed().start()

    for i in range(8):
        time.sleep(5)
        price, ts = feed.latest()
        closes = feed.closes(20)
        c5m = feed.candles(300, 10)
        print(
            f"[{i + 1}] BTC: ${price:,.0f} | {len(closes)} prices | {len(c5m)} 5m candles"
        )
        if c5m:
            c = c5m[-1]
            print(
                f"  5m: O={c['open']:.2f} H={c['high']:.2f} L={c['low']:.2f} C={c['close']:.2f}"
            )

    feed.stop()
