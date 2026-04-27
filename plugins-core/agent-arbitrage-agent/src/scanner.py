"""
Market Scanner — fetches real Polymarket orderbook data from CLOB API.
Builds a live picture of liquidity, spreads, and trading opportunities.
"""

import json
import time
import requests
from dataclasses import dataclass, field
from typing import Dict, List, Optional
from datetime import datetime


GAMMA_API = "https://gamma-api.polymarket.com"
CLOB_API = "https://clob.polymarket.com"
REQUEST_TIMEOUT = 10


@dataclass
class OrderBookLevel:
    price: float
    size: float

    @property
    def value(self) -> float:
        return self.price * self.size


@dataclass
class Market:
    id: str
    question: str
    tokens: List[str]  # [yes_token_id, no_token_id]
    yes_price: float
    no_price: float
    spread_bps: float
    spread_pct: float
    mid_price: float
    liquidity: float
    volume_24h: float
    resolved: bool
    market_type: str  # "binary" or "multi"
    n_legs: int = 2
    best_bid_yes: float = 0.0
    best_ask_yes: float = 0.0
    best_bid_no: float = 0.0
    best_ask_no: float = 0.0

    @property
    def is_binary(self) -> bool:
        return self.n_legs == 2

    @property
    def gamma_sum(self) -> float:
        """For multi-leg markets, sum of YES prices. Should = 1.0 at fair value."""
        return self.yes_price * self.n_legs

    @property
    def opportunity_score(self) -> float:
        """Higher = more attractive for market making. Based on spread + volume."""
        spread_value = self.spread_pct * 10000
        vol_score = min(self.volume_24h / 100000, 1.0) * 50
        liq_score = min(self.liquidity / 10000, 1.0) * 30
        spread_score = min(spread_value / 100, 1.0) * 20
        return vol_score + liq_score + spread_score


