#!/usr/bin/env python3
"""
Strategy Auto-Researcher.

Tests ALL combinations of:
- Timeframes: 5s, 10s, 15s, 30s, 1m, 3m, 5m, 15m
- Indicators: RSI, MACD, Bollinger, momentum score
- Entry strategies: breakout, mean_reversion, overbought_oversold, multi_tf_confirm
- Params: thresholds, stop loss, take profit, position size

Uses genetic algorithm to find the best strategy params.
Uses historical data (or generated realistic BTC data) for backtesting.

Usage:
    researcher = StrategyResearcher(candle_engine)
    best = researcher.research(n_generations=20, pop_size=30)
    print(best)
"""

import json
import math
import random
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

sys.path.insert(0, str(Path(__file__).parent))

PAPER_CAPITAL = 100.0
TAKER_FEE_BPS = 5.0
SATS = 100000000
HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
RESULTS_DIR = Path(os.path.join(HARVEY_HOME, "data", "arbitrage-agent", "v2", "state"))
BEST_PARAMS_FILE = RESULTS_DIR / "best_intraday_params.json"


class Trade:
    def __init__(self, side, entry_price, size, stop_loss, take_profit, opened_at_ts):
        self.side = side
        self.entry_price = entry_price
        self.size = size
        self.stop_loss = stop_loss
        self.take_profit = take_profit
        self.opened_at_ts = opened_at_ts
        self.closed_at_ts: Optional[int] = None
        self.exit_price: Optional[float] = None
        self.pnl: Optional[float] = None
        self.result: Optional[str] = None
        self.how: Optional[str] = None


STRATEGIES = {
    "breakout": "Entry when price breaks key level with momentum",
    "mean_reversion": "Entry when price reverts from extreme (RSI/BB)",
    "overbought_oversold": "Entry when RSI leaves overbought/oversold",
    "multi_tf_confirm": "Entry when multiple timeframes agree",
}


class StrategyParams:
    TIMEFRAMES = ["5s", "10s", "15s", "30s", "1m", "3m", "5m", "15m"]

    GRID = {
        "strategy": [
            "breakout",
            "mean_reversion",
            "overbought_oversold",
            "multi_tf_confirm",
        ],
        "timeframe": TIMEFRAMES,
        "rsi_period": [7, 14, 21],
        "rsi_oversold": [20, 30, 40],
        "rsi_overbought": [60, 70, 80],
        "macd_fast": [8, 12, 16],
        "macd_slow": [20, 26, 32],
        "bb_period": [10, 20, 30],
        "mom_th": [0.001, 0.002, 0.003, 0.005, 0.008, 0.01],
        "stop_loss_pct": [1.0, 2.0, 3.0, 5.0],
        "take_profit_pct": [1.0, 2.0, 3.0, 5.0],
        "size_pct": [0.001, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2],
        "max_hold_secs": [60, 120, 300, 600],
    }

    def __init__(self, overrides: Optional[Dict] = None):
        self._d: Dict[str, Any] = {}
        for key, vals in self.GRID.items():
            self._d[key] = random.choice(vals)
        if overrides:
            self._d.update(overrides)

    def get(self, key: str) -> Any:
        return self._d.get(key)

    def __getitem__(self, key: str) -> Any:
        return self._d[key]

    def __setitem__(self, key: str, val: Any):
        self._d[key] = val

    def to_dict(self) -> Dict:
        return dict(self._d)

    def mutate(self) -> "StrategyParams":
        p = StrategyParams(dict(self._d))
        key = random.choice(list(self.GRID.keys()))
        p._d[key] = random.choice(self.GRID[key])
        return p

    @classmethod
    def crossover(cls, a: "StrategyParams", b: "StrategyParams") -> "StrategyParams":
        d = {}
        for k in cls.GRID.keys():
            d[k] = random.choice([a._d[k], b._d[k]])
        return StrategyParams(d)


