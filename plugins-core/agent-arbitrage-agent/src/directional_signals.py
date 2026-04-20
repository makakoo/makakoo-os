#!/usr/local/opt/python@3.11/bin/python3.11
"""
Directional Signal Generator — BTC momentum × Polymarket price confirmation.

Core strategy:
  BTC momentum (primary) × PM momentum (confirmation) → directional signal

Signal scoring:
  - BTC strong up + PM confirming → HIGH confidence BUY YES
  - BTC strong up + PM fading     → LOW confidence / skip
  - BTC strong down + PM confirming → HIGH confidence BUY NO
  - BTC strong down + PM squeezing  → LOW confidence / skip
  - BTC neutral → rely on PM momentum alone

Genome params tune:
  - momentum thresholds per timeframe
  - confirmation requirements
  - confidence scaling
"""

from dataclasses import dataclass
from typing import Optional

from btc_data_fetcher import BTCDataFetcher, BTCFeatures
from polymarket_data_fetcher import PolymarketFetcher, PMMarket, PMFeatures


@dataclass
class DirectionalSignal:
    """A trading signal combining BTC + Polymarket momentum."""

    side: str  # "YES" or "NO"
    confidence: float  # 0.0–1.0
    reasoning: str
    btc_price: float
    btc_momentum: float
    pm_momentum: float
    pm_price: float
    entry_price: float
    timeframe: str  # "1m", "5m", "15m", "1h" (which timeframe triggered)
    score: float  # raw score before confidence


