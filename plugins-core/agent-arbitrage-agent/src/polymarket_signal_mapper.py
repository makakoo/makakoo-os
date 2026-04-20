#!/usr/local/opt/python@3.11/bin/python3.11
"""
Polymarket Signal Mapper — maps BTC technical signals to tradeable Polymarket markets.

The BTC signal gives us:
  - Direction: UP or DOWN
  - Probability: P(up) / P(down) from multi-TF indicators
  - Confidence: 0-100%

We scan available Polymarket markets and find the best match.
For each market we calculate: is the Polymarket price a good entry vs our signal?

Example:
  BTC signal: P(up) = 62%, direction = UP
  Polymarket market: "Will BTC be above $70,000 by Friday?" → YES = 0.55

  Our edge: we think P(up) = 62% but market implies 55% → BUY YES (we have 7% edge)
  Expected value: (0.62 × 1.0 + 0.38 × 0.0) - 0.55 = 0.07 → positive EV trade
"""

import json
import re
import time
from dataclasses import dataclass
from typing import Optional

import requests

from crypto_price_scanner import CryptoPriceScanner, parse_price_number
from btc_signals import CompositeSignal


@dataclass
class MappedTrade:
    """A tradeable Polymarket market mapped to our BTC signal."""

    market_id: str
    question: str
    yes_price: float
    no_price: float
    liquidity: float
    hours_remaining: float
    btc_direction: str
    our_probability: float  # our calculated P(up) or P(down)
    market_probability: float  # what Polymarket implies
    edge: float  # our_probability - market_probability
    expected_value: float  # EV of the trade after fees
    position_side: str  # "YES" or "NO"
    position_size_pct: float  # recommended size (% of capital)
    reasoning: str


