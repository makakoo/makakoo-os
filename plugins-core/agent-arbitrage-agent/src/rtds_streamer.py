#!/usr/bin/env python3
"""
RTDS WebSocket Streamer — connects to Polymarket RTDS and streams live crypto prices.
Prices are stored in a shared dict for use by the trading engine.
"""

import json, time, threading
import websocket


class RTDSStreamer:
    """Streams live BTC/ETH/SOL prices from Polymarket RTDS WebSocket."""

    def __init__(self, symbols=None):
        self.symbols = symbols or ["btcusdt", "ethusdt", "solusdt", "xrpusdt"]
        self.prices = {}  # symbol -> latest price
        self.timestamps = {}  # symbol -> timestamp
        self.history = {}  # symbol -> [(ts, price), ...]
        self.running = False
        self.ws = None
        self.thread = None
        self.MAX_HISTORY = 1000

    def _on_message(self, ws, message):
        try:
            data = json.loads(message)
            if data.get("topic") != "crypto_prices":
                return
            payload = data.get("payload", {})
            sym = payload.get("symbol", "")
            val = payload.get("value")
            ts = payload.get("timestamp")
            if val is None or ts is None:
                return
            self.prices[sym] = float(val)
            self.timestamps[sym] = int(ts)

            hist = self.history.setdefault(sym, [])
            hist.append((int(ts), float(val)))
            if len(hist) > self.MAX_HISTORY:
                self.history[sym] = hist[-self.MAX_HISTORY :]
        except Exception:
            pass

    def _on_error(self, ws, error):
        pass

    def _on_close(self, ws, *args):
        self.running = False

    def _on_open(self, ws):
        msg = {
            "action": "subscribe",
            "subscriptions": [{"topic": "crypto_prices", "type": "update"}],
        }
        ws.send(json.dumps(msg))

    def start(self):
        self.running = True
        self.ws = websocket.WebSocketApp(
            "wss://ws-live-data.polymarket.com",
            on_message=self._on_message,
            on_error=self._on_error,
            on_close=self._on_close,
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

    def latest(self, symbol):
        """Return (price, timestamp_ms) for a symbol, or (None, None)."""
        return self.prices.get(symbol), self.timestamps.get(symbol)

    def history_n(self, symbol, n=60):
        """Return last n price points for a symbol."""
        h = self.history.get(symbol, [])
        return h[-n:]

    def momentum(self, symbol, window=5):
        """
        Return (current_price, momentum_pct, short_term_change_pct).
        momentum_pct = % deviation from 60-tick average (trend direction).
        short_term_change_pct = % change over last `window` ticks.
        """
        h = self.history.get(symbol, [])
        if len(h) < window + 1:
            return None, 0.0, 0.0
        current = h[-1][1]
        avg = sum(p for _, p in h[-60:]) / min(len(h[-60:]), 60)
        recent = sum(p for _, p in h[-window:]) / min(len(h[-window:]), window)
        mom = (current - avg) / max(avg, 0.001) * 100
        stc = (current - h[-(window + 1)][1]) / max(h[-(window + 1)][1], 0.001) * 100
        return current, round(mom, 4), round(stc, 4)

    def run_forever(self, poll_seconds=1):
        """Run the streamer and poll loop. Call from main thread."""
        self.start()
        try:
            while self.running:
                time.sleep(poll_seconds)
        except KeyboardInterrupt:
            self.stop()


if __name__ == "__main__":
    streamer = RTDSStreamer()
    streamer.run_forever()