class DirectionalSignalGenerator:
    """
    Generates directional trade signals from BTC Binance data + Polymarket prices.

    Strategy thesis:
      Polymarket YES/NO prices are probability estimates.
      BTC is the leading indicator for most high-volume Polymarket markets.
      When BTC rallies, YES prices on crypto-correlated markets should rise.
      When BTC dumps, NO prices should rise.

      We detect momentum divergence + convergence and trade directionally.
    """

    def __init__(self):
        self.btc = BTCDataFetcher()
        self.pm = PolymarketFetcher()
        self._btc_features: Optional[BTCFeatures] = None
        self._pm_features: dict[str, PMFeatures] = {}

    def refresh(self):
        """Fetch fresh data from all sources."""
        self._btc_features = self.btc.features()
        # Pre-fetch PM features for top markets
        markets = self.pm.get_top_markets(min_volume=50000, limit=10)
        for m in markets:
            f = self.pm.features(m.yes_token)
            if f:
                self._pm_features[m.id] = f
        return self._btc_features

    def btc_features(self) -> BTCFeatures:
        if self._btc_features is None:
            self._btc_features = self.btc.features()
        return self._btc_features

    def pm_features(self, market_id: str) -> Optional[PMFeatures]:
        return self._pm_features.get(market_id)

    def generate(
        self,
        market: PMMarket,
        genome,
    ) -> Optional[DirectionalSignal]:
        """
        Generate a directional signal for a Polymarket market.

        Args:
            market: PMMarket to evaluate
            genome: StrategyGenome with threshold/weight params

        Returns:
            DirectionalSignal or None (no trade)
        """
        btc_f = self.btc_features()
        pm_f = self._pm_features.get(market.id)

        # Refresh PM features for this specific market
        if pm_f is None:
            pm_f = self.pm.features(market.yes_token)
            if pm_f is None:
                return None

        # ── BTC momentum evaluation ───────────────────────────────────────────
        # Check momentum across timeframes; weight by genome preference
        btc_scores = {
            "1m": btc_f.momentum_1m,
            "5m": btc_f.momentum_5m,
            "15m": btc_f.momentum_15m,
            "1h": btc_f.momentum_1h,
        }

        # Primary timeframe from genome
        primary_tf = getattr(genome, "primary_timeframe", "5m")
        btc_mom = btc_scores.get(primary_tf, btc_f.momentum_5m)

        # Weighted composite if genome uses multi-tf
        use_composite = getattr(genome, "use_composite_momentum", False)
        if use_composite:
            w1m = getattr(genome, "btc_weight_1m", 0.0)
            w5m = getattr(genome, "btc_weight_5m", 0.3)
            w15m = getattr(genome, "btc_weight_15m", 0.4)
            w1h = getattr(genome, "btc_weight_1h", 0.3)
            total = w1m + w5m + w15m + w1h
            btc_mom = (
                btc_f.momentum_1m * (w1m / total)
                + btc_f.momentum_5m * (w5m / total)
                + btc_f.momentum_15m * (w15m / total)
                + btc_f.momentum_1h * (w1h / total)
            )

        # ── Polymarket momentum evaluation ──────────────────────────────────
        pm_mom = pm_f.momentum_5m  # short-term PM momentum
        pm_mom_15m = pm_f.momentum_15m

        # ── Thresholds from genome ──────────────────────────────────────────
        bull_thresh = getattr(genome, "bull_threshold", 0.5)  # % BTC mom to call BULL
        bear_thresh = getattr(genome, "bear_threshold", -0.5)  # % BTC mom to call BEAR
        pm_confirm_thresh = getattr(
            genome, "pm_confirm_threshold", 0.02
        )  # % PM mom to confirm

        # RSI filter
        rsi_ok_bull = btc_f.rsi_14 < getattr(genome, "rsi_overbought", 70)
        rsi_ok_bear = btc_f.rsi_14 > getattr(genome, "rsi_oversold", 30)
        rsi_filter = getattr(genome, "use_rsi_filter", False)
        if rsi_filter and not (rsi_ok_bull or rsi_ok_bear):
            return None  # RSI outside acceptable range

        # ── Signal generation ───────────────────────────────────────────────
        btc_strong_bull = btc_mom > bull_thresh
        btc_strong_bear = btc_mom < bear_thresh
        btc_neutral = not btc_strong_bull and not btc_strong_bear

        pm_confirming_yes = pm_mom > pm_confirm_thresh or pm_mom_15m > pm_confirm_thresh
        pm_confirming_no = (
            pm_mom < -pm_confirm_thresh or pm_mom_15m < -pm_confirm_thresh
        )

        # ── Entry price ─────────────────────────────────────────────────────
        entry_price = self.pm.fetch_current_price(market.yes_token)
        if entry_price < 0.02 or entry_price > 0.98:
            return None

        # ── Confidence scoring ───────────────────────────────────────────────
        base_conf = getattr(genome, "base_confidence", 0.50)
        conf_per_bps_mom = getattr(
            genome, "conf_per_bps_mom", 0.005
        )  # per 1bps BTC mom
        max_conf = getattr(genome, "max_confidence", 0.85)

        side = None
        confidence = 0.0
        reasoning = ""

        if btc_strong_bull and pm_confirming_yes:
            # STRONG: BTC up + PM YES trending up → BUY YES
            raw_conf = base_conf + abs(btc_mom * conf_per_bps_mom * 100)
            confidence = min(max_conf, raw_conf)
            side = "YES"
            reasoning = (
                f"BTC +{btc_mom:.3f}% ({primary_tf}) + PM YES +{pm_mom:.4f}% → YES "
                f"RSI={btc_f.rsi_14:.0f} BB={btc_f.bb_position:.2f}"
            )
        elif btc_strong_bull and not pm_confirming_yes:
            # WEEKLY: BTC up but PM fading → skip or tiny YES
            if pm_mom < -pm_confirm_thresh * 2:
                return None  # strong divergence, skip
            confidence = base_conf * 0.5
            side = "YES"
            reasoning = (
                f"BTC +{btc_mom:.3f}% but PM fading ({pm_mom:.4f}%) → weak YES signal"
            )
        elif btc_strong_bear and pm_confirming_no:
            # STRONG: BTC down + PM NO trending up → BUY NO
            raw_conf = base_conf + abs(btc_mom * conf_per_bps_mom * 100)
            confidence = min(max_conf, raw_conf)
            side = "NO"
            entry_price = 1.0 - entry_price  # NO price
            reasoning = (
                f"BTC {btc_mom:.3f}% ({primary_tf}) + PM NO +{abs(pm_mom):.4f}% → NO "
                f"RSI={btc_f.rsi_14:.0f} BB={btc_f.bb_position:.2f}"
            )
        elif btc_strong_bear and not pm_confirming_no:
            if pm_mom > pm_confirm_thresh * 2:
                return None
            confidence = base_conf * 0.5
            side = "NO"
            entry_price = 1.0 - entry_price
            reasoning = (
                f"BTC {btc_mom:.3f}% but PM squeezing ({pm_mom:.4f}%) → weak NO signal"
            )
        elif btc_neutral:
            # No BTC trend — skip unless PM momentum is very strong
            strong_pm_yes = (
                pm_mom > pm_confirm_thresh * 3 or pm_mom_15m > pm_confirm_thresh * 3
            )
            strong_pm_no = (
                pm_mom < -pm_confirm_thresh * 3 or pm_mom_15m < -pm_confirm_thresh * 3
            )
            if strong_pm_yes:
                confidence = base_conf * 0.6
                side = "YES"
                reasoning = (
                    f"BTC neutral, PM YES momentum +{pm_mom:.4f}% ({primary_tf})"
                )
            elif strong_pm_no:
                confidence = base_conf * 0.6
                side = "NO"
                entry_price = 1.0 - entry_price
                reasoning = f"BTC neutral, PM NO momentum {pm_mom:.4f}% ({primary_tf})"
            else:
                return None  # nothing happening

        # Apply min confidence filter
        min_conf = getattr(genome, "min_confidence", 0.40)
        if confidence < min_conf:
            return None

        # Volume filter
        vol_filter = getattr(genome, "min_volume_24h", 10000)
        if market.volume_24h < vol_filter:
            return None

        # Max position price filter
        max_price = getattr(genome, "max_entry_price", 0.95)
        if entry_price > max_price:
            return None

        return DirectionalSignal(
            side=side,
            confidence=confidence,
            reasoning=reasoning,
            btc_price=btc_f.price,
            btc_momentum=btc_mom,
            pm_momentum=pm_mom,
            pm_price=entry_price,
            entry_price=entry_price,
            timeframe=primary_tf,
            score=btc_mom,
        )


