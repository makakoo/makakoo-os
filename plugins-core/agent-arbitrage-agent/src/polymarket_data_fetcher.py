#!/usr/local/opt/python@3.11/bin/python3.11
"""
Polymarket Data Fetcher — Gamma API + CLOB prices-history.

Fetches:
- Active market list with metadata (volume, liquidity, prices, end dates)
- Price history for YES tokens (from CLOB prices-history endpoint)
- Computed momentum/volatility features
"""

import json
import time
import requests
from dataclasses import dataclass
from typing import Optional


GAMMA_API = "https://gamma-api.polymarket.com"
CLOB_API = "https://clob.polymarket.com"
REQUEST_TIMEOUT = 10


@dataclass
class PMMarket:
    """Polymarket market with key fields."""

    id: str
    slug: str
    question: str
    yes_token: str
    no_token: str
    yes_price: float
    no_price: float
    mid_price: float
    spread_bps: float
    volume_24h: float
    volume: float
    liquidity: float
    end_date: str
    resolved: bool
    closed: bool

    @property
    def prob_yes(self) -> float:
        return self.yes_price


@dataclass
class PMFeatures:
    """Price-based features for a Polymarket YES token."""

    yes_price: float
    momentum_5m: float  # % change in YES price, last 5min
    momentum_15m: float
    momentum_1h: float
    volatility_1h: float  # std of 1h returns
    volume_24h_change: float  # 24h vol change %
    spread_bps: float
    price_vs_24h_mid: float  # how far from the 24h average


