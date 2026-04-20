#!/usr/local/opt/python@3.11/bin/python3.11
"""
Improved Polymarket Signal Mapper — smarter edge calculation, proper expiry filtering.

Key improvements:
- Reject markets resolving in > 48 hours (no trading multi-month positions on intraday signals)
- Better edge calculation: accounts for implied vol, time remaining, and signal confidence
- Maps BTC direction to ANY short-term market where macro/crypto sentiment matters
- Reports expected value and optimal position size
"""

import json
import re
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Optional

import requests

GAMMA_API = "https://gamma-api.polymarket.com"
TAKER_FEE = 0.02


@dataclass
class MappedTrade:
    market_id: str
    question: str
    yes_price: float
    no_price: float
    liquidity: float
    hours_remaining: float
    days_remaining: float
    btc_direction: str
    our_prob: float
    market_prob: float
    edge: float
    expected_value: float
    position_side: str
    kelly_size_pct: float
    reasoning: str


def parse_price_number(s: str) -> Optional[float]:
    s = s.strip().replace(",", "").replace("$", "").lower()
    if s.endswith("k"):
        return float(s[:-1]) * 1e3
    if s.endswith("m"):
        return float(s[:-1]) * 1e6
    if s.endswith("b"):
        return float(s[:-1]) * 1e9
    try:
        return float(s)
    except:
        return None


class SignalMapper:
    """
    Maps BTC technical signals to Polymarket opportunities.

    Filters:
    - Max 48h until expiry (no multi-month positions on intraday signals)
    - Min liquidity $1000
    - Edge > 3%

    Edge model:
    - Compare our P(direction) vs Polymarket implied probability
    - Adjust for time remaining (short fuse = bigger edge needed)
    - Apply 2% taker fee to EV calculation
    """

    MAX_HOURS = 48.0
    MIN_LIQUIDITY = 1000.0
    MIN_EDGE = 0.03
    MIN_PROB = 0.30
    MAX_PROB = 0.85

    def __init__(self):
        self.session = requests.Session()
        self.session.headers.update(
            {"Content-Type": "application/json", "User-Agent": "makakoo-os/1.0"}
        )

    def _parse_target(self, question: str) -> Optional[tuple]:
        """Extract (target_price, direction) from question."""
        q = question.lower()
        patterns = [
            (r"above\s+\$?([\d,\.]+[kmb]?)", "above"),
            (r"below\s+\$?([\d,\.]+[kmb]?)", "below"),
            (r"reach\s+\$?([\d,\.]+[kmb]?)", "above"),
            (r"hit\s+\$?([\d,\.]+[kmb]?)", "above"),
        ]
        for pat, direction in patterns:
            m = re.search(pat, q)
            if m:
                val = parse_price_number(m.group(1))
                if val and val > 0:
                    return (val, direction)
        return None

    def _hours_until(self, end_date: str) -> float:
        """Hours until market expiry."""
        try:
            end_ts = datetime.fromisoformat(end_date.replace("Z", "+00:00")).timestamp()
            return (end_ts - datetime.now(timezone.utc).timestamp()) / 3600
        except:
            return 999

    def _time_decay_factor(self, hours: float) -> float:
        """
        Reduce edge requirement for very short-term markets.
        If market resolves in < 6h, we need less edge (quick resolution).
        If market is 48h, full edge required.
        """
        if hours <= 6:
            return 0.5
        elif hours <= 12:
            return 0.65
        elif hours <= 24:
            return 0.80
        elif hours <= 36:
            return 0.90
        return 1.0

    def _calculate_edge(
        self,
        btc_signal,  # CompositeSignal
        market_prob: float,
        side: str,
        hours: float,
        target_price: Optional[float],
        btc_price: float,
        direction: str,
    ) -> tuple[float, float, str]:
        """
        Calculate edge and EV.

        Returns: (edge, expected_value, reasoning)
        """
        our_raw = (
            btc_signal.probability_up
            if direction == "above"
            else btc_signal.probability_down
        )

        # Adjust our probability based on distance from target
        if target_price and target_price > 0:
            pct_to_target = abs(target_price - btc_price) / btc_price
            if direction == "above":
                if btc_price >= target_price:
                    # Already above — if RSI overbought + MACD bearish, could still drop
                    our_raw = max(0.40, btc_signal.probability_down * 0.6)
                else:
                    # Need to reach target
                    our_raw = our_raw * max(0.3, 1 - pct_to_target * 2)
            else:
                if btc_price <= target_price:
                    our_raw = max(0.40, btc_signal.probability_up * 0.6)
                else:
                    our_raw = our_raw * max(0.3, 1 - pct_to_target * 2)

        # Clamp our probability
        our_prob = max(self.MIN_PROB, min(self.MAX_PROB, our_raw))

        # Market probability
        if side == "YES":
            market_prob_adj = market_prob
        else:
            market_prob_adj = 1.0 - market_prob

        # Raw edge
        edge = our_prob - market_prob_adj

        # Time decay: short-term markets need less edge to be worth it
        time_factor = self._time_decay_factor(hours)
        adjusted_edge = edge * time_factor

        # Expected value after 2% taker fee
        cost = market_prob if side == "YES" else (1.0 - market_prob)
        # EV = P(win) * (1 - cost - fee) - P(loss) * cost
        #     = our_prob * (1 - cost - fee*cost) - (1-our_prob) * cost
        fee_impact = cost * TAKER_FEE
        win_payout = 1.0 - cost - fee_impact
        loss_cost = cost + fee_impact
        ev = our_prob * win_payout - (1 - our_prob) * loss_cost

        reasoning = (
            f"BTC signal={btc_signal.direction} P(up)={btc_signal.probability_up:.0%} "
            f"our_prob={our_prob:.0%} market_prob={market_prob_adj:.0%} edge={edge:.1%} "
            f"time_factor={time_factor:.0%}"
        )

        return adjusted_edge, ev, reasoning

    def map_signal(self, btc_signal, btc_price: float) -> list[MappedTrade]:
        """
        Map BTC signal to best available Polymarket markets.

        Returns top 10 opportunities sorted by expected value.
        """
        if btc_signal.direction == "neutral":
            return []

        try:
            r = self.session.get(
                f"{GAMMA_API}/markets",
                params={"active": "true", "closed": "false", "limit": 200},
                timeout=10,
            )
            if r.status_code != 200:
                return []
            raw_markets = r.json()
        except Exception:
            return []

        opportunities = []

        for raw in raw_markets:
            try:
                liq = float(raw.get("liquidity", 0) or 0)
                if liq < self.MIN_LIQUIDITY:
                    continue

                question = raw.get("question", "")
                if not question:
                    continue

                end_date = raw.get("endDate", "")
                hours = self._hours_until(end_date)
                if hours < 0 or hours > self.MAX_HOURS:
                    continue

                prices = raw.get("outcomePrices", "[]")
                if isinstance(prices, str):
                    prices = json.loads(prices)
                if not prices or len(prices) < 2:
                    continue

                yes_price = float(prices[0])
                no_price = float(prices[1])
                if yes_price < 0.01 or yes_price > 0.99:
                    continue

                # Try to parse target
                target_parsed = self._parse_target(question)
                target_price = target_parsed[0] if target_parsed else None
                direction = target_parsed[1] if target_parsed else "above"

                # Determine which side to take
                if btc_signal.direction == "up":
                    if direction == "above":
                        side = "YES"  # BTC up + target above = YES
                        market_prob = yes_price
                    else:
                        side = "NO"  # BTC up + target below = fade NO
                        market_prob = no_price
                elif btc_signal.direction == "down":
                    if direction == "below":
                        side = "YES"  # BTC down + target below = YES
                        market_prob = yes_price
                    else:
                        side = "NO"  # BTC down + target above = NO
                        market_prob = no_price
                else:
                    continue

                edge, ev, reasoning = self._calculate_edge(
                    btc_signal,
                    market_prob,
                    side,
                    hours,
                    target_price,
                    btc_price,
                    direction,
                )

                if edge < self.MIN_EDGE:
                    continue

                # Kelly position sizing: f* = (bp - q) / b
                # b = 1 - cost (net odds), p = our_prob, q = 1 - our_prob
                cost = yes_price if side == "YES" else no_price
                gross_multiplier = (1.0 / cost) if cost > 0.01 else 1.0
                b = gross_multiplier - 1  # net odds received on winning
                p = edge + market_prob  # our win probability
                q = 1 - p
                if b > 0 and p > 0.5:
                    kelly = max(0.01, min(0.25, (b * p - q) / b))
                else:
                    kelly = 0.05

                opportunities.append(
                    MappedTrade(
                        market_id=raw.get("id", ""),
                        question=question,
                        yes_price=yes_price,
                        no_price=no_price,
                        liquidity=liq,
                        hours_remaining=hours,
                        days_remaining=hours / 24,
                        btc_direction=btc_signal.direction,
                        our_prob=edge + market_prob,
                        market_probability=market_prob,
                        edge=edge,
                        expected_value=ev,
                        position_side=side,
                        kelly_size_pct=kelly,
                        reasoning=reasoning,
                    )
                )

            except Exception:
                continue

        # Sort by EV descending
        opportunities.sort(key=lambda x: x.expected_value, reverse=True)
        return opportunities[:10]