def score_params(
    params: StrategyParams,
    candles_by_tf: Dict[str, List],
    symbol: str = "BTC",
    debug: bool = False,
) -> Tuple[float, float, int, float]:
    """
    Backtest a strategy on historical candles.
    Returns (pnl, win_rate, n_trades, sharpe_ratio).
    """
    capital = PAPER_CAPITAL
    positions: List[Trade] = []
    closed_trades: List[Trade] = []
    W = params["timeframe"]
    cs = candles_by_tf.get(W, [])

    if len(cs) < 50:
        return 0.0, 0.5, 0, 0.0

    closes = [c.close for c in cs]
    strategy = params["strategy"]

    rsi_vals = _rsi(closes, params["rsi_period"])
    macd_line, signal_line, hist = _macd(
        closes, params["macd_fast"], params["macd_slow"]
    )
    bb_u, bb_m, bb_l = _bollinger(closes, params["bb_period"])

    if debug:
        print(f"  DEBUG: strategy={strategy} timeframe={W} mom_th={params['mom_th']}")
        print(f"  DEBUG: sample closes: {closes[:10]}")
        print(
            f"  DEBUG: first 5mom: {[(closes[i] - closes[i - 5]) / closes[i - 5] * 100 for i in range(5, 20)]}"
        )

    for i in range(20, min(len(cs), len(rsi_vals))):
        cur_price = cs[i].close
        ts = cs[i].ts

        if len(positions) < 5 and not any(p.side for p in positions):
            entry = _check_entry(
                params,
                strategy,
                i,
                cs,
                closes,
                rsi_vals,
                macd_line,
                hist,
                bb_u,
                bb_m,
                bb_l,
            )

            if entry:
                side, reason = entry
                dollar_size = capital * params["size_pct"]
                sz_sats = int(dollar_size / cur_price * SATS)
                pos_val = sz_sats * cur_price / SATS
                if pos_val > capital * 0.90 or sz_sats < 1:
                    pass
                else:
                    capital -= pos_val
                    trade = Trade(
                        side=side,
                        entry_price=cur_price,
                        size=sz_sats,
                        stop_loss=params["stop_loss_pct"],
                        take_profit=params["take_profit_pct"],
                        opened_at_ts=ts,
                    )
                    positions.append(trade)

        next_positions = []
        for pos in positions:
            pct = (cur_price - pos.entry_price) / pos.entry_price * 100
            if pos.side == "SHORT":
                pct = -pct

            exit_now = False
            how = ""
            if pct >= pos.take_profit:
                exit_now = True
                how = "tp"
            elif pct <= -pos.stop_loss:
                exit_now = True
                how = "sl"
            elif (ts - pos.opened_at_ts) / 1000 >= params["max_hold_secs"]:
                exit_now = True
                how = "time"

            if exit_now:
                sz_sats = pos.size
                pos_val = sz_sats * pos.entry_price / SATS
                pnl_dollars = sz_sats * pos.entry_price * pct / (100 * SATS)
                fee = pos_val * TAKER_FEE_BPS / 10000
                net = pnl_dollars - fee
                capital += pos_val + net
                result = "win" if net > 0.01 else "loss" if net < -0.01 else "breakeven"
                pos.closed_at_ts = ts
                pos.exit_price = cur_price
                pos.pnl = net
                pos.result = result
                pos.how = how
                closed_trades.append(pos)
            else:
                next_positions.append(pos)
        positions = next_positions

    if closed_trades:
        wins = sum(1 for t in closed_trades if t.result == "win")
        losses = sum(1 for t in closed_trades if t.result == "loss")
        wr = wins / (wins + losses) if (wins + losses) > 0 else 0.5
        pnls = [t.pnl for t in closed_trades]
        mean_pnl = sum(pnls) / len(pnls)
        std_pnl = math.sqrt(sum((p - mean_pnl) ** 2 for p in pnls) / max(len(pnls), 1))
        sharpe = (
            (mean_pnl / max(std_pnl, 0.001)) * math.sqrt(252 * 24 * 60)
            if std_pnl > 0
            else 0
        )
        final_pnl = capital - PAPER_CAPITAL
        return round(final_pnl, 4), round(wr, 3), len(closed_trades), round(sharpe, 4)

    final_pnl = capital - PAPER_CAPITAL
    return round(final_pnl, 4), 0.5, 0, 0.0