class PolymarketFetcher:
    def __init__(self):
        self.session = requests.Session()
        self.session.headers.update({"Content-Type": "application/json"})
        self._market_cache: dict[str, PMMarket] = {}
        self._price_cache: dict[str, list] = {}
        self._price_cache_time: dict[str, float] = {}
        self._events_cache: list = {}
        self._events_cache_time: float = 0
        self.cache_ttl = 30

    def get_live_events(self, limit: int = 50) -> list:
        """Get active events with their markets from Gamma API."""
        now = time.time()
        if self._events_cache and (now - self._events_cache_time) < self.cache_ttl:
            return self._events_cache
        try:
            r = self.session.get(
                f"{GAMMA_API}/events",
                params={"active": "true", "closed": "false", "limit": limit},
                timeout=REQUEST_TIMEOUT,
            )
            if r.status_code == 200:
                data = r.json()
                if isinstance(data, list):
                    self._events_cache = data
                    self._events_cache_time = now
                    return data
                elif isinstance(data, dict) and "data" in data:
                    self._events_cache = data["data"]
                    self._events_cache_time = now
                    return self._events_cache
        except Exception as e:
            print(f"[PM] Gamma events error: {e}")
        return []

    def get_markets(self, limit: int = 200) -> list[PMMarket]:
        """Get active binary markets from Gamma API."""
        markets = []
        try:
            r = self.session.get(
                f"{GAMMA_API}/markets",
                params={"active": "true", "closed": "false", "limit": limit},
                timeout=REQUEST_TIMEOUT,
            )
            if r.status_code == 200:
                data = r.json()
                raw_markets = data if isinstance(data, list) else data.get("data", [])
                for m in raw_markets:
                    if not m.get("clobTokenIds"):
                        continue
                    try:
                        tokens = json.loads(m["clobTokenIds"])
                    except:
                        continue
                    if len(tokens) < 2:
                        continue
                    try:
                        prices = json.loads(m.get("outcomePrices", "[]"))
                        outcomes = json.loads(m.get("outcomes", "[]"))
                    except:
                        continue
                    if len(prices) < 2:
                        continue

                    yes_price = float(prices[0]) if prices[0] not in (None, "") else 0.0
                    no_price = float(prices[1]) if prices[1] not in (None, "") else 0.0
                    mid = (yes_price + no_price) / 2
                    spread = abs(yes_price - no_price)
                    spread_bps = spread * 10000

                    market = PMMarket(
                        id=m.get("id", m.get("conditionId", "")),
                        slug=m.get("slug", ""),
                        question=m.get("question", ""),
                        yes_token=tokens[0],
                        no_token=tokens[1],
                        yes_price=yes_price,
                        no_price=no_price,
                        mid_price=mid,
                        spread_bps=spread_bps,
                        volume_24h=float(m.get("volume24hr") or 0),
                        volume=float(m.get("volume") or 0),
                        liquidity=float(m.get("liquidity") or 0),
                        end_date=m.get("endDateIso", m.get("endDate", "")),
                        resolved=bool(m.get("closed", False)),
                        closed=bool(m.get("closed", False)),
                    )
                    markets.append(market)
                    self._market_cache[market.id] = market
        except Exception as e:
            print(f"[PM] Gamma markets error: {e}")
        return markets

    def get_top_markets(
        self, min_volume: float = 10000, limit: int = 10
    ) -> list[PMMarket]:
        """Get highest-volume markets suitable for intraday trading."""
        all_markets = self.get_markets(limit=200)
        filtered = [
            m for m in all_markets if m.volume_24h >= min_volume and not m.closed
        ]
        return sorted(filtered, key=lambda m: m.volume_24h, reverse=True)[:limit]

    def get_price_history(
        self, token_id: str, interval: str = "1h", limit: int = 168
    ) -> list:
        """Get YES token price history from CLOB prices-history endpoint."""
        cache_key = f"{token_id}_{interval}_{limit}"
        now = time.time()
        if (
            cache_key in self._price_cache
            and (now - self._price_cache_time.get(cache_key, 0)) < 60
        ):
            return self._price_cache[cache_key]
        try:
            r = self.session.get(
                f"{CLOB_API}/prices-history",
                params={"market": token_id, "interval": interval, "limit": limit},
                timeout=REQUEST_TIMEOUT,
            )
            if r.status_code == 200:
                data = r.json()
                history = data.get("history", [])
                self._price_cache[cache_key] = history
                self._price_cache_time[cache_key] = now
                return history
        except Exception as e:
            print(f"[PM] prices-history error: {e}")
        return []

    def get_midpoint(self, token_id: str) -> Optional[float]:
        """Get current midpoint price for a token from CLOB."""
        try:
            r = self.session.get(
                f"{CLOB_API}/midpoint",
                params={"token_id": token_id},
                timeout=REQUEST_TIMEOUT,
            )
            if r.status_code == 200:
                data = r.json()
                mid = data.get("mid")
                if mid:
                    return float(mid)
        except:
            pass
        return None

    def features(self, yes_token_id: str) -> Optional[PMFeatures]:
        """Compute price-based features for a YES token."""
        history_h = self.get_price_history(yes_token_id, "1h", 168)
        history_5m_raw = self.get_price_history(yes_token_id, "1m", 60)

        if not history_h:
            return None

        # Parse prices
        def parse_prices(hist):
            result = []
            for item in hist:
                if isinstance(item, dict):
                    result.append(float(item.get("p", 0)))
                elif isinstance(item, list):
                    result.append(float(item[1] if len(item) > 1 else item[0]))
                else:
                    result.append(float(item))
            return result

        prices_h = parse_prices(history_h)
        prices_5m = (
            parse_prices(history_5m_raw[-60:]) if history_5m_raw else prices_h[-5:]
        )

        if not prices_h:
            return None

        yes_price = prices_h[-1]

        def mom(prices, n):
            if len(prices) < n + 1:
                return 0.0
            return (prices[-1] - prices[-n - 1]) / max(prices[-n - 1], 0.001) * 100

        def volatility(prices):
            if len(prices) < 3:
                return 0.0
            rets = [
                (prices[i + 1] - prices[i]) / max(prices[i], 0.001)
                for i in range(len(prices) - 1)
            ]
            if len(rets) < 2:
                return 0.0
            mean = sum(rets) / len(rets)
            variance = sum((r - mean) ** 2 for r in rets) / len(rets)
            return variance**0.5 * 100

        vol_1h = volatility(prices_h[-24:]) if len(prices_h) >= 24 else 0.0

        # 24h volume change
        # (can't get historical volume from current API, use price-based proxy)
        price_vs_avg = 0.0
        if len(prices_h) >= 24:
            avg_24h = sum(prices_h[-24:]) / 24
            price_vs_avg = (prices_h[-1] - avg_24h) / max(avg_24h, 0.001) * 100

        # Current spread
        mid = self.get_midpoint(yes_token_id)
        if mid:
            spread_bps = abs(yes_price - mid) * 10000
        else:
            spread_bps = 0.0

        return PMFeatures(
            yes_price=yes_price,
            momentum_5m=mom(prices_5m, 1),
            momentum_15m=mom(prices_5m, 3),
            momentum_1h=mom(prices_h, 1),
            volatility_1h=vol_1h,
            volume_24h_change=0.0,  # not available from this API
            spread_bps=spread_bps,
            price_vs_24h_mid=price_vs_avg,
        )

    def fetch_current_price(self, yes_token_id: str) -> float:
        """Get current YES price, with fallback."""
        mid = self.get_midpoint(yes_token_id)
        if mid:
            return mid
        history = self.get_price_history(yes_token_id, "1m", 5)
        if history:
            item = history[-1]
            if isinstance(item, dict):
                return float(item.get("p", 0.5))
            elif isinstance(item, list):
                return float(item[1])
        return 0.5


if __name__ == "__main__":
    fetcher = PolymarketFetcher()
    markets = fetcher.get_top_markets(min_volume=50000, limit=5)
    print(f"Top markets: {len(markets)}")
    for m in markets:
        print(f"\n  Q: {m.question[:80]}")
        print(
            f"  YES: {m.yes_price:.3f}  NO: {m.no_price:.3f}  spread: {m.spread_bps:.0f}bps"
        )
        print(f"  vol24h: ${m.volume_24h:,.0f}  liq: ${m.liquidity:,.0f}")
        f = fetcher.features(m.yes_token)
        if f:
            print(
                f"  PM momentum — 5m: {f.momentum_5m:+.3f}%  15m: {f.momentum_15m:+.3f}%  1h: {f.momentum_1h:+.3f}%"
            )
            print(
                f"  vol_1h: {f.volatility_1h:.3f}%  vs_24h_avg: {f.price_vs_24h_mid:+.3f}%"
            )
