#!/usr/local/opt/python@3.11/bin/python3.11
"""
Intraday Monitor — Continuous Polymarket BTC/ETH trading daemon.

Runs 24/7 using the best evolved genome.
- Primary scanner: CryptoPriceScanner (BTC/ETH price-target markets)
- Fallback scanner: generic Scanner (other opportunities)
- Uses BTC price signal logic (not momentum)
- Paper trading until bankroll is sufficient for real money

Usage:
    python3 intraday_monitor.py [--genome genome.json] [--capital 100]
    nohup python3 intraday_monitor.py --capital 100 >> logs/intraday_monitor.log 2>&1 &
"""

import json
import os
import signal
import sys
import time
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from polymarket_paper_trader import PolymarketPaperTrader
from strategy_genome import StrategyGenome

HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
BEST_GENOME_FILE = STATE_DIR / "best_genome.json"

POLL_SECONDS = 30
PAPER_CAPITAL = 100.0


def log(msg, file=None):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    log_file = LOG_DIR / "intraday_monitor.log"
    with open(log_file, "a") as f:
        f.write(line + "\n")


def load_best_genome() -> StrategyGenome:
    if BEST_GENOME_FILE.exists():
        with open(BEST_GENOME_FILE) as f:
            d = json.load(f)
        genome = StrategyGenome.from_dict(d.get("genome", d))
        log(f"Loaded genome: {genome.name} (score={d.get('score', '?')})")
        return genome

    log("No genome found — creating default")
    g = StrategyGenome()
    g.name = "default_intraday"
    g.min_liquidity = 1000
    g.max_hours = 48.0
    g.max_positions = 5
    g.max_position_pct = 0.15
    g.kelly_fraction = 0.25
    g.cancel_after_seconds = 600
    g.fill_probability = 0.30
    g.min_spread_bps = 30
    g.min_volume_usd = 5000
    return g


def main():
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--capital", type=float, default=PAPER_CAPITAL)
    parser.add_argument("--poll", type=int, default=POLL_SECONDS)
    args = parser.parse_args()

    LOG_DIR.mkdir(parents=True, exist_ok=True)
    STATE_DIR.mkdir(parents=True, exist_ok=True)

    log("=" * 60)
    log("INTRADAY MONITOR STARTING — Polymarket BTC/ETH Intraday Trading")
    log(f"Capital: ${args.capital} | Poll: {args.poll}s")
    log("=" * 60)

    genome = load_best_genome()

    trader = PolymarketPaperTrader(genome, paper_capital=args.capital)

    def stop_handler(sig, frame):
        log("Shutdown signal received — stopping gracefully...")
        trader._running = False

    signal.signal(signal.SIGINT, stop_handler)
    signal.signal(signal.SIGTERM, stop_handler)

    trader._running = True

    log(f"Scanner status: {trader.crypto_scanner.status()}")
    btc = trader.crypto_scanner.fetch_btc_price() if trader.crypto_scanner else None
    if btc:
        log(f"Current BTC price: ${btc:,.0f}")

    tick = 0
    while trader._running:
        try:
            tick += 1
            trader.scanner.scan()

            btc_price = None
            if trader.crypto_scanner:
                btc_price = trader.crypto_scanner.fetch_btc_price()
                trader.crypto_scanner.scan()

                if tick % 5 == 0:
                    cs = trader.crypto_scanner.status()
                    log(
                        f"[TICK {tick}] BTC=${cs.get('btc_price', '?')} | "
                        f"crypto_mkts={cs['total_crypto_markets']} | "
                        f"btc={cs['btc_markets']} eth={cs['eth_markets']} | "
                        f"intraday={cs['intraday_markets']} | "
                        f"capital=${trader.capital:.2f} | "
                        f"positions={len(trader.positions)} pending={len(trader.pending)}"
                    )

            trader._tick_pending()
            trader._resolve_positions()
            trader._open_orders_crypto(btc_price)

            if tick % 10 == 0:
                trader._status()

            trader._save()
            time.sleep(args.poll)

        except Exception as e:
            log(f"ERROR: {e}")
            import traceback

            log(traceback.format_exc())
            time.sleep(args.poll)

    trader._save()
    trader._status()
    log("Monitor stopped.")


if __name__ == "__main__":
    main()