def _check_entry(
    params, strategy, i, cs, closes, rsi_vals, macd_line, hist, bb_u, bb_m, bb_l
):
    if strategy == "breakout":
        if i < 5:
            return None
        mom = (closes[i] - closes[i - 5]) / closes[i - 5] * 100
        vol = (
            (max(closes[i - 5 : i + 1]) - min(closes[i - 5 : i + 1]))
            / closes[i - 5]
            * 100
        )
        if mom > params["mom_th"] and vol > params["mom_th"] * 0.3:
            return ("LONG", "breakout")
        elif mom < -params["mom_th"] and vol > params["mom_th"] * 0.3:
            return ("SHORT", "breakdown")

    elif strategy == "mean_reversion":
        if not rsi_vals or len(rsi_vals) < params["rsi_period"] + 1:
            return None
        rsi = rsi_vals[i]
        cur = closes[i]
        bb_upper = (
            bb_u[i - params["bb_period"] + 1]
            if bb_u and i >= params["bb_period"] - 1
            else cur * 1.02
        )
        bb_lower = (
            bb_l[i - params["bb_period"] + 1]
            if bb_l and i >= params["bb_period"] - 1
            else cur * 0.98
        )

        if rsi < params["rsi_oversold"] and cur <= bb_lower * 1.01:
            return ("LONG", "oversold_revert")
        elif rsi > params["rsi_overbought"] and cur >= bb_upper * 0.99:
            return ("SHORT", "overbought_revert")

    elif strategy == "overbought_oversold":
        if not rsi_vals or len(rsi_vals) < params["rsi_period"] + 1:
            return None
        rsi = rsi_vals[i]
        if rsi < params["rsi_oversold"]:
            return ("LONG", "rsi_oversold")
        elif rsi > params["rsi_overbought"]:
            return ("SHORT", "rsi_overbought")

    elif strategy == "multi_tf_confirm":
        rsi_idx = i
        if (
            not rsi_vals
            or len(rsi_vals) <= rsi_idx
            or not macd_line
            or len(macd_line) < 5
        ):
            return None
        rsi = rsi_vals[rsi_idx]
        macd_h = hist[-1] if hist else 0
        mom = (closes[i] - closes[i - 10]) / closes[i - 10] * 100 if i >= 10 else 0

        if rsi < params["rsi_oversold"] and macd_h > 0 and mom > params["mom_th"]:
            return ("LONG", "multi_tf_bullish")
        elif rsi > params["rsi_overbought"] and macd_h < 0 and mom < -params["mom_th"]:
            return ("SHORT", "multi_tf_bearish")

    return None


def _rsi(values, period):
    if len(values) < period + 1:
        return []
    changes = [values[i] - values[i - 1] for i in range(1, len(values))]
    gains = [max(c, 0) for c in changes]
    losses = [-min(c, 0) for c in changes]
    avg_gain = sum(gains[:period]) / period
    avg_loss = sum(losses[:period]) / period
    result = [50.0]
    for i in range(period, len(changes)):
        avg_gain = (avg_gain * (period - 1) + gains[i]) / period
        avg_loss = (avg_loss * (period - 1) + losses[i]) / period
        if avg_loss == 0:
            result.append(100.0)
        else:
            rs = avg_gain / avg_loss
            result.append(100 - (100 / (1 + rs)))
    return result


def _macd(values, fast, slow):
    def ema(vals, n):
        if len(vals) < n:
            return []
        k = 2 / (n + 1)
        result = [sum(vals[:n]) / n]
        for v in vals[n:]:
            result.append(v * k + result[-1] * (1 - k))
        return result

    if len(values) < slow + 1:
        return [], [], []
    ef = ema(values, fast)
    es = ema(values, slow)
    macd_line = [ef[i] - es[i] for i in range(len(es))]
    sig = ema(macd_line, 9)
    h = []
    off = len(macd_line) - len(sig)
    for i in range(len(sig)):
        h.append(macd_line[i + off] - sig[i])
    return macd_line, sig, h


def _bollinger(values, period, std=2.0):
    if len(values) < period:
        return [], [], []
    result = []
    for i in range(period - 1, len(values)):
        window = values[i - period + 1 : i + 1]
        mean = sum(window) / period
        sd = math.sqrt(sum((v - mean) ** 2 for v in window) / period)
        result.append((mean + std * sd, mean, mean - std * sd))
    u, m, l = zip(*result)
    return list(u), list(m), list(l)


