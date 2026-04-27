#!/usr/bin/env python3
"""
Paper Trader v4 — Virtual money, real Polymarket Gamma prices.
All P&L is virtual. No real money moved.
"""

import os, sys, json, time, signal, random, logging
from datetime import datetime
from pathlib import Path
from dataclasses import dataclass, asdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from scanner import Scanner, GAMMA_API, CLOB_API
from strategy_genome import StrategyGenome

PAPER_CAPITAL = 100.0
STATE_FILE = Path(__file__).parent / "state" / "paper_trades.json"
JOURNAL_FILE = Path(__file__).parent / "state" / "paper_journal.jsonl"
LOG_DIR = Path(__file__).parent / "logs"
MAKER_REBATE_BPS = 1.0
TAKER_FEE_BPS = 5.0
MIN_CAPITAL = 1.0


@dataclass
class PaperPosition:
    market_id: str
    question: str
    token_id: str
    side: str
    entry_price: float
    size: float
    value: float
    opened_at: str


@dataclass
class PaperTrade:
    id: str
    market_id: str
    question: str
    side: str
    entry_price: float
    exit_price: float
    size: float
    pnl: float
    rebate: float
    fee: float
    opened_at: str
    closed_at: str
    duration_secs: float
    result: str
    how: str


class PaperTrader:
    def __init__(self, genome: StrategyGenome, paper_capital: float = PAPER_CAPITAL):
        self.genome = genome
        self.paper_capital = paper_capital
        self.capital = paper_capital
        self.positions = []
        self.trades = []
        self.pending = []
        self.scanner = Scanner()
        self.rebates = 0.0
        self.fees = 0.0
        self.wins = self.losses = self.breakeven = 0
        self._counter = 0
        self._running = False
        self.started_at = datetime.now().isoformat()

        LOG_DIR.mkdir(parents=True, exist_ok=True)
        STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
        lf = LOG_DIR / (
            "paper_trader_%s.log" % datetime.now().strftime("%Y%m%d_%H%M%S")
        )
        logging.basicConfig(
            level=logging.INFO,
            format="%(asctime)s %(message)s",
            handlers=[logging.FileHandler(lf), logging.StreamHandler()],
        )
        self.log = logging.getLogger("paper_trader")

    def _id(self) -> str:
        self._counter += 1
        return "t_%s_%d" % (datetime.now().strftime("%H%M%S"), self._counter)

    def _now(self) -> str:
        return datetime.now().isoformat()

    def _load(self):
        if STATE_FILE.exists():
            with open(STATE_FILE) as f:
                d = json.load(f)
            self.capital = d.get("capital", self.paper_capital)
            self.positions = [PaperPosition(**p) for p in d.get("positions", [])]
            self.trades = [PaperTrade(**t) for t in d.get("trades", [])]
            self.rebates = d.get("total_rebates", 0.0)
            self.fees = d.get("total_fees", 0.0)
            self.wins = d.get("wins", 0)
            self.losses = d.get("losses", 0)
            self.breakeven = d.get("breakeven", 0)
            self.started_at = d.get("started_at", self.started_at)
            self.log.info(
                "Loaded: $%.2f | %d pos | %d trades | W=%d L=%d"
                % (
                    self.capital,
                    len(self.positions),
                    len(self.trades),
                    self.wins,
                    self.losses,
                )
            )

    def _save(self):
        with open(STATE_FILE, "w") as f:
            json.dump(
                {
                    "capital": self.capital,
                    "positions": [asdict(p) for p in self.positions],
                    "trades": [asdict(t) for t in self.trades],
                    "total_rebates": self.rebates,
                    "total_fees": self.fees,
                    "wins": self.wins,
                    "losses": self.losses,
                    "breakeven": self.breakeven,
                    "started_at": self.started_at,
                    "updated_at": self._now(),
                },
                f,
                indent=2,
            )

    def _journal(self, t: PaperTrade):
        with open(JOURNAL_FILE, "a") as f:
            f.write(json.dumps(asdict(t)) + "\n")

    def _place(self, m, side: str, price: float, size: int, how: str) -> bool:
        value = round(price * size, 4)
        if value < 0.50:
            return False
        if value > self.capital * 0.85:
            return False
        self.pending.append(
            {
                "market_id": m.id,
                "question": m.question,
                "token_id": m.tokens[0] if side == "YES" else m.tokens[1],
                "side": side,
                "price": price,
                "size": size,
                "value": value,
                "placed_at": self._now(),
                "expires_at": time.time() + self.genome.cancel_after_seconds,
                "how": how,
            }
        )
        self.capital -= value
        self.log.info(
            "[POST] %s %d @ $%.4f ($%.2f) | %.35s | cap=$%.2f"
            % (side, size, price, value, m.question, self.capital)
        )
        return True

    def _tick_pending(self):
        still = []
        for o in self.pending:
            mb = self.scanner._cache.get(o["market_id"])
            if not mb:
                still.append(o)
                continue

            fill_prob = (
                self.genome.fill_probability * self.genome.spread_multiplier * 0.20
            )
            fill_prob = min(fill_prob, 0.50)

            if random.random() < fill_prob:
                rebate = o["value"] * MAKER_REBATE_BPS / 10000
                self.capital += o["value"] + rebate
                self.rebates += rebate
                self.positions.append(
                    PaperPosition(
                        market_id=o["market_id"],
                        question=o["question"],
                        token_id=o["token_id"],
                        side=o["side"],
                        entry_price=o["price"],
                        size=o["size"],
                        value=o["value"],
                        opened_at=o["placed_at"],
                    )
                )
                self.log.info(
                    "[FILLED] %s %d @ $%.4f (maker+rebate $%.6f) | cap=$%.2f"
                    % (o["side"], o["size"], o["price"], rebate, self.capital)
                )
            elif time.time() >= o["expires_at"]:
                self.capital += o["value"]
                self.log.info("[EXPIRED] %s @ $%.4f" % (o["side"], o["price"]))
            else:
                still.append(o)
        self.pending = still

    def _resolve_positions(self):
        still = []
        for pos in self.positions:
            mb = self.scanner._cache.get(pos.market_id)
            if not mb:
                still.append(pos)
                continue

            if mb.resolved:
                yes_p = mb.yes_price
                no_p = 1.0 - yes_p
                exit_yes = yes_p
                exit_no = no_p
                exit_p = exit_yes if pos.side == "YES" else exit_no

                if pos.side == "YES":
                    pnl = (exit_yes - pos.entry_price) * pos.size
                else:
                    pnl = (pos.entry_price - exit_no) * pos.size

                fee = pos.value * TAKER_FEE_BPS / 10000
                net = pnl - fee
                self.capital += pos.value + net
                self.fees += fee

                if net > 0.001:
                    result = "win"
                    self.wins += 1
                elif net < -0.001:
                    result = "loss"
                    self.losses += 1
                else:
                    result = "breakeven"
                    self.breakeven += 1

                dur = (
                    datetime.now() - datetime.fromisoformat(pos.opened_at)
                ).total_seconds()
                t = PaperTrade(
                    id=self._id(),
                    market_id=pos.market_id,
                    question=pos.question,
                    side=pos.side,
                    entry_price=pos.entry_price,
                    exit_price=exit_p,
                    size=pos.size,
                    pnl=round(net, 6),
                    rebate=round(pos.value * MAKER_REBATE_BPS / 10000, 6),
                    fee=round(fee, 6),
                    opened_at=pos.opened_at,
                    closed_at=self._now(),
                    duration_secs=round(dur, 1),
                    result=result,
                    how="gamma_settlement",
                )
                self.trades.append(t)
                self._journal(t)
                self.log.info(
                    "[SETTLED] %s %dx entry=$%.4f exit=$%.4f pnl=$%.4f %s"
                    % (pos.side, pos.size, pos.entry_price, exit_p, net, result)
                )
            else:
                still.append(pos)
        self.positions = still

    def _open_orders(self):
        if self.capital < MIN_CAPITAL:
            self.log.info(
                "Capital $%.2f < $%.2f -- waiting" % (self.capital, MIN_CAPITAL)
            )
            return

        cands = self.scanner.get_candidates(self.genome)
        if not cands:
            return

        open_ids = set(p.market_id for p in self.positions)
        pending_ids = set(o["market_id"] for o in self.pending)
        n_open = len(open_ids)

        if n_open >= self.genome.max_positions:
            return

        for mb in cands:
            if n_open >= self.genome.max_positions:
                break
            if mb.id in open_ids or mb.id in pending_ids:
                continue

            yes_p = mb.yes_price
            no_p = 1.0 - yes_p

            spread = abs(yes_p - no_p)
            spr_pct = spread / max(yes_p, 0.001)
            bid_off = spr_pct * self.genome.bid_offset_bps / 10000
            ask_off = spr_pct * self.genome.ask_offset_bps / 10000

            yes_bid = yes_p * (1 - bid_off)
            no_bid = no_p * (1 - bid_off)
            yes_bid = max(yes_bid, 0.001)
            no_bid = max(no_bid, 0.001)

            order_val = max(
                min(
                    self.genome.min_position_size,
                    self.capital * self.genome.max_position_pct,
                ),
                1.0,
            )

            yes_sz = max(1, int(order_val / yes_bid))
            no_sz = max(1, int(order_val / no_bid))

            if self.genome.post_both_sides:
                self._place(mb, "YES", yes_bid, yes_sz, "maker_bid")
                self._place(mb, "NO", no_bid, no_sz, "maker_bid")
                n_open += 1
            else:
                side = "YES" if yes_p < 0.50 else "NO"
                price = yes_bid if side == "YES" else no_bid
                sz = yes_sz if side == "YES" else no_sz
                if self._place(mb, side, price, sz, "maker_bid"):
                    n_open += 1

    def _status(self):
        pos_v = sum(p.value for p in self.positions)
        pen_v = sum(o["value"] for o in self.pending)
        pnl = self.capital - self.paper_capital
        wr = self.wins / max(self.wins + self.losses, 1) * 100
        days = (
            datetime.now() - datetime.fromisoformat(self.started_at)
        ).total_seconds() / 86400
        daily = pnl / max(days, 0.001)
        self.log.info(
            "\n%s\nPAPER TRADER v4 | %s\n%s\n"
            "  Capital:  $%.2f  (started $%.2f)\n"
            "  PnL:     $%.4f  ($%.4f/day)\n"
            "  Positions: %d ($%.2f) | Pending: %d ($%.2f)\n"
            "  Trades:  %d (W=%d L=%d BE=%d) | WinRate: %.1f%%\n"
            "  Rebates: +$%.4f  Fees: -$%.4f\n"
            "  Genome:  %s\n"
            "%s"
            % (
                "=" * 56,
                datetime.now().strftime("%Y-%m-%d %H:%M"),
                "=" * 56,
                self.capital,
                self.paper_capital,
                pnl,
                daily,
                len(self.positions),
                pos_v,
                len(self.pending),
                pen_v,
                len(self.trades),
                self.wins,
                self.losses,
                self.breakeven,
                wr,
                self.rebates,
                self.fees,
                self.genome.name,
                "=" * 56,
            )
        )

    def run(self, poll: int = 60):
        self._running = True
        self._load()

        def stop(sig, frame):
            self.log.info("Stopping...")
            self._running = False

        signal.signal(signal.SIGINT, stop)
        signal.signal(signal.SIGTERM, stop)

        self.log.info(
            "Paper Trader v4 | $%s virtual | Genome: %s | poll=%ds"
            % (self.paper_capital, self.genome.name, poll)
        )

        tick = 0
        while self._running:
            try:
                tick += 1
                self.scanner.scan()
                self._tick_pending()
                self._resolve_positions()
                self._open_orders()

                if tick % 10 == 0:
                    self._status()

                self._save()
                time.sleep(poll)

            except Exception as e:
                self.log.error("Error: %s" % e)
                time.sleep(poll)

        self._save()
        self._status()
        self.log.info("Stopped.")


if __name__ == "__main__":
    import argparse
    from strategy_genome import StrategyGenome

    parser = argparse.ArgumentParser()
    parser.add_argument("--capital", type=float, default=PAPER_CAPITAL)
    parser.add_argument("--poll", type=int, default=30)
    args = parser.parse_args()

    gp = Path(__file__).parent / "state" / "best_genome.json"
    if gp.exists():
        with open(gp) as f:
            d = json.load(f)
        genome = StrategyGenome.from_dict(d["genome"])
        print("Loaded genome: %s" % genome.name)
    else:
        genome = StrategyGenome()
        genome.name = "default"
        genome.spread_multiplier = 2.2
        genome.bid_offset_bps = 10
        genome.ask_offset_bps = 10
        genome.fill_probability = 0.20
        genome.max_positions = 5
        genome.post_both_sides = True
        genome.min_liquidity = 1000
        genome.min_spread_bps = 50
        genome.min_volume_usd = 5000
        genome.min_position_size = 3.0
        genome.max_position_pct = 0.10
        genome.cancel_after_seconds = 600

    PaperTrader(genome, paper_capital=args.capital).run(poll=args.poll)
