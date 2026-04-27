#!/usr/local/opt/python@3.11/bin/python3.11
"""
Intraday Signaler — Production BTC Technical Analysis Trading System.

Continuously:
  1. Stream live BTC 5s/15s/1m/5m/15m candles via Polymarket WebSocket
  2. Run multi-TF technical analysis (RSI, MACD, Bollinger, Fib, S/R)
  3. Generate directional signals with probability scores
  4. Map signals to Polymarket markets (edge-based)
  5. Execute paper trades via PolymarketPaperTrader

No guessing. Data-driven signals from real market data.
"""

import json
import os
import signal
import sys
import time
from dataclasses import asdict
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from btc_signals import BTCSignalGenerator, CompositeSignal
from btc_price_feed import BTCPriceFeed
from polymarket_signal_mapper import PolymarketSignalMapper
from polymarket_paper_trader import PolymarketPaperTrader
from strategy_genome import StrategyGenome

HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
BEST_GENOME_FILE = STATE_DIR / "best_genome.json"
SIGNALS_LOG = DATA_DIR / "signals_log.jsonl"

POLL_SECONDS = 30
PAPER_CAPITAL = 100.0


def log(msg, file=None):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_DIR / "intraday_signaler.log", "a") as f:
        f.write(line + "\n")


def load_genome() -> StrategyGenome:
    if BEST_GENOME_FILE.exists():
        with open(BEST_GENOME_FILE) as f:
            d = json.load(f)
        genome = StrategyGenome.from_dict(d.get("genome", d))
        log(f"Loaded genome: {genome.name} (score={d.get('score', '?')})")
        return genome

    log("No genome — creating default")
    g = StrategyGenome()
    g.name = "default_signaler"
    g.min_confidence = 0.50
    g.max_hours = 48.0
    g.max_positions = 5
    g.max_position_pct = 0.15
    g.kelly_fraction = 0.25
    g.cancel_after_seconds = 600
    g.fill_probability = 0.30
    g.min_spread_bps = 30
    g.min_volume_usd = 5000
    g.min_liquidity = 1000
    return g


def log_signal(sig: CompositeSignal):
    """Log signal to signals_log.jsonl for later analysis."""
    d = {
        "timestamp": sig.timestamp,
        "direction": sig.direction,
        "confidence": round(sig.confidence, 3),
        "probability_up": round(sig.probability_up, 3),
        "probability_down": round(sig.probability_down, 3),
        "score": round(sig.score, 2),
        "btc_price": sig.btc_price,
        "fib_level": sig.fib_level,
        "nearest_support": sig.nearest_support,
        "nearest_resistance": sig.nearest_resistance,
        "reasoning": sig.reasoning,
        "tf_signals": [
            {
                "timeframe": ts.timeframe,
                "direction": ts.direction,
                "score": round(ts.score, 1),
                "rsi": round(ts.rsi, 1),
                "macd": ts.macd_signal,
                "bb": ts.bb_signal,
                "confidence": round(ts.confidence, 2),
            }
            for ts in sig.timeframe_signals
        ],
    }
    with open(SIGNALS_LOG, "a") as f:
        f.write(json.dumps(d) + "\n")


def execute_trades(
    trader: PolymarketPaperTrader,
    opportunities: list,
    genome: StrategyGenome,
    capital: float,
):
    """Execute paper trades on mapped Polymarket opportunities."""
    if not opportunities:
        return 0

    executed = 0
    open_ids = {p.market_id for p in trader.positions}
    pending_ids = {o["market_id"] for o in trader.pending}

    for opp in opportunities:
        if len(trader.positions) + len(trader.pending) >= genome.max_positions:
            break
        if opp.market_id in open_ids or opp.market_id in pending_ids:
            continue

        # Skip if edge/confidence too low
        if opp.edge < 0.03:
            continue

        price = opp.yes_price if opp.position_side == "YES" else opp.no_price
        if price < 0.02 or price > 0.98:
            continue

        # Position sizing
        spend_pct = min(
            genome.max_position_pct, opp.position_size_pct * genome.kelly_fraction * 4
        )
        spend = capital * spend_pct
        shares = max(1.0, spend / price)
        cost = shares * price

        if cost > capital * 0.90:
            shares = capital * 0.85 / price
            cost = shares * price

        if cost < 0.10 or shares < 1.0:
            continue

        # Reserve capital for this pending order
        trader.capital -= cost
        trader.pending.append(
            {
                "market_id": opp.market_id,
                "question": opp.question,
                "token_id": "",  # filled by scanner
                "side": opp.position_side,
                "price": price,
                "shares": shares,
                "cost": cost,
                "placed_at": datetime.now().isoformat(),
                "expires_at": time.time() + genome.cancel_after_seconds,
                "signal_confidence": opp.edge,
                "signal_reasoning": opp.reasoning,
                "btc_direction": opp.btc_direction,
            }
        )

        log(
            f"[TRADE] {opp.position_side} {shares:.2f}@{price:.4f} cost=${cost:.2f} "
            f"| edge={opp.edge:.1%} EV={opp.expected_value:.4f} "
            f"| {opp.question[:50]}"
        )
        executed += 1
        open_ids.add(opp.market_id)

    return executed