if __name__ == "__main__":
    gen = DirectionalSignalGenerator()
    btc_f = gen.refresh()

    print(f"\nBTC: ${btc_f.price:,.0f}")
    print(
        f"  1m: {btc_f.momentum_1m:+.3f}%  5m: {btc_f.momentum_5m:+.3f}%  15m: {btc_f.momentum_15m:+.3f}%  1h: {btc_f.momentum_1h:+.3f}%"
    )
    print(
        f"  RSI: {btc_f.rsi_14:.1f}  MACD hist: {btc_f.macd_hist:+.4f}  BB: {btc_f.bb_position:.3f}"
    )

    markets = gen.pm.get_top_markets(min_volume=50000, limit=5)
    print(f"\nTop markets: {len(markets)}")

    # Create a mock genome with reasonable defaults
    from types import SimpleNamespace

    genome = SimpleNamespace(
        bull_threshold=0.3,
        bear_threshold=-0.3,
        pm_confirm_threshold=0.01,
        min_confidence=0.40,
        base_confidence=0.50,
        conf_per_bps_mom=0.01,
        max_confidence=0.80,
        use_rsi_filter=False,
        rsi_overbought=70,
        rsi_oversold=30,
        primary_timeframe="5m",
        use_composite_momentum=False,
        btc_weight_1m=0.0,
        btc_weight_5m=0.3,
        btc_weight_15m=0.4,
        btc_weight_1h=0.3,
        min_volume_24h=10000,
        max_entry_price=0.95,
    )

    for m in markets:
        sig = gen.generate(m, genome)
        if sig:
            print(
                f"\n  SIGNAL [{sig.confidence:.0%}]: {sig.side} @ {sig.entry_price:.4f}"
            )
            print(f"  {sig.reasoning}")
        else:
            print(f"\n  NO SIGNAL: {m.question[:60]}")
