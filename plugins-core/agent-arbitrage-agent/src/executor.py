#!/usr/bin/env python3
"""
Production Executor — live Polymarket trading via py_clob_client.
Uses the optimized strategy genome from the simulation phase.
"""

import os
import sys
import json
import time
import signal
import logging
from datetime import datetime
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from config import (
    GAMMA_API,
    CLOB_API,
    MAX_TRADE_SIZE_LIVE,
    DRY_RUN,
    MAX_DAILY_SPEND,
    MAKER_REBATE_RATE,
)
from scanner import Scanner, Market
from strategy_genome import StrategyGenome


LOG_DIR = Path(__file__).parent / "logs"
LOG_DIR.mkdir(exist_ok=True)
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
    handlers=[
        logging.FileHandler(
            LOG_DIR / f"executor_{datetime.now().strftime('%Y%m%d_%H%M%S')}.log"
        ),
        logging.StreamHandler(),
    ],
)
log = logging.getLogger(__name__)


HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
ENV_PATH = os.path.join(HARVEY_HOME, "data", "arbitrage-agent", ".env.live")


class LiveExecutor:
    """
    Executes real trades on Polymarket CLOB using the best genome.
    Supports both maker (limit orders) and dry-run modes.
    """

    def __init__(self, genome: StrategyGenome, dry_run: bool = True):
        self.genome = genome
        self.dry_run = dry_run
        self.scanner = Scanner()
        self.client = None
        self.positions: list = []
        self.orders: list = []
        self.daily_spend = 0.0
        self.trades_today = 0
        self.daily_reset = datetime.now()
        self._running = False

        if not dry_run:
            self._init_client()

    def _init_client(self):
        """Initialize py_clob_client with credentials."""
        try:
            from dotenv import load_dotenv
            from py_clob_client.client import ClobClient
            from py_clob_client.clob_types import ApiCreds

            load_dotenv(ENV_PATH)
            host = CLOB_API
            pk = os.environ.get("POLYMARKET_PRIVATE_KEY")
            funder = os.environ.get("POLYMARKET_FUNDER_ADDRESS")
            sig_type = int(os.environ.get("POLYMARKET_SIGNATURE_TYPE", 2))

            self.client = ClobClient(
                host,
                key=pk,
                chain_id=137,
                signature_type=sig_type,
                funder=funder,
            )
            creds = self.client.create_or_derive_api_creds()
            self.client.set_api_creds(creds)

            if self.client.get_ok() != "OK":
                log.error("CLOB auth check failed")
                self.client = None
            else:
                log.info("CLOB session established")
        except Exception as e:
            log.error(f"Failed to init CLOB client: {e}")
            self.client = None

    def get_balance(self) -> float:
        """Get USDC collateral balance."""
        if not self.client:
            return 0.0
        try:
            return float(self.client.get_balance() or 0)
        except Exception:
            return 0.0

    def check_allowance(self) -> float:
        """Check USDC allowance."""
        if not self.client:
            return 0.0
        try:
            return float(self.client.get_allowance() or 0)
        except Exception:
            return 0.0

    def approve_if_needed(self):
        """Approve USDC for trading if needed."""
        if not self.client:
            return
        try:
            allowance = self.get_allowance()
            if allowance < 10.0:
                log.info("Approving USDC for trading...")
                self.client.post_approve()
        except Exception as e:
            log.warning(f"Approval failed: {e}")

    def place_maker_order(
        self, token_id: str, side: str, price: float, size: float
    ) -> Optional[Dict]:
        """Place a maker (limit) order. Returns order info or None."""
        if self.dry_run:
            log.info(
                f"[DRY-RUN] Would place {'BUY' if side == 'YES' else 'SELL'} {side} @ ${price:.4f} x {size}"
            )
            return {"orderID": "dry_run_123", "success": True}

        if not self.client:
            return None

        try:
            from py_clob_client.order_builder.constants import BUY, SELL
            from py_clob_client.clob_types import OrderArgs

            order_side = BUY if side == "YES" else SELL
            order_args = OrderArgs(
                price=price, size=size, side=order_side, token_id=token_id
            )
            signed = self.client.create_order(order_args)
            resp = self.client.post_order(signed)

            if resp.get("success"):
                log.info(
                    f"Order placed: {resp.get('orderID')} | {side} @ {price} x {size}"
                )
                self.orders.append(resp)
            else:
                log.warning(f"Order failed: {resp}")
            return resp
        except Exception as e:
            log.error(f"Order error: {e}")
            return None

    def cancel_order(self, order_id: str) -> bool:
        """Cancel an open order."""
        if self.dry_run:
            log.info(f"[DRY-RUN] Would cancel order {order_id}")
            return True

        if not self.client:
            return False
        try:
            resp = self.client.delete_order(order_id)
            return resp.get("success", False)
        except Exception as e:
            log.error(f"Cancel error: {e}")
            return False

    def scan_and_trade(self) -> Dict:
        """Scan markets and place maker orders per genome strategy."""
        if not self.dry_run and self.daily_spend >= MAX_DAILY_SPEND:
            return {"action": "daily_limit_reached", "spend": self.daily_spend}

        balance = self.get_balance()
        if balance < 1.0:
            return {"action": "insufficient_balance", "balance": balance}

        candidates = self.scanner.get_candidates(self.genome)
        if not candidates:
            return {
                "action": "no_candidates",
                "reason": "no markets pass genome filters",
            }

        stats = {
            "action": "scanned",
            "candidates": len(candidates),
            "balance": balance,
            "orders_placed": 0,
        }

        max_pos = self.genome.max_positions
        if len(self.positions) >= max_pos:
            return {"action": "max_positions", **stats}

        placed = 0
        for market in candidates[: max_pos - len(self.positions)]:
            spread = market.spread_pct
            mid = market.mid_price

            bid_price = mid - (
                spread
                * self.genome.spread_multiplier
                * self.genome.bid_offset_bps
                / 100
            )
            ask_price = mid + (
                spread
                * self.genome.spread_multiplier
                * self.genome.ask_offset_bps
                / 100
            )
            bid_price = max(bid_price, 0.001)
            ask_price = min(ask_price, 0.999)

            max_val = min(balance * self.genome.max_position_pct, MAX_TRADE_SIZE_LIVE)
            max_val = max(max_val, self.genome.min_position_size)

            if self.genome.post_both_sides:
                yes_size = max(1, int(max_val / bid_price))
                no_size = max(1, int(max_val / ask_price))

                r1 = self.place_maker_order(
                    market.tokens[0], "YES", bid_price, yes_size
                )
                if r1:
                    placed += 1
                    self.positions.append(
                        {"market": market, "side": "YES", "order": r1}
                    )

                r2 = self.place_maker_order(market.tokens[1], "NO", ask_price, no_size)
                if r2:
                    placed += 1
                    self.positions.append({"market": market, "side": "NO", "order": r2})
            else:
                side = "YES" if market.yes_price < 0.50 else "NO"
                price = bid_price if side == "YES" else ask_price
                token_id = market.tokens[0] if side == "YES" else market.tokens[1]
                size = max(1, int(max_val / price))

                r = self.place_maker_order(token_id, side, price, size)
                if r:
                    placed += 1
                    self.positions.append({"market": market, "side": side, "order": r})

            stats["orders_placed"] = placed

        return stats

    def cancel_stale_orders(self):
        """Cancel orders that have been open too long."""
        now = time.time()
        still_active = []
        for pos in self.positions:
            order = pos.get("order", {})
            order_id = order.get("orderID", "")
            if not order_id:
                continue
            placed_at = pos.get("placed_at", now)
            if now - placed_at > self.genome.cancel_after_seconds:
                if self.cancel_order(order_id):
                    log.info(f"Cancelled stale order {order_id}")
                    continue
            still_active.append(pos)
        self.positions = still_active

    def run(self, poll_interval: int = 30):
        """
        Main trading loop. Runs until interrupted.
        """
        log.info(
            f"Starting LiveExecutor | Dry={self.dry_run} | Genome: {self.genome.name}"
        )
        log.info(
            f"Params: spread={self.genome.spread_multiplier}x "
            f"fill={self.genome.fill_probability:.0%} "
            f"max_pos={self.genome.max_positions}"
        )
        self._running = True

        def shutdown(signum, frame):
            log.info("Shutdown signal received. Cancelling orders...")
            self._running = False

        signal.signal(signal.SIGINT, shutdown)
        signal.signal(signal.SIGTERM, shutdown)

        while self._running:
            try:
                if (datetime.now() - self.daily_reset).days >= 1:
                    self.daily_spend = 0.0
                    self.trades_today = 0
                    self.daily_reset = datetime.now()
                    log.info("Daily spend counter reset")

                self.cancel_stale_orders()
                result = self.scan_and_trade()

                if result["action"] == "daily_limit_reached":
                    log.info("Daily spend limit reached. Sleeping 5 min...")
                    time.sleep(300)
                elif result["action"] == "no_candidates":
                    time.sleep(poll_interval)
                else:
                    log.info(f"Scan result: {result}")
                    time.sleep(poll_interval)

            except Exception as e:
                log.error(f"Trading loop error: {e}")
                time.sleep(poll_interval)

        log.info("Executor stopped. Cancelling all open orders...")
        for pos in self.positions:
            order_id = pos.get("order", {}).get("orderID", "")
            if order_id:
                self.cancel_order(order_id)
        log.info("Shutdown complete.")


def load_best_genome() -> StrategyGenome:
    """Load the best genome from saved state."""
    path = Path(__file__).parent / "state" / "best_genome.json"
    if path.exists():
        with open(path) as f:
            data = json.load(f)
        return StrategyGenome.from_dict(data["genome"])
    log.warning("No saved genome found. Using default strategy.")
    return StrategyGenome()


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Polymarket Live Executor")
    parser.add_argument("--dry-run", action="store_true", default=DRY_RUN)
    parser.add_argument(
        "--genome", type=str, default="best", help="genome name or 'best'"
    )
    args = parser.parse_args()

    genome = load_best_genome()
    executor = LiveExecutor(genome, dry_run=args.dry_run)
    executor.run()
