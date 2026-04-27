#!/usr/local/Cellar/python@3.11/3.11.10/Frameworks/Python.framework/Versions/3.11/bin/python3.11
"""
BTC Sniper — Fast Paper Mode
============================
Paper trading with relaxed thresholds to generate MAXIMUM trades rapidly.
Fires on delta>=8 (79% WR historically) and conf>=0.4 to collect data fast.
Logs every trade to intraday_journal.jsonl for GA analysis.
"""

import copy, json, os, random, signal, sys, time, requests, threading
from collections import defaultdict
from dataclasses import dataclass, asdict
from datetime import datetime
from pathlib import Path
from typing import Optional

HARVEY_HOME = Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "arbitrage-agent" / "v2"
STATE_DIR = DATA_DIR / "state"
LOG_DIR = DATA_DIR / "logs"
JOURNAL_FILE = STATE_DIR / "intraday_journal.jsonl"
BEST_PARAMS_FILE = STATE_DIR / "sniper_best_params.json"
PAPER_CAPITAL = 100.0
MIN_SPEND = 2.50
TAKER_FEE_BPS = 200
POLYFEE = 0.01
BINANCE_REST = "https://api.binance.com/api/v3/ticker/price?symbol=BTCUSDT"
BINANCE_KLINES = "https://api.binance.com/api/v3/klines"
GAMMA_API = "https://gamma-api.polymarket.com"

STATE_DIR.mkdir(parents=True, exist_ok=True)
LOG_DIR.mkdir(parents=True, exist_ok=True)


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_DIR / "btc_sniper_paper_fast.log", "a") as f:
        f.write(line + "\n")


@dataclass
class SniperParams:
    version: str = "pro1.0"
    name: str = "paper_fast"
    delta_thresh: float = 8.0
    conf_thresh: float = 0.40
    ens_thresh: float = 0.50
    spend_ratio: float = 0.30
    max_bet_pct: float = 0.40
    max_hold_seconds: int = 300
    min_market_volume: int = 0
    max_spread_bps: int = 2000
    pop_size: int = 12

    def to_dict(self):
        return asdict(self)

    def from_dict(d):
        return SniperParams(
            **{k: v for k, v in d.items() if k in asdict(SniperParams())}
        )

    def mutate(self, rate=0.4):
        import copy

        p = copy.deepcopy(self)
        p.name = f"gen_{int(time.time()) % 1000000}_{random.randint(1000, 9999)}"
        for attr in [
            "delta_thresh",
            "conf_thresh",
            "ens_thresh",
            "spend_ratio",
            "max_bet_pct",
        ]:
            if random.random() < rate:
                delta = getattr(p, attr) * 0.3
                setattr(
                    p, attr, max(0.01, getattr(p, attr) + random.uniform(-delta, delta))
                )
        p.conf_thresh = max(0.1, min(0.9, p.conf_thresh))
        p.delta_thresh = max(4, min(20, p.delta_thresh))
        return p


# ── Data fetchers ──────────────────────────────────────────────────────────
def get_btc_price():
    try:
        r = requests.get(BINANCE_REST, timeout=5)
        return float(r.json()["price"])
    except:
        return None


def get_klines(interval="5m", limit=30):
    try:
        params = {"symbol": "BTCUSDT", "interval": interval, "limit": limit}
        r = requests.get(BINANCE_KLINES, params=params, timeout=10)
        data = r.json()
        closes = [float(k[4]) for k in data]
        highs = [float(k[2]) for k in data]
        lows = [float(k[3]) for k in data]
        vols = [float(k[5]) for k in data]
        return closes, highs, lows, vols
    except:
        return [None] * 30, [None] * 30, [None] * 30, [None] * 30