def main():
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--capital", type=float, default=PAPER_CAPITAL)
    parser.add_argument("--poll", type=int, default=POLL_SECONDS)
    parser.add_argument("--no-stream", action="store_true")
    args = parser.parse_args()

    LOG_DIR.mkdir(parents=True, exist_ok=True)
    STATE_DIR.mkdir(parents=True, exist_ok=True)

    log("=" * 60)
    log("INTRADAY SIGNALER STARTING — BTC Technical Analysis Trading")
    log(f"Capital: ${args.capital} | Poll: {args.poll}s")
    log("=" * 60)

    genome = load_genome()

    # Initialize paper trader
    trader = PolymarketPaperTrader(genome, paper_capital=args.capital)

    # Initialize BTC signal generator + price feed
    signal_gen = BTCSignalGenerator(min_prices=25)
    mapper = PolymarketSignalMapper(min_edge=0.03, min_liquidity=1000)

    price_feed = None
    if not args.no_stream:
        price_feed = BTCPriceFeed(cg_interval=15, pm_ws_interval=10).start()
        log("BTCPriceFeed started — waiting for data...")
        time.sleep(35)

    def stop_handler(sig, frame):
        log("Shutdown signal received...")
        trader._running = False
        if price_feed:
            price_feed.stop()

    signal.signal(signal.SIGINT, stop_handler)
    signal.signal(signal.SIGTERM, stop_handler)

    trader._running = True
    tick = 0
    last_signal_time = 0
    last_opportunities: list = []

    while trader._running:
        try:
            tick += 1

            # Generate BTC signal
            sig = None
            if price_feed:
                sig = signal_gen.generate(price_feed)

            if sig and tick % 3 == 0:
                log_signal(sig)
                log(
                    f"[SIGNAL {tick}] {sig.direction.upper()} conf={sig.confidence:.0%} "
                    f"BTC=${sig.btc_price:,.0f} P(up)={sig.probability_up:.0%} "
                    f"score={sig.score:.0f} | {' '.join(sig.reasoning[:3])}"
                )

            # Map to Polymarket opportunities
            opportunities = []
            if sig and sig.direction != "neutral" and sig.confidence >= 0.40:
                opportunities = mapper.map_signal(sig, sig.btc_price)
                if opportunities:
                    last_opportunities = opportunities
                    log(
                        f"[MAPPER] {len(opportunities)} opportunities | "
                        f"top edge={opportunities[0].edge:.1%} "
                        f"{opportunities[0].position_side}@{opportunities[0].market_probability:.3f} "
                        f"{opportunities[0].question[:50]}"
                    )

            # Tick pending orders + resolve positions
            trader._tick_pending()
            trader._resolve_positions()

            # Execute trades on mapped opportunities
            if opportunities:
                executed = execute_trades(trader, opportunities, genome, trader.capital)
                if executed > 0:
                    log(
                        f"[FILLED] {executed} new positions | cap=${trader.capital:.2f}"
                    )

            # Periodic status
            if tick % 10 == 0:
                pnl = trader.capital - PAPER_CAPITAL
                total = trader.wins + trader.losses + trader.breakeven
                wr = trader.wins / max(total, 1)
                log(
                    f"[STATUS {tick}] cap=${trader.capital:.2f} pnl=${pnl:+.4f} "
                    f"W={trader.wins} L={trader.losses} WR={wr:.0%} "
                    f"pos={len(trader.positions)} pending={len(trader.pending)}"
                )

            trader._save()
            time.sleep(args.poll)

        except Exception as e:
            log(f"ERROR: {e}")
            import traceback

            log(traceback.format_exc())
            time.sleep(args.poll)

    if price_feed:
        price_feed.stop()
    trader._save()
    log("Signaler stopped.")


if __name__ == "__main__":
    main()