class PolymarketSignalMapper:
    """
    Maps BTC technical signals to available Polymarket markets.

    Flow:
    1. Get BTC technical signal (direction, probability)
    2. Scan Polymarket for active markets
    3. Score each market by edge = our_prob - market_prob
    4. Return top candidates with recommended position sizing
    """

    GAMMA_API = "https://gamma-api.polymarket.com"

    def __init__(self, min_edge: float = 0.05, min_liquidity: float = 500):
        self.min_edge = min_edge
        self.min_liquidity = min_liquidity
        self.crypto_scanner = CryptoPriceScanner(
            min_liquidity=min_liquidity, max_hours=200
        )
        self.session = requests.Session()
        self.session.headers.update(
            {"Content-Type": "application/json", "User-Agent": "harvey-os/1.0"}
        )

    def fetch_all_active_markets(self, limit: int = 100) -> list[dict]:
        """Fetch all active Polymarket markets."""
        try:
            r = self.session.get(
                f"{self.GAMMA_API}/markets",
                params={"active": "true", "closed": "false", "limit": limit},
                timeout=10,
            )
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return []

    def _parse_market_target(self, question: str) -> Optional[tuple]:
        """
        Extract (target_price, direction) from any crypto market question.
        direction: "above" or "below"
        """
        q = question.lower()

        def find_pattern(patterns: list[str]) -> Optional[float]:
            for p in patterns:
                m = re.search(p, q)
                if m:
                    val = parse_price_number(m.group(1))
                    if val and val > 0:
                        return val
            return None

        # BTC patterns
        if "bitcoin" in q or " btc " in q or q.startswith("btc ") or q == "btc":
            target = find_pattern(
                [
                    r"above\s+\$?([\d,\.]+[kmb]?)",
                    r"below\s+\$?([\d,\.]+[kmb]?)",
                    r"reach\s+\$?([\d,\.]+[kmb]?)",
                    r"hit\s+\$?([\d,\.]+[kmb]?)",
                    r"close\s+(?:above|below)\s+\$?([\d,\.]+[kmb]?)",
                ]
            )
            if target:
                direction = "above" if "above" in q else "below"
                return (target, direction)

        # ETH patterns
        if (
            "ethereum" in q or " eth " in q or q.startswith("eth ") or " eth/" in q
        ) and "doge" not in q:
            target = find_pattern(
                [
                    r"above\s+\$?([\d,\.]+[kmb]?)",
                    r"below\s+\$?([\d,\.]+[kmb]?)",
                    r"reach\s+\$?([\d,\.]+[kmb]?)",
                ]
            )
            if target:
                direction = "below" if "below" in q else "above"
                return (target, direction)

        return None

    def _calculate_edge(
        self,
        btc_signal,  # CompositeSignal
        market_price: float,
        side: str,
        direction: str,
        target_price: float,
        btc_current: float,
    ) -> Optional[float]:
        """
        Calculate our edge in a market given our BTC signal.

        Args:
            btc_signal: CompositeSignal with probability_up/down
            market_price: Polymarket YES or NO price
            side: "YES" or "NO"
            direction: "above" or "below"
            target_price: the price target in the question
            btc_current: current BTC price

        Returns:
            edge as a float, or None if no edge
        """
        if target_price is None or target_price <= 0:
            return None

        # Distance from current BTC price to target
        if direction == "above":
            pct_to_target = (target_price - btc_current) / btc_current
        else:
            pct_to_target = (btc_current - target_price) / btc_current

        if pct_to_target <= 0:
            # Already at/beyond target
            if direction == "above":
                our_yes_prob = max(0.60, btc_signal.probability_up)
            else:
                our_yes_prob = 1.0 - max(0.60, btc_signal.probability_up)
        else:
            # Need to move to target
            # Estimate probability of reaching target based on our signal
            # If signal says UP, we're more likely to reach "above" targets
            # If signal says DOWN, we're more likely to reach "below" targets
            if direction == "above":
                # Target is ABOVE current price — need upward movement
                our_yes_prob = btc_signal.probability_up * (1.0 - pct_to_target * 2)
                our_yes_prob = max(0.30, min(0.90, our_yes_prob))
            else:
                # Target is BELOW current price — need downward movement
                our_yes_prob = btc_signal.probability_down * (1.0 - pct_to_target * 2)
                our_yes_prob = max(0.10, min(0.70, our_yes_prob))

        if side == "NO":
            our_prob = 1.0 - our_yes_prob
            market_prob = 1.0 - market_price
        else:
            our_prob = our_yes_prob
            market_prob = market_price

        edge = our_prob - market_prob
        return edge

    def map_signal(self, btc_signal, btc_price: float) -> list[MappedTrade]:
        """
        Map a BTC signal to the best available Polymarket markets.

        Returns list of MappedTrade sorted by expected value descending.
        """
        if btc_signal is None:
            return []

        raw_markets = self.fetch_all_active_markets()
        mapped = []

        for raw in raw_markets:
            try:
                liq = float(raw.get("liquidity", 0) or 0)
                if liq < self.min_liquidity:
                    continue

                question = raw.get("question", "")
                if not question:
                    continue

                tids = raw.get("clobTokenIds", [])
                if isinstance(tids, str):
                    tids = json.loads(tids)
                if not tids or len(tids) < 2:
                    continue

                prices = raw.get("outcomePrices", [])
                if isinstance(prices, str):
                    prices = json.loads(prices)
                if len(prices) < 2:
                    continue

                yes_price = float(prices[0])
                no_price = float(prices[1])
                if yes_price < 0.02 or yes_price > 0.98:
                    continue

                # Try to parse target price and direction
                parsed = self._parse_market_target(question)
                if not parsed:
                    continue

                target_price, direction = parsed

                # Resolve time remaining
                end_date = raw.get("endDate", None)
                hours_remaining = 200.0
                if end_date:
                    try:
                        from datetime import datetime, timezone

                        end_ts = datetime.fromisoformat(
                            end_date.replace("Z", "+00:00")
                        ).timestamp()
                        hours_remaining = (end_ts - time.time()) / 3600
                        if hours_remaining < 0:
                            continue
                    except Exception:
                        pass

                # Determine which side to take based on our signal
                if btc_signal.direction == "up":
                    if direction == "above":
                        # BTC going up + target above = BUY YES
                        side = "YES"
                        market_price = yes_price
                    else:
                        # BTC going up + target below = FADE (buy NO, bet against)
                        side = "NO"
                        market_price = no_price
                elif btc_signal.direction == "down":
                    if direction == "below":
                        # BTC going down + target below = BUY YES (it will be below)
                        side = "YES"
                        market_price = yes_price
                    else:
                        # BTC going down + target above = FADE (buy NO)
                        side = "NO"
                        market_price = no_price
                else:
                    continue

                edge = self._calculate_edge(
                    btc_signal, market_price, side, direction, target_price, btc_price
                )
                if edge is None or edge < self.min_edge:
                    continue

                # Expected value after 2% taker fee
                fee_rate = 0.02
                if side == "YES":
                    # Cost to buy YES: price per share
                    cost = market_price
                    # If win: receive $1, net = 1 - cost - fee
                    # If loss: lose cost + fee
                    ev = edge * (1.0 - cost - cost * fee_rate) + (
                        -cost * (1 - edge) * fee_rate
                    )
                    # Simplified: EV = edge - fee
                    ev = edge - cost * fee_rate
                else:
                    # NO side
                    cost = market_price
                    ev = edge - cost * fee_rate

                # Position sizing: more edge = bigger position (kelly fraction)
                position_size_pct = min(
                    0.20, max(0.02, abs(edge) * btc_signal.confidence)
                )

                mapped.append(
                    MappedTrade(
                        market_id=raw.get("id", ""),
                        question=question,
                        yes_price=yes_price,
                        no_price=no_price,
                        liquidity=liq,
                        hours_remaining=hours_remaining,
                        btc_direction=btc_signal.direction,
                        our_probability=edge + market_price,
                        market_probability=market_price,
                        edge=edge,
                        expected_value=ev,
                        position_side=side,
                        position_size_pct=position_size_pct,
                        reasoning=f"BTC signal={btc_signal.direction} P(up)={btc_signal.probability_up:.0%} | market={side}@{market_price:.3f} edge={edge:.1%}",
                    )
                )

            except Exception:
                continue

        # Sort by edge descending
        mapped.sort(key=lambda x: x.edge, reverse=True)
        return mapped[:10]  # top 10 opportunities


