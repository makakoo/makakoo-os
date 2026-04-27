#!/usr/local/opt/python@3.11/bin/python3.11
"""
Crypto Price Scanner —专门扫描 Polymarket 上的短期加密货币价格预测市场。

Finds Polymarket binary markets about BTC/ETH/SOL price action.
Keywords: "Will Bitcoin be above $X", "Will BTC close above $Y", etc.
Short-term = resolves within 48 hours.
"""

import json
import re
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Optional

import requests

GAMMA_API = "https://gamma-api.polymarket.com"
REQUEST_TIMEOUT = 10


def parse_price_number(s: str) -> Optional[float]:
    """Parse '76,000' '1m' '$1.5b' '500' into a float."""
    s = s.strip().replace(",", "").replace("$", "").lower()
    if s.endswith("k"):
        return float(s[:-1]) * 1e3
    if s.endswith("m"):
        return float(s[:-1]) * 1e6
    if s.endswith("b"):
        return float(s[:-1]) * 1e9
    try:
        return float(s)
    except ValueError:
        return None


@dataclass
class CryptoMarket:
    """A Polymarket binary market about crypto price action."""

    id: str
    question: str
    tokens: list[str]  # [yes_token_id, no_token_id]
    yes_price: float
    no_price: float
    spread_bps: float
    mid_price: float
    liquidity: float
    volume_24h: float
    resolved: bool
    end_date: Optional[str] = None
    hours_until_end: Optional[float] = None
    crypto_type: str = "BTC"  # BTC, ETH, SOL, DOGE, OTHER
    target_price: Optional[float] = None  # e.g. 76000.0
    direction: Optional[str] = None  # "above" or "below"


def get_btc_price() -> Optional[float]:
    """Fetch current BTC/USD price from CoinGecko (free, no API key)."""
    try:
        r = requests.get(
            "https://api.coingecko.com/api/v3/simple/price",
            params={"ids": "bitcoin", "vs_currencies": "usd"},
            timeout=10,
        )
        if r.status_code == 200:
            data = r.json()
            return float(data["bitcoin"]["usd"])
    except Exception:
        pass
    return None


def parse_crypto_market(
    question: str, yes_price: float, no_price: float
) -> Optional[tuple]:
    """
    Parse a Polymarket question to check if it's a crypto price prediction market.
    Returns (crypto_type, target_price, direction) or None.

    Handles: "above $76,000", "below $66k", "hit $1m", "reach $1.5b"
    """
    q = question.lower()

    def find_target(patterns: list[str]) -> Optional[float]:
        for p in patterns:
            m = re.search(p, q)
            if m:
                val = parse_price_number(m.group(1))
                if val and val > 0:
                    return val
        return None

    # Bitcoin patterns
    if "bitcoin" in q or " btc " in q or q.startswith("btc ") or q == "btc":
        crypto = "BTC"
        target = find_target(
            [
                r"above\s+\$?([\d,\.]+[kmb]?)",
                r"below\s+\$?([\d,\.]+[kmb]?)",
                r"reach\s+\$?([\d,\.]+[kmb]?)",
                r"hit\s+\$?([\d,\.]+[kmb]?)",
                r"close\s+(?:above|below)\s+\$?([\d,\.]+[kmb]?)",
                r"\$?([\d,\.]+[kmb]?)\s+(?:before|by|on)\s+",
            ]
        )
        if target:
            direction = (
                "above"
                if "above" in q
                or ("reach" in q and "above" not in q and "below" not in q)
                else ("below" if "below" in q else "above")
            )
            return (crypto, target, direction)
        if "up or down" in q:
            return (crypto, None, "direction")

    # Ethereum patterns
    if (
        "ethereum" in q or " eth " in q or q.startswith("eth ") or " eth/" in q
    ) and "doge" not in q:
        crypto = "ETH"
        between_m = re.search(r"\$?([\d,\.]+[kmb]?)\s+and\s+\$?([\d,\.]+[kmb]?)", q)
        if between_m:
            low = parse_price_number(between_m.group(1))
            high = parse_price_number(between_m.group(2))
            if low and high:
                return (crypto, (low + high) / 2, "between")
        target = find_target(
            [
                r"above\s+\$?([\d,\.]+[kmb]?)",
                r"below\s+\$?([\d,\.]+[kmb]?)",
                r"reach\s+\$?([\d,\.]+[kmb]?)",
                r"hit\s+\$?([\d,\.]+[kmb]?)",
            ]
        )
        if target:
            direction = "below" if "below" in q else "above"
            return (crypto, target, direction)

    # Dogecoin / DOGE patterns
    if "dogecoin" in q or "doge" in q or " doge" in q:
        crypto = "DOGE"
        target = find_target(
            [
                r"above\s+\$?([\d,\.]+[kmb]?)",
                r"below\s+\$?([\d,\.]+[kmb]?)",
                r"reach\s+\$?([\d,\.]+[kmb]?)",
            ]
        )
        if target:
            return (crypto, target, "above" if "above" in q else "below")
        if "up or down" in q:
            return (crypto, None, "direction")

    return None