def get_btc_now():
    now_ts = int(time.time())
    window_sec = 300
    current_window = (now_ts // window_sec) * window_sec
    return current_window, get_btc_price()


def fetch_btc_markets(tf_minutes=5):
    now_ts = int(time.time())
    window_sec = tf_minutes * 60
    current_window = (now_ts // window_sec) * window_sec
    slug_prefix = f"btc-updown-{tf_minutes}m"
    for offset in [0, 1, 2]:
        window = current_window + offset * window_sec
        slug = f"{slug_prefix}-{window}"
        try:
            r = requests.get(f"{GAMMA_API}/markets", params={"slug": slug}, timeout=8)
            if r.status_code != 200:
                continue
            data = r.json()
            markets = data if isinstance(data, list) else data.get("data", [])
            if not markets:
                continue
            m = markets[0]
            if not m.get("acceptingOrders", False):
                continue
            if m.get("closed", True):
                continue
            return m
        except:
            continue
    return None


def fetch_market_resolution(market_id):
    try:
        r = requests.get(f"{GAMMA_API}/markets/{market_id}", timeout=8)
        if r.status_code != 200:
            return None
        m = r.json()
        prices_raw = m.get("outcomePrices", "[]")
        prices = json.loads(prices_raw) if isinstance(prices_raw, str) else prices_raw
        labels_raw = m.get("outcomes", '["Up","Down"]')
        labels = json.loads(labels_raw) if isinstance(labels_raw, str) else labels_raw
        for i, p in enumerate(prices):
            try:
                if float(p) >= 0.99:
                    lbl = labels[i] if i < len(labels) else ""
                    if str(lbl).lower() in ("up", "yes"):
                        return "Up"
                    elif str(lbl).lower() in ("down", "no"):
                        return "Down"
            except:
                pass
        return None
    except:
        return None


# ── Signal Engine ──────────────────────────────────────────────────────────
class SignalEngine:
    def __init__(self):
        self.ph = []
        self.btc_price = 0
        self.prices_5m = [None] * 30
        self.highs_5m = [None] * 30
        self.lows_5m = [None] * 30
        self.vols_5m = [None] * 30
        self.prices_1m = [None] * 20

    def update(self, btc_price):
        self.btc_price = btc_price
        now_ts = int(time.time())
        self.ph.append((now_ts, btc_price))
        if len(self.ph) > 120:
            self.ph = self.ph[-120:]
        closes, highs, lows, vols = get_klines("5m", 30)
        if closes[0] is not None:
            self.prices_5m = closes
            self.highs_5m = highs
            self.lows_5m = lows
            self.vols_5m = vols
        closes_1m, _, _, _ = get_klines("1m", 20)
        if closes_1m[0] is not None:
            self.prices_1m = closes_1m

    def ensemble(self, ens_thresh, window_delta):
        if len(self.ph) < 20:
            return {
                "direction": "Neutral",
                "conf": 0.0,
                "reasons": [],
                "conditions": {},
            }
        conf_ups, conf_downs = [], []
        reasons = []

        # Window delta (primary)
        if window_delta >= 15:
            conf_ups.append(0.80)
        elif window_delta >= 10:
            conf_ups.append(0.65)
        elif window_delta >= 8:
            conf_ups.append(0.55)
        elif window_delta >= 5:
            conf_ups.append(0.30)
        if window_delta <= -15:
            conf_downs.append(0.80)
        elif window_delta <= -10:
            conf_downs.append(0.65)
        elif window_delta <= -8:
            conf_downs.append(0.55)
        elif window_delta <= -5:
            conf_downs.append(0.30)

        # RSI 14
        if len(self.prices_5m) >= 15:
            deltas = [
                self.prices_5m[i] - self.prices_5m[i - 1]
                for i in range(1, len(self.prices_5m))
            ]
            gains = [d for d in deltas[-14:] if d > 0]
            losses = [-d for d in deltas[-14:] if d < 0]
            ag = sum(gains) / 14 if gains else 0
            al = sum(losses) / 14 if losses else 1e-9
            rs = ag / al
            rsi = 100 - (100 / (1 + rs))
            if rsi < 35 and conf_ups:
                conf_ups.append(0.10)
            elif rsi > 65 and conf_downs:
                conf_downs.append(0.10)

        # MACD (12, 26, 9)
        if len(self.prices_5m) >= 26:
            ema12 = self._ema(self.prices_5m, 12)
            ema26 = self._ema(self.prices_5m, 26)
            macd = ema12 - ema26
            signal_line = self._ema([macd] * len(self.prices_5m[-9:]), 9)
            if macd > signal_line and conf_ups:
                conf_ups.append(0.08)
            elif macd < signal_line and conf_downs:
                conf_downs.append(0.08)

        total_conf = max(sum(conf_ups), sum(conf_downs))
        direction = (
            "Up"
            if sum(conf_ups) > sum(conf_downs)
            else "Down"
            if conf_downs
            else "Neutral"
        )
        return {
            "direction": direction,
            "conf": total_conf,
            "reasons": reasons,
            "conditions": {"window_delta": window_delta},
        }

    def _ema(self, data, n):
        if len(data) < n:
            return data[-1] if data else 0
        k = 2 / (n + 1)
        ema = sum(data[:n]) / n
        for v in data[n:]:
            ema = v * k + ema * (1 - k)
        return ema


# ── Fast GA ────────────────────────────────────────────────────────────────
class FastGA:
    def __init__(self):
        self.best_params = SniperParams()
        self.best_score = float("-inf")
        self.population = []

    def run(self, journal_file, generations=20, session_min=5):
        log("### FAST GA STARTING ###")
        # Load paper trades as baseline
        trades = self._load_trades(journal_file)
        log(f"Loaded {len(trades)} paper trades for GA")

        # Start with proven good params
        base = SniperParams(
            delta_thresh=10.0,
            conf_thresh=0.45,
            ens_thresh=0.50,
            spend_ratio=0.30,
            name="ga_baseline",
        )
        self.population = [base] + [base.mutate(0.5) for _ in range(19)]

        for gen in range(generations):
            results = []
            for p in self.population:
                score = self._score_params(p, trades)
                results.append((score, p))
                log(
                    f"  Gen{gen + 1} {p.name}: score={score:.2f} delta={p.delta_thresh:.1f} conf={p.conf_thresh:.2f}"
                )

            results.sort(key=lambda x: x[0], reverse=True)
            best_score, best_p = results[0]
            log(
                f"Gen {gen + 1}/{generations}: BEST {best_p.name} score={best_score:.2f} delta={best_p.delta_thresh:.1f} conf={best_p.conf_thresh:.2f}"
            )

            if best_score > self.best_score:
                self.best_score = best_score
                self.best_params = copy.deepcopy(best_p)
                self.best_params.name = f"ga_best_gen{gen + 1}"
                self._save()
                log(f"  🏆 NEW BEST: score={best_score:.2f}")

            # Selection + mutation
            elite = [p for _, p in results[:5]]
            next_pop = list(elite)
            while len(next_pop) < 20:
                parent = random.choice(elite)
                child = parent.mutate(rate=0.5)
                next_pop.append(child)
            self.population = next_pop[:20]

            trades = self._load_trades(journal_file)  # Refresh with new trades

        log(
            f"### FAST GA DONE. Best: delta={self.best_params.delta_thresh:.1f} conf={self.best_params.conf_thresh:.2f} score={self.best_score:.2f}"
        )

    def _load_trades(self, journal_file):
        trades = []
        try:
            with open(journal_file) as f:
                for line in f:
                    try:
                        trades.append(json.loads(line))
                    except:
                        pass
        except:
            pass
        return [t for t in trades if t.get("mode") == "paper"]

    def _score_params(self, params, trades):
        if not trades:
            return 0.0
        wins = sum(1 for t in trades if t.get("won"))
        losses = len(trades) - wins
        wr = wins / len(trades) if trades else 0
        pnl = sum(t.get("pnl", 0) for t in trades)
        # Score: penalize if conf_thresh too low (too many bad trades) or delta_thresh too low
        conf_penalty = max(0, 0.4 - params.conf_thresh) * 50
        delta_penalty = max(0, 8 - params.delta_thresh) * 2
        score = pnl * 10 + wr * 30 - conf_penalty - delta_penalty
        return score

    def _save(self):
        try:
            d = self.best_params.to_dict()
            d["best_score"] = self.best_score
            d["generation"] = int(time.time())
            with open(BEST_PARAMS_FILE, "w") as f:
                json.dump(d, f, indent=2)
        except Exception as e:
            log(f"GA save error: {e}")


# ── Paper Trader ────────────────────────────────────────────────────────────
class PaperTrader:
    def __init__(self, params=None):
        self.params = params or SniperParams()
        self.live = False
        self.bankroll = PAPER_CAPITAL
        self._balance = PAPER_CAPITAL
        self.starting = PAPER_CAPITAL
        self.wins = 0
        self.losses = 0
        self.total_pnl = 0.0
        self.blocks = 0
        self.t0 = time.time()
        self.se_5m = SignalEngine()
        self.se_15m = SignalEngine()
        self.windows = {
            5: {"start": 0, "price": 0, "traded": False, "market_id": None},
            15: {"start": 0, "price": 0, "traded": False, "market_id": None},
        }
        self.trades = []
        self.running = True

    def run(self, duration=28800):
        log(
            f"Starting PAPER sniper: delta>={self.params.delta_thresh}, conf>={self.params.conf_thresh}"
        )
        signal.signal(signal.SIGINT, lambda s, f: setattr(self, "running", False))
        last_market_check = {5: 0, 15: 0}
        last_status = 0
        last_btc = 0

        while self.running and (time.time() - self.t0) < duration:
            now = time.time()
            btc = get_btc_price()
            if btc is None:
                time.sleep(1)
                continue

            if abs(btc - last_btc) > 1:
                self.se_5m.update(btc)
                self.se_15m.update(btc)
                last_btc = btc

            # Check signals every second for each timeframe
            for tf in [5, 15]:
                if now - last_market_check.get(tf, 0) < 5:
                    continue
                win = self.windows[tf]

                # Refetch market
                mkt = fetch_btc_markets(tf)
                if mkt:
                    from datetime import datetime as dt_cls

                    ts_str = mkt.get("endDate", "") or mkt.get("endDate_iso", "")
                    try:
                        dt = dt_cls.fromisoformat(ts_str.replace("Z", "+00:00"))
                        ws = int(dt.timestamp())
                    except:
                        ws = (now // (tf * 60)) * (tf * 60) + tf * 60
                    if not win["start"] or ws != win["start"]:
                        win["start"] = ws - tf * 60
                        win["price"] = btc
                        win["traded"] = False
                        win["market_id"] = mkt.get("id")

                    # Check trade
                    window_delta = btc - win["price"]
                    if (
                        abs(window_delta) >= self.params.delta_thresh
                        and not win["traded"]
                        and win["market_id"]
                    ):
                        sig = self.se_5m.ensemble(self.params.ens_thresh, window_delta)
                        if (
                            sig["direction"] != "Neutral"
                            and sig["conf"] >= self.params.conf_thresh
                        ):
                            pnl = self._place_trade(sig, win, tf, btc)
                            if pnl is not None:
                                win["traded"] = True
                                self.total_pnl += pnl
                                if pnl > 0:
                                    self.wins += 1
                                else:
                                    self.losses += 1
                                log(
                                    f"  🟡 BET {tf}m: {sig['direction']} ΔBTC={window_delta:+.0f} conf={sig['conf']:.2f} pnl={pnl:+.2f}"
                                )

                last_market_check[tf] = now

            # Resolve trades
            resolved = []
            for t in self.trades:
                if t.get("resolved"):
                    continue
                tf_sec = t["window_tf"] * 60
                if now >= t["window_start"] + tf_sec + 10:
                    actual = (
                        fetch_market_resolution(t["market_id"])
                        if t.get("market_id")
                        else None
                    )
                    if actual:
                        won = actual == t["direction"]
                        pnl = self._calc_pnl(
                            won, t["size"], t["poly_price"], t["direction"]
                        )
                        t["resolved"] = True
                        t["won"] = won
                        t["pnl"] = pnl
                        self._journal_trade(t)
                        log(
                            f"  {'🟢' if won else '🔴'} RESOLVED {t['direction']}: {'WIN' if won else 'LOSS'} pnl={pnl:+.2f}"
                        )
                    elif now >= t["window_start"] + tf_sec + 600:
                        t["resolved"] = True
                        t["won"] = False
                        t["pnl"] = -abs(t["size"] * t["poly_price"])
                        self._journal_trade(t)
                        log(f"  🔴 TIMEOUT LOSS pnl={t['pnl']:+.2f}")

            # Status
            if now - last_status >= 30:
                tt = self.wins + self.losses
                wr = self.wins / tt if tt > 0 else 0
                pnl_pct = (self._balance - self.starting) / self.starting * 100
                log(
                    f"[{datetime.fromtimestamp(now).strftime('%H:%M:%S')}] elapsed={(now - self.t0) / 3600:.1f}h trades={tt}(W:{self.wins} L:{self.losses}) WR={wr:.0%} Bk=${self._balance:.2f}({pnl_pct:+.1f}%) BTC=${btc:.0f}"
                )
                last_status = now

            time.sleep(0.5)

        tt = self.wins + self.losses
        log(
            f"\nFINAL: {tt} trades, {self.wins}W/{self.losses}L, PnL=${self.total_pnl:+.2f}"
        )

    def _place_trade(self, sig, win, tf, btc):
        direction = sig["direction"]
        prices_raw = win["market_id"]  # placeholder
        poly_price = 0.50

        # Fetch market for price
        try:
            mkt = fetch_btc_markets(tf)
            if mkt:
                op = json.loads(mkt.get("outcomePrices", "[]"))
                if len(op) >= 2:
                    poly_price = float(op[1]) if direction == "Up" else float(op[0])
        except:
            pass

        spend = self._balance * self.params.spend_ratio
        size = spend / poly_price
        if size < 5:
            size = 5.0
        cost = size * poly_price
        if cost > self._balance * 0.95:
            size = self._balance * 0.95 / poly_price
        cost = size * poly_price

        trade = {
            "direction": direction,
            "size": size,
            "poly_price": poly_price,
            "spend": cost,
            "btc_price_enter": btc,
            "btc_delta": btc - win["price"],
            "conf": sig["conf"],
            "window_start": win["start"],
            "window_tf": tf,
            "market_id": win.get("market_id"),
            "resolved": False,
            "placed_at": datetime.now().isoformat(),
        }
        self.trades.append(trade)
        self._balance -= cost
        return 0  # PnL calculated at resolution

    def _calc_pnl(self, won, size, poly_price, direction):
        if won:
            winnings = (
                size * (1.0 - poly_price) - size * poly_price * TAKER_FEE_BPS / 10000
            )
            return winnings - size * poly_price * POLYFEE
        else:
            return -size * poly_price

    def _journal_trade(self, t):
        try:
            d = {
                "mode": "paper",
                "params": self.params.to_dict(),
                "window_start": t["window_start"],
                "window_tf": t["window_tf"],
                "direction": t["direction"],
                "spend": t["spend"],
                "poly_price": t["poly_price"],
                "btc_delta": t["btc_delta"],
                "btc_price_enter": t["btc_price_enter"],
                "conf": t["conf"],
                "won": t["won"],
                "pnl": t["pnl"],
                "placed_at": t.get("placed_at"),
            }
            with open(JOURNAL_FILE, "a") as f:
                f.write(json.dumps(d) + "\n")
        except Exception as e:
            log(f"Journal error: {e}")


if __name__ == "__main__":
    duration = int(sys.argv[1]) if len(sys.argv) > 1 else 28800

    # Run fast GA first (uses existing journal data)
    ga = FastGA()
    ga_thread = threading.Thread(target=ga.run, args=(JOURNAL_FILE, 20, 5), daemon=True)
    ga_thread.start()

    # Run paper sniper
    trader = PaperTrader()
    trader.run(duration=duration)