if __name__ == "__main__":
    import sys

    sys.path.insert(0, str(__file__).rsplit("/", 1)[0])

    from btc_signals import BTCSignalGenerator
    from candle_engine import CandleEngine

    print("=== Live BTC Signal → Polymarket Mapper Test ===")

    engine = CandleEngine(["btcusdt"]).start()
    print("Waiting for candles...")
    time.sleep(15)

    gen = BTCSignalGenerator(min_candles=30)
    mapper = PolymarketSignalMapper(min_edge=0.03, min_liquidity=1000)

    sig = gen.generate(engine, "btcusdt")
    if sig:
        print(f"\nBTC Signal: {sig.direction} | Confidence: {sig.confidence:.0%}")
        print(
            f"BTC: ${sig.btc_price:,.0f} | P(up)={sig.probability_up:.0%} | Score={sig.score:.0f}"
        )
        for r in sig.reasoning:
            print(f"  {r}")

        print("\n--- Polymarket Opportunities ---")
        opportunities = mapper.map_signal(sig, sig.btc_price)
        if opportunities:
            for opp in opportunities[:5]:
                print(f"\n  [{opp.position_side}] {opp.question[:60]}")
                print(f"    Price: YES={opp.yes_price:.3f} NO={opp.no_price:.3f}")
                print(
                    f"    Edge: {opp.edge:.1%} | EV: {opp.expected_value:.4f} | Size: {opp.position_size_pct:.0%}"
                )
                print(
                    f"    Hours left: {opp.hours_remaining:.0f}h | Liq: ${opp.liquidity:,.0f}"
                )
                print(f"    → {opp.reasoning}")
        else:
            print("  No opportunities with edge > 3%")
    else:
        print("No signal yet — need more candle data")

    engine.stop()