class StrategyResearcher:
    """Genetic algorithm strategy optimizer."""

    def __init__(self, candle_engine=None):
        self.engine = candle_engine
        self.history: List[Dict] = []
        self.best: Optional[StrategyParams] = None
        self.best_score = -999.0

    def generate_historical_candles(self, n_candles: int = 500) -> Dict[str, List]:
        """Generate realistic BTC candles for backtesting (when no live data)."""
        candles_by_tf: Dict[str, List] = {}
        tf_seconds = {
            "5s": 5,
            "10s": 10,
            "15s": 15,
            "30s": 30,
            "1m": 60,
            "3m": 180,
            "5m": 300,
            "15m": 900,
        }
        base_price = 67500.0
        rng = random.Random(42)

        max_ticks_needed = n_candles * max(
            tf_seconds.values()
        )  # 500 * 900 = 450k ticks max
        btc_path = [base_price]
        for _ in range(max_ticks_needed + 200):
            pct = rng.gauss(0, 1) * 0.0001
            trend = rng.choice([-0.00001, 0, 0.00001])
            btc_path.append(btc_path[-1] * (1 + trend + pct))

        for tf, sec in tf_seconds.items():
            cs = []
            for i in range(0, n_candles * sec, sec):
                window = btc_path[i : i + sec]
                if window:
                    cs.append(
                        type(
                            "C",
                            (),
                            {
                                "ts": 1700000000000 + i * 1000,
                                "open": window[0],
                                "high": max(window),
                                "low": min(window),
                                "close": window[-1],
                                "volume": rng.uniform(0.1, 10.0),
                                "timeframe": tf,
                                "symbol": "btcusdt",
                            },
                        )()
                    )
            candles_by_tf[tf] = cs

        return candles_by_tf

    def research(self, n_generations: int = 10, pop_size: int = 30) -> Dict:
        candles = {}
        if self.engine:
            try:
                for tf in StrategyParams.TIMEFRAMES:
                    candles[tf] = self.engine.candles("btcusdt", tf, 1000)
                if not any(candles.values()):
                    raise ValueError("No live candles")
                print(
                    f"  Using live data: {sum(len(v) for v in candles.values())} candles"
                )
            except Exception:
                print("  Falling back to generated historical data")
                candles = self.generate_historical_candles()
        else:
            candles = self.generate_historical_candles()

        population = [StrategyParams() for _ in range(pop_size)]
        best_params = None
        best_score = -999.0

        counter = [0]
        for gen in range(n_generations):
            scored = []
            for params in population:
                try:
                    pnl, wr, n_trades, sharpe = score_params(params, candles)
                    if n_trades < 5:
                        score = -10
                    else:
                        score = pnl * 10 + sharpe * 5 + (wr - 0.5) * 10
                    scored.append(
                        (score, counter[0], pnl, wr, n_trades, sharpe, params)
                    )
                    counter[0] += 1
                except Exception:
                    pass

            scored.sort(reverse=True)
            if not scored:
                continue

            top_score, _cid, top_pnl, top_wr, top_n, top_sharpe, top_params = scored[0]
            print(
                f"  Gen {gen + 1}: P&L=${top_pnl:+.2f} WR={top_wr:.0%} trades={top_n} sharpe={top_sharpe:.2f} score={top_score:.2f} strat={top_params['strategy']} tf={top_params['timeframe']}"
            )

            if top_score > best_score:
                best_score = top_score
                best_params = top_params

            next_pop = [top_params]
            elites = [s[6] for s in scored[: max(2, pop_size // 5)]]
            next_pop.extend(elites[:3])

            while len(next_pop) < pop_size:
                if random.random() < 0.7 and len(elites) >= 2:
                    p1, p2 = random.sample(elites, 2)
                    child = StrategyParams.crossover(p1, p2)
                else:
                    child = StrategyParams()
                if random.random() < 0.3:
                    child = child.mutate()
                next_pop.append(child)
            population = next_pop[:pop_size]

        print(f"\nBest: {best_params.to_dict()}")
        print(f"Score: {best_score:.2f}")

        RESULTS_DIR.mkdir(parents=True, exist_ok=True)

        # Save for strategy_researcher (raw genetic output)
        with open(RESULTS_DIR / "best_strategy.json", "w") as f:
            json.dump(
                {
                    "params": best_params.to_dict(),
                    "score": best_score,
                    "timestamp": datetime.now().isoformat(),
                },
                f,
                indent=2,
            )

        # Save in live trader format (best_intraday_params.json)
        # Include ALL params the live trader needs
        live_params = best_params.to_dict()
        live_params.update(
            {
                "window": 5,
                "vol_th": 0.1,
                "allow_breakout": True,
                "allow_breakdown": True,
                "allow_dip_buy": True,
                "allow_rip_sell": False,
                "symbols": ["btcusdt"],
            }
        )
        with open(BEST_PARAMS_FILE, "w") as f:
            json.dump(
                {
                    "params": live_params,
                    "score": best_score,
                    "source": "strategy_researcher genetic optimizer",
                    "journal_stats": {},
                    "timestamp": datetime.now().isoformat(),
                },
                f,
                indent=2,
            )

        return best_params.to_dict()


if __name__ == "__main__":
    print("=" * 60)
    print("STRATEGY AUTO-RESEARCHER")
    print(f"Time: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print("=" * 60)

    researcher = StrategyResearcher()
    print("\n[1/2] Generating historical BTC data...")
    candles = researcher.generate_historical_candles()
    print(f"  Generated {sum(len(v) for v in candles.values())} candles")

    print("\n[2/2] Running genetic optimization...")
    best = researcher.research(n_generations=10, pop_size=30)

    print(f"\nBest strategy: {best['strategy']} on {best['timeframe']}")
    print(
        f"RSI: period={best['rsi_period']} OB={best['rsi_overbought']} OS={best['rsi_oversold']}"
    )
    print(f"MACD: {best['macd_fast']}/{best['macd_slow']}")
    print(f"BB period: {best['bb_period']}")
    print(f"Stop: {best['stop_loss_pct']}% | Take profit: {best['take_profit_pct']}%")