class Scanner:
    """Scans Polymarket for tradeable markets."""

    def __init__(self, min_volume: float = 1000, min_liquidity: float = 500):
        self.min_volume = min_volume
        self.min_liquidity = min_liquidity
        self.session = requests.Session()
        self.session.headers.update({"Content-Type": "application/json"})
        self._cache: Dict[str, Market] = {}
        self._last_fetch = 0
        self._cache_ttl = 30

    def get_orderbook(self, token_id: str) -> Optional[Dict]:
        """Fetch orderbook for a single token from CLOB."""
        try:
            r = self.session.get(
                f"{CLOB_API}/book",
                params={"token_id": token_id},
                timeout=REQUEST_TIMEOUT,
            )
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return None

    def fetch_binary_markets(self, limit: int = 50) -> List[Market]:
        """Fetch active binary (2-outcome) markets with orderbook data."""
        markets = []
        try:
            r = self.session.get(
                f"{GAMMA_API}/markets",
                params={"active": "true", "closed": "false", "limit": limit},
                timeout=REQUEST_TIMEOUT,
            )
            if r.status_code != 200:
                return markets
            raw = r.json()
        except Exception:
            return markets

        for m in raw:
            try:
                vol = float(m.get("volume", 0) or 0)
                liq = float(m.get("liquidity", 0) or 0)
                if vol < self.min_volume and liq < self.min_liquidity:
                    continue

                tids = m.get("clobTokenIds", [])
                if isinstance(tids, str):
                    tids = json.loads(tids)
                if not tids or len(tids) < 2:
                    continue

                outcomes = m.get("outcomes", [])
                if isinstance(outcomes, str):
                    outcomes = json.loads(outcomes)

                prices = m.get("outcomePrices", [])
                if isinstance(prices, str):
                    prices = json.loads(prices)
                if len(prices) < 2:
                    continue

                yes_price = float(prices[0])
                no_price = float(prices[1])

                if yes_price < 0.01 or yes_price > 0.99:
                    continue

                spread = abs(yes_price - no_price)
                spread_bps = spread * 10000
                mid = (yes_price + no_price) / 2

                market = Market(
                    id=m.get("id", ""),
                    question=m.get("question", ""),
                    tokens=tids[:2],
                    yes_price=yes_price,
                    no_price=no_price,
                    spread_bps=spread_bps,
                    spread_pct=spread,
                    mid_price=mid,
                    liquidity=liq,
                    volume_24h=vol,
                    resolved=bool(m.get("closed", False)),
                    market_type="binary",
                    n_legs=2,
                    best_bid_yes=no_price,
                    best_ask_yes=yes_price,
                    best_bid_no=yes_price,
                    best_ask_no=no_price,
                )
                markets.append(market)

            except Exception:
                continue

        return markets

    def enrich_with_orderbook(self, market: Market) -> Market:
        """Add real CLOB orderbook depth to a market."""
        try:
            yes_book = self.get_orderbook(market.tokens[0])
            no_book = self.get_orderbook(market.tokens[1])

            if yes_book and yes_book.get("bids"):
                market.best_bid_yes = float(yes_book["bids"][0]["price"])
            if yes_book and yes_book.get("asks"):
                market.best_ask_yes = float(yes_book["asks"][0]["price"])
            if no_book and no_book.get("bids"):
                market.best_bid_no = float(no_book["bids"][0]["price"])
            if no_book and no_book.get("asks"):
                market.best_ask_no = float(no_book["asks"][0]["price"])
        except Exception:
            pass
        return market

    def scan(self, use_cache: bool = True) -> List[Market]:
        """Full scan: fetch markets + enrich with orderbook data."""
        now = time.time()
        if use_cache and (now - self._last_fetch) < self._cache_ttl and self._cache:
            return list(self._cache.values())

        markets = self.fetch_binary_markets(limit=100)
        enriched = []
        for m in markets:
            if m.is_binary:
                m = self.enrich_with_orderbook(m)
            enriched.append(m)

        self._cache = {m.id: m for m in enriched}
        self._last_fetch = now
        return enriched

    def get_candidates(self, genome) -> List[Market]:
        """Filter scanned markets against genome parameters."""
        all_markets = self.scan()
        candidates = []
        for m in all_markets:
            if m.resolved:
                continue
            if m.liquidity < genome.min_liquidity:
                continue
            if m.volume_24h < genome.min_volume_usd:
                continue
            if m.yes_price > genome.max_price or m.yes_price < 0.01:
                continue
            if m.n_legs > genome.max_legs:
                continue
            if m.spread_bps < genome.min_spread_bps:
                continue
            candidates.append(m)
        return sorted(candidates, key=lambda x: x.opportunity_score, reverse=True)

    def get_candidates_for_intraday(
        self,
        min_liq: float = 10000,
        min_spread: float = 30,
        min_vol: float = 10000,
    ) -> List[Market]:
        """Simple filter for intraday trading."""
        all_markets = self.scan()
        cands = []
        for m in all_markets:
            if m.resolved:
                continue
            if m.liquidity < min_liq:
                continue
            if m.volume_24h < min_vol:
                continue
            if m.spread_bps < min_spread:
                continue
            if m.yes_price < 0.05 or m.yes_price > 0.95:
                continue
            cands.append(m)
        return sorted(cands, key=lambda x: x.opportunity_score, reverse=True)


def fetch_neg_risk_events() -> List[Dict]:
    """Fetch multi-outcome (NegRisk) events from Gamma API."""
    try:
        r = requests.get(
            f"{GAMMA_API}/events",
            params={"active": "true", "closed": "false", "limit": 50},
            timeout=REQUEST_TIMEOUT,
        )
        if r.status_code == 200:
            raw = r.json()
            events = []
            for ev in raw:
                markets = ev.get("markets", [])
                if len(markets) < 3:
                    continue
                prices = []
                valid = True
                for sm in markets:
                    try:
                        ps = sm.get("outcomePrices", "[]")
                        if isinstance(ps, str):
                            ps = json.loads(ps)
                        if ps:
                            prices.append(float(ps[0]))
                    except Exception:
                        valid = False
                        break
                if not valid or not prices:
                    continue
                gamma_sum = sum(prices)
                events.append(
                    {
                        "id": ev.get("id", ""),
                        "title": ev.get("title", ""),
                        "n_markets": len(markets),
                        "gamma_sum": gamma_sum,
                        "markets": markets,
                    }
                )
            return sorted(events, key=lambda x: x["gamma_sum"], reverse=True)
    except Exception:
        pass
    return []
