#!/usr/local/opt/python@3.11/bin/python3.11
"""
Crypto Signals — Polymarket BTC/ETH price-target market signal logic.

Core insight: Polymarket BTC markets are phrased as:
  "Will the price of Bitcoin be above $76,000 on April 2?"
  → current_btc_price vs target_price is the ONLY signal that matters

Signal logic:
  - "above $X" market: if btc_price > X → YES is more likely
  - "below $X" market: if btc_price < X → NO is more likely
  - Implied probability check: if market implies 40% but BTC is at $74k/$76k = 97%, buy YES
  - Momentum confirmation: helps filter false breakouts
  - Time remaining: more time = more opportunity for BTC to move

 NEVER trade based on momentum alone on a price-target market.
"""

import time
from typing import Optional
from dataclasses import dataclass

from crypto_price_scanner import CryptoMarket, CryptoPriceScanner


@dataclass
class TradingSignal:
    side: str  # "YES" or "NO"
    confidence: float  # 0.0–1.0
    reasoning: str
    btc_price: float
    target_price: float
    market_price: float  # the YES price at signal time
    premium: float  # how much above implied probability we think it should be


def btc_price_signal(market: CryptoMarket, btc_price: float) -> Optional[TradingSignal]:
    """
    Generate a trade signal for a BTC price-target market.

    The market question tells us the target. The current BTC price tells us
    where we are NOW. The gap is our edge.

    Args:
        market: CryptoMarket from CryptoPriceScanner
        btc_price: current BTC/USD price from CoinGecko

    Returns:
        TradingSignal or None (no signal / skip)
    """
    if btc_price is None:
        return None

    if market.crypto_type != "BTC":
        return None

    target = market.target_price
    if target is None:
        return None

    current = btc_price
    yes_p = market.yes_price
    no_p = market.no_price

    if market.direction not in ("above", "below"):
        return None

    if market.direction == "above":
        # "Will BTC be above $X?"
        # If BTC already above X, YES is more likely
        distance_pct = (current - target) / target  # e.g. 0.05 = 5% above target
        implied_prob = yes_p  # market implies ~yes_p probability of happening

        if current >= target:
            # BTC already above target
            # But market isn't priced at 1.0 → there's still time risk
            # Consider buying YES if price is reasonable
            if yes_p < 0.70:
                # Market doesn't fully believe it — BTC is already there!
                # Buy YES with confidence proportional to how far above we are
                confidence = min(0.95, 0.50 + distance_pct * 5)
                premium = (1.0 - implied_prob) * 0.5  # how much we're gaining
                return TradingSignal(
                    side="YES",
                    confidence=confidence,
                    reasoning=f"BTC ${current:.0f} already above ${target:.0f} target — market only implies {implied_prob:.0%}",
                    btc_price=current,
                    target_price=target,
                    market_price=yes_p,
                    premium=premium,
                )
            elif yes_p > 0.90:
                # Too expensive, skip
                return None
        else:
            # BTC below target — need to rally
            distance_pct = (target - current) / target  # e.g. 0.12 = 12% below
            # How likely is BTC to reach target before expiry?
            # Use a simple model: 12% in remaining hours
            hours_left = market.hours_until_end or 24
            required_rally = distance_pct

            # Rough probability model: BTC avg daily vol ~3-5%
            # Probability ~ (daily_vol * hours_left / 24) / required_rally, capped
            daily_vol = 0.04  # 4% daily vol
            prob_reach = (
                min(0.90, (daily_vol * hours_left / 24) / required_rally)
                if required_rally > 0
                else 0.99
            )

            if yes_p < prob_reach - 0.10:
                # Market underestimates — YES is cheap relative to our model
                confidence = min(0.80, (prob_reach - yes_p) * 2)
                return TradingSignal(
                    side="YES",
                    confidence=confidence,
                    reasoning=f"BTC ${current:.0f} needs +{distance_pct:.1%} to reach ${target:.0f} in {hours_left:.0f}h — market implies {yes_p:.0%}, model suggests {prob_reach:.0%}",
                    btc_price=current,
                    target_price=target,
                    market_price=yes_p,
                    premium=prob_reach - yes_p,
                )
            elif yes_p > prob_reach + 0.15:
                # Market overestimates — fade it with NO
                confidence = min(0.75, (yes_p - prob_reach) * 2)
                return TradingSignal(
                    side="NO",
                    confidence=confidence,
                    reasoning=f"BTC ${current:.0f} needs +{distance_pct:.1%} to reach ${target:.0f} in {hours_left:.0f}h — market implies {yes_p:.0%}, model suggests {prob_reach:.0%}",
                    btc_price=current,
                    target_price=target,
                    market_price=yes_p,
                    premium=yes_p - prob_reach,
                )

    elif market.direction == "below":
        # "Will BTC be below $X?"
        distance_pct = (current - target) / target
        if current <= target:
            if no_p < 0.70:
                confidence = min(0.95, 0.50 + abs(distance_pct) * 5)
                return TradingSignal(
                    side="NO",
                    confidence=confidence,
                    reasoning=f"BTC ${current:.0f} already below ${target:.0f} — market only implies {no_p:.0%}",
                    btc_price=current,
                    target_price=target,
                    market_price=no_p,
                    premium=(1.0 - no_p) * 0.5,
                )
        else:
            # BTC above target — need to drop
            distance_pct = (current - target) / target
            hours_left = market.hours_until_end or 24
            required_drop = distance_pct
            daily_vol = 0.04
            prob_reach = (
                min(0.90, (daily_vol * hours_left / 24) / required_drop)
                if required_drop > 0
                else 0.99
            )

            if no_p < prob_reach - 0.10:
                confidence = min(0.80, (prob_reach - no_p) * 2)
                return TradingSignal(
                    side="NO",
                    confidence=confidence,
                    reasoning=f"BTC ${current:.0f} needs -{required_drop:.1%} to drop below ${target:.0f} in {hours_left:.0f}h — market implies {no_p:.0%}",
                    btc_price=current,
                    target_price=target,
                    market_price=no_p,
                    premium=prob_reach - no_p,
                )

    return None