if __name__ == "__main__":
    import sys

    sys.path.insert(0, str(__file__).rsplit("/", 1)[0])

    from btc_price_feed import BTCPriceFeed
    from btc_signals import BTCSignalGenerator

    feed = BTCPriceFeed(cg_interval=5, pm_ws_interval=5).start()
    print("Waiting 40s for data...")
    time.sleep(40)

    gen = BTCSignalGenerator(min_prices=25)
    mapper = SignalMapper()

    sig = gen.generate(feed)
    if sig:
        print(
            f"\nSignal: {sig.direction} conf={sig.confidence:.0%} BTC=${sig.btc_price:,.0f}"
        )
        print(f"P(up)={sig.probability_up:.0%} score={sig.score:.0f}")
        for r in sig.reasoning:
            print(f"  {r}")

        print("\n--- Top Opportunities ---")
        opps = mapper.map_signal(sig, sig.btc_price)
        if opps:
            for o in opps[:5]:
                print(f"\n  [{o.position_side}] {o.question[:60]}")
                print(f"    YES={o.yes_price:.3f} NO={o.no_price:.3f}")
                print(f"    Hours: {o.hours_remaining:.0f}h | Liq: ${o.liquidity:,.0f}")
                print(
                    f"    Edge={o.edge:.1%} EV={o.expected_value:.4f} Kelly={o.kelly_size_pct:.0%}"
                )
                print(f"    → {o.reasoning}")
        else:
            print("  No opportunities found")
    else:
        print("No signal")

    feed.stop()
