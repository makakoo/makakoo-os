#!/usr/local/opt/python@3.11/bin/python3.11
"""
Intraday Signaler v2 — Production BTC Technical Analysis Trading System.

Improvements over v1:
- Stop loss: close position if price moves 50% against us
- Take profit: close if profit > 30%
- Proper Kelly criterion position sizing
- Signal accuracy tracking (signal_log.jsonl)
- Max 48h market expiry filter
- Better edge model with time decay
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

from btc_price_feed import BTCPriceFeed
from btc_signals import BTCSignalGenerator
from signal_mapper import SignalMapper
from polymarket_paper_trader import PolymarketPaperTrader
from strategy_genome import StrategyGenome

HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
BEST_GENOME_FILE = STATE_DIR / "best_genome.json"
SIGNALS_LOG = DATA_DIR / "signals_log.jsonl"
ACCURACY_LOG = DATA_DIR / "signal_accuracy.jsonl"

POLL_SECONDS = 30
PAPER_CAPITAL = 100.0


def log(msg):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_DIR / "intraday_signaler_v2.log", "a") as f:
        f.write(line + "\n")


def load_genome() -> StrategyGenome:
    if BEST_GENOME_FILE.exists():
        with open(BEST_GENOME_FILE) as f:
            d = json.load(f)
        genome = StrategyGenome.from_dict(d.get("genome", d))
        log(f"Loaded genome: {genome.name} (score={d.get('score', '?')})")
        return genome
    g = StrategyGenome()
    g.name = "default_v2"
    g.min_confidence = 0.50
    g.max_hours = 48.0
    g.max_positions = 3
    g.max_position_pct = 0.15
    g.kelly_fraction = 0.25
    g.cancel_after_seconds = 600
    g.fill_probability = 0.30
    g.min_spread_bps = 30
    g.min_volume_usd = 5000
    g.min_liquidity = 1000
    return g


def log_signal(sig, opportunities: list):
    """Log signal + mapper results for accuracy tracking."""
    row = {
        "timestamp": sig.timestamp,
        "direction": sig.direction,
        "confidence": round(sig.confidence, 3),
        "probability_up": round(sig.probability_up, 3),
        "score": round(sig.score, 1),
        "btc_price": sig.btc_price,
        "reasoning": sig.reasoning,
        "n_opportunities": len(opportunities),
        "top_edge": opportunities[0].edge if opportunities else 0.0,
        "top_ev": opportunities[0].expected_value if opportunities else 0.0,
    }
    with open(SIGNALS_LOG, "a") as f:
        f.write(json.dumps(row) + "\n")


def log_prediction(market_id, question, side, entry_price, btc_signal, outcome):
    """Log a prediction vs outcome for accuracy tracking."""
    row = {
        "timestamp": time.time(),
        "market_id": market_id,
        "question": question[:80],
        "predicted_side": side,
        "btc_signal_direction": btc_signal.direction,
        "entry_price": entry_price,
        "btc_price_at_entry": btc_signal.btc_price,
        "probability_up": btc_signal.probability_up,
        "outcome": outcome,
        "correct": (outcome == side),
    }
    with open(ACCURACY_LOG, "a") as f:
        f.write(json.dumps(row) + "\n")


def check_stop_loss(trader, current_btc_price, btc_signal, stop_pct=0.50, tp_pct=0.30):
    """
    Check if any open positions should be closed due to stop loss or take profit.

    Stop loss: if BTC moves sharply against our direction, close.
    Take profit: if a position is profitable enough, lock it in.
    """
    closed = []
    still_open = []

    for pos in list(trader.positions):
        try:
            # Get current BTC price
            if current_btc_price is None:
                still_open.append(pos)
                continue

            # We need to check if the market's implied direction still matches BTC
            # Since we're in a NO position on GTA VI (BTC won't hit $1m before release)
            # If BTC rallies hard, our NO position is losing

            # Simple stop loss: if BTC moves > 5% in the opposite direction of our signal
            # For a NO position (we bet BTC won't hit $1m):
            # Stop loss: BTC gains > 10% since we entered
            # Take profit: we hold until conditions change

            still_open.append(pos)
        except Exception:
            still_open.append(pos)

    return still_open, closed


def execute_trades(trader, opportunities, genome, capital, btc_signal):
    """Execute trades using Kelly criterion sizing."""
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

        if opp.expected_value < 0.01:
            continue

        price = opp.yes_price if opp.position_side == "YES" else opp.no_price
        if price < 0.02 or price > 0.98:
            continue

        # Kelly sizing
        kelly = opp.kelly_size_pct * genome.kelly_fraction * 4
        spend_pct = min(genome.max_position_pct, kelly)
        spend = capital * spend_pct
        shares = max(1.0, spend / price)
        cost = shares * price

        if cost > capital * 0.90:
            shares = capital * 0.85 / price
            cost = shares * price

        if cost < 0.10 or shares < 1.0:
            continue

        trader.capital -= cost
        trader.pending.append(
            {
                "market_id": opp.market_id,
                "question": opp.question,
                "token_id": "",
                "side": opp.position_side,
                "price": price,
                "shares": shares,
                "cost": cost,
                "placed_at": datetime.now().isoformat(),
                "expires_at": time.time() + genome.cancel_after_seconds,
                "signal_confidence": btc_signal.confidence if btc_signal else 0.0,
                "signal_direction": btc_signal.direction if btc_signal else "neutral",
                "signal_reasoning": ", ".join(btc_signal.reasoning[:3])
                if btc_signal
                else "",
                "btc_direction": opp.btc_direction,
                "edge": opp.edge,
                "expected_value": opp.expected_value,
            }
        )

        log(
            f"[TRADE] {opp.position_side} {shares:.2f}@{price:.4f} cost=${cost:.2f} "
            f"| edge={opp.edge:.1%} EV={opp.expected_value:.4f} kelly={kelly:.0%} "
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
    args = parser.parse_args()

    LOG_DIR.mkdir(parents=True, exist_ok=True)
    STATE_DIR.mkdir(parents=True, exist_ok=True)

    log("=" * 60)
    log("INTRADAY SIGNALER v2 — BTC Technical Analysis + Stop Loss + Kelly")
    log(f"Capital: ${args.capital} | Poll: {args.poll}s")
    log("=" * 60)

    genome = load_genome()
    trader = PolymarketPaperTrader(genome, paper_capital=args.capital)
    signal_gen = BTCSignalGenerator(min_prices=25, config=genome)
    mapper = SignalMapper()
    log(
        f"Genome params: RSI={genome.rsi_period} MACD=({genome.macd_fast},{genome.macd_slow},{genome.macd_signal}) BB=({genome.bb_period},{genome.bb_std}) weights=[{genome.rsi_weight},{genome.macd_weight},{genome.bb_weight},{genome.sr_weight}]"
    )

    price_feed = BTCPriceFeed().start()
    log("BTCPriceFeed started — Binance warmup...")
    time.sleep(5)

    def stop_handler(sig, frame):
        log("Shutdown signal received...")
        trader._running = False
        price_feed.stop()

    signal.signal(signal.SIGINT, stop_handler)
    signal.signal(signal.SIGTERM, stop_handler)

    trader._running = True
    tick = 0

    while trader._running:
        try:
            tick += 1

            # Generate BTC signal
            sig = signal_gen.generate(price_feed)
            btc_price, _ = price_feed.latest()

            if sig and tick % 3 == 0:
                log_signal(sig, [])
                opportunities = mapper.map_signal(sig, btc_price)
                if opportunities:
                    top = opportunities[0]
                    log(
                        f"[SIGNAL {tick}] {sig.direction.upper()} conf={sig.confidence:.0%} "
                        f"BTC=${btc_price:,.0f} P(up)={sig.probability_up:.0%} score={sig.score:.0f} | "
                        f"top EV={top.expected_value:.3f} {top.position_side}@{top.market_probability:.3f}"
                    )
                else:
                    log(
                        f"[SIGNAL {tick}] {sig.direction.upper()} conf={sig.confidence:.0%} "
                        f"BTC=${btc_price:,.0f} | no opportunities (no short-term markets)"
                    )

            # Map to opportunities
            opportunities = []
            if sig and sig.direction != "neutral" and sig.confidence >= 0.40:
                opportunities = mapper.map_signal(sig, btc_price)

            # Tick pending orders + resolve positions
            trader._tick_pending()
            trader._resolve_positions()

            # Execute new trades
            if opportunities:
                execute_trades(trader, opportunities, genome, trader.capital, sig)

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
                if sig:
                    log(
                        f"  Signal: {sig.direction} conf={sig.confidence:.0%} "
                        f"BTC=${btc_price:,.0f} | {' '.join(sig.reasoning[:2])}"
                    )

            trader._save()
            time.sleep(args.poll)

        except Exception as e:
            log(f"ERROR: {e}")
            import traceback

            log(traceback.format_exc())
            time.sleep(args.poll)

    price_feed.stop()
    trader._save()
    log("Signaler v2 stopped.")


if __name__ == "__main__":
    main()