class CryptoPriceScanner:
    """
    Scans Polymarket for short-term crypto price prediction markets.

    Filters for:
    - BTC, ETH, SOL, DOGE price markets
    - Ending within 48 hours (intraday / short-term)
    - Minimum liquidity $500
    - Not yet resolved

    Usage:
        scanner = CryptoPriceScanner()
        markets = scanner.scan()
        for m in markets:
            print(f"{m.crypto_type} {m.direction} ${m.target_price} by {m.end_date[:10]}")
    """

    def __init__(
        self,
        min_liquidity: float = 500,
        max_hours: float = 200,
        poll_interval: float = 60,
    ):
        self.min_liquidity = min_liquidity
        self.max_hours = max_hours
        self.poll_interval = poll_interval
        self.session = requests.Session()
        self.session.headers.update(
            {"Content-Type": "application/json", "User-Agent": "makakoo-os/1.0"}
        )
        self._cache: dict[str, CryptoMarket] = {}
        self._last_fetch = 0.0
        self._btc_price: Optional[float] = None
        self._last_btc_fetch = 0.0

    def fetch_btc_price(self) -> Optional[float]:
        """Fetch current BTC price, cached for 30 seconds."""
        now = time.time()
        if self._btc_price is not None and (now - self._last_btc_fetch) < 30:
            return self._btc_price
        price = get_btc_price()
        if price:
            self._btc_price = price
            self._last_btc_fetch = now
        return price

    def _parse_market(self, raw_m: dict) -> Optional[CryptoMarket]:
        """Parse a raw Gamma API market dict into a CryptoMarket."""
        try:
            vol = float(raw_m.get("volume", 0) or 0)
            liq = float(raw_m.get("liquidity", 0) or 0)
            if liq < self.min_liquidity:
                return None

            tids = raw_m.get("clobTokenIds", [])
            if isinstance(tids, str):
                tids = json.loads(tids)
            if not tids or len(tids) < 2:
                return None

            prices = raw_m.get("outcomePrices", [])
            if isinstance(prices, str):
                prices = json.loads(prices)
            if len(prices) < 2:
                return None

            yes_price = float(prices[0])
            no_price = float(prices[1])
            if yes_price < 0.01 or yes_price > 0.99:
                return None

            question = raw_m.get("question", "")
            parsed = parse_crypto_market(question, yes_price, no_price)
            if not parsed:
                return None

            crypto_type, target_price, direction = parsed

            end_date = raw_m.get("endDate", None)
            hours_until_end = None
            if end_date:
                try:
                    end_ts = datetime.fromisoformat(
                        end_date.replace("Z", "+00:00")
                    ).timestamp()
                    now_ts = datetime.now(timezone.utc).timestamp()
                    hours_until_end = (end_ts - now_ts) / 3600
                    if hours_until_end < 0:
                        return None
                except Exception:
                    pass

            if hours_until_end is not None and hours_until_end > self.max_hours:
                return None

            spread = abs(yes_price - no_price)
            mid = (yes_price + no_price) / 2

            return CryptoMarket(
                id=raw_m.get("id", ""),
                question=question,
                tokens=tids[:2],
                yes_price=yes_price,
                no_price=no_price,
                spread_bps=spread * 10000,
                mid_price=mid,
                liquidity=liq,
                volume_24h=vol,
                resolved=bool(raw_m.get("closed", False)),
                end_date=end_date,
                hours_until_end=hours_until_end,
                crypto_type=crypto_type,
                target_price=target_price,
                direction=direction,
            )
        except Exception:
            return None

    def fetch_all_crypto_markets(self, limit: int = 100) -> list[CryptoMarket]:
        """Fetch all markets and filter for crypto price prediction."""
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

        for raw_m in raw:
            m = self._parse_market(raw_m)
            if m and not m.resolved:
                markets.append(m)

        return markets

    def scan(self, use_cache: bool = True) -> list[CryptoMarket]:
        """Full scan with caching."""
        now = time.time()
        if use_cache and (now - self._last_fetch) < self.poll_interval and self._cache:
            return list(self._cache.values())

        markets = self.fetch_all_crypto_markets()
        self._cache = {m.id: m for m in markets}
        self._last_fetch = now
        return markets

    def get_btc_market(self) -> Optional[CryptoMarket]:
        """Get the first active BTC short-term market."""
        markets = self.scan()
        btc = [m for m in markets if m.crypto_type == "BTC" and not m.resolved]
        return btc[0] if btc else None

    def get_eth_market(self) -> Optional[CryptoMarket]:
        """Get the first active ETH short-term market."""
        markets = self.scan()
        eth = [m for m in markets if m.crypto_type == "ETH" and not m.resolved]
        return eth[0] if eth else None

    def status(self) -> dict:
        """Return scanner status including current BTC price."""
        markets = self.scan()
        btc_price = self.fetch_btc_price()
        btc_markets = [m for m in markets if m.crypto_type == "BTC"]
        eth_markets = [m for m in markets if m.crypto_type == "ETH"]
        short_markets = [
            m for m in markets if m.hours_until_end and m.hours_until_end <= 24
        ]

        return {
            "total_crypto_markets": len(markets),
            "btc_markets": len(btc_markets),
            "eth_markets": len(eth_markets),
            "intraday_markets": len(short_markets),
            "btc_price": btc_price,
            "cache_age_sec": time.time() - self._last_fetch
            if self._last_fetch
            else None,
        }


if __name__ == "__main__":
    scanner = CryptoPriceScanner(min_liquidity=500, max_hours=200)
    status = scanner.status()
    print(f"=== Crypto Price Scanner Status ===")
    print(f"BTC price: ${status['btc_price']}")
    print(f"Total crypto markets: {status['total_crypto_markets']}")
    print(f"  BTC markets: {status['btc_markets']}")
    print(f"  ETH markets: {status['eth_markets']}")
    print(f"  Intraday (<24h): {status['intraday_markets']}")
    print()

    markets = scanner.scan()
    print(f"=== Crypto Markets ({len(markets)}) ===")
    for m in sorted(markets, key=lambda x: x.hours_until_end or 999):
        h = f"{m.hours_until_end:.0f}h" if m.hours_until_end else "?"
        print(
            f"  [{m.crypto_type}] {h} | {m.yes_price:.3f}/{m.no_price:.3f} | "
            f"${m.liquidity:.0f} liq | {m.question[:60]}"
        )
        if m.target_price:
            print(f"         → {m.direction} ${m.target_price:,.0f}")