def momentum_confirmation(
    market: CryptoMarket, history: dict, lookback: int = 5
) -> float:
    """
    Return momentum score (-1 to 1) for a market's YES price.
    Positive = YES trending up.
    """
    key = market.id
    if key not in history or len(history[key]) < 3:
        return 0.0

    prices = [p for (ts, p) in history[key][-lookback:]]
    if len(prices) < 2:
        return 0.0

    delta = prices[-1] - prices[0]
    return max(-1.0, min(1.0, delta * 10))


def should_trade(
    signal: TradingSignal,
    genome,
    market: CryptoMarket,
    current_positions: int,
) -> bool:
    """
    Final gate: does this signal pass genome filters?
    """
    if signal is None:
        return False

    if current_positions >= genome.max_positions:
        return False

    if signal.confidence < 0.40:
        return False

    price = signal.market_price
    if price < 0.05 or price > 0.95:
        return False

    if market.hours_until_end and market.hours_until_end > genome.max_hours:
        return False

    liq = market.liquidity
    if liq < genome.min_liquidity:
        return False

    return True


if __name__ == "__main__":
    # Test with mock data
    from dataclasses import replace

    scanner = CryptoPriceScanner()
    btc = scanner.fetch_btc_price()
    print(f"BTC price: ${btc}")

    markets = scanner.scan()
    print(f"Crypto markets: {len(markets)}")

    for m in markets:
        sig = btc_price_signal(m, btc)
        if sig:
            print(f"\nSIGNAL: {sig.side} conf={sig.confidence:.0%}")
            print(f"  Reasoning: {sig.reasoning}")
            print(
                f"  BTC=${sig.btc_price} target=${sig.target_price} market_price={sig.market_price:.3f}"
            )
        else:
            print(f"\nNo signal: {m.question[:60]}")
