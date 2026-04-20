#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Sniper Strategy V3 — Polymarket Directional Trading
=======================================================
Autoresearch experiment: maximize PnL/hr using real Binance data.
Only this file is modified during autoresearch iterations.
"""

import json, time, math, os, sys, requests
from datetime import datetime, timezone
from collections import defaultdict

BINANCE_KLINES = "https://api.binance.com/api/v3/klines"
MIN_SPEND = 1.05
MAX_SPEND_RATIO = 0.10
POLYFEE = 0.01


def poly_price(delta: float, price: float) -> float:
    SPREAD = 0.02
    return 0.50 - SPREAD


class SignalEngine:
    def __init__(self):
        self.ph = []

    def update(self, price: float, ts: float):
        self.ph.append((ts, price))
        if len(self.ph) > 300:
            self.ph.pop(0)

    def _ema(self, data, n):
        k = 2 / (n + 1)
        r = [data[0]]
        for p in data[1:]:
            r.append(p * k + r[-1] * (1 - k))
        return r

    def rsi(self, n=14):
        if len(self.ph) < n + 1:
            return 0.0, 0.0
        ds = [self.ph[i][1] - self.ph[i - 1][1] for i in range(1, len(self.ph))]
        g = [d for d in ds[-n:] if d > 0]
        l = [-d for d in ds[-n:] if d < 0]
        ag = sum(g) / n if g else 0
        al = sum(l) / n if l else 1e-9
        r = 100 - (100 / (1 + ag / al))
        if r < 30:
            return 1.0, 0.0
        elif r > 70:
            return 0.0, 1.0
        return 0.0, 0.0

    def mom(self, n=10):
        if len(self.ph) < n + 1:
            return 0.0, 0.0
        cur = self.ph[-1][1]
        past = self.ph[-n][1]
        pct = (cur - past) / past
        if pct > 0.003:
            return min(1.0, pct / 0.01), 0.0
        elif pct < -0.003:
            return 0.0, min(1.0, abs(pct) / 0.01)
        return 0.0, 0.0

    def vol_ratio(self):
        if len(self.ph) < 30:
            return 0.0, 0.0
        recent = sum(p for _, p in self.ph[-15:]) / 15
        older = sum(p for _, p in self.ph[-30:-15]) / 15
        ratio = recent / (older + 1e-9)
        if ratio > 1.3:
            return min(1.0, (ratio - 1.3) / 0.7), 0.0
        elif ratio < 0.7:
            return 0.0, min(1.0, (0.7 - ratio) / 0.7)
        return 0.0, 0.0

    def bollinger(self, n=20, std=2.0):
        if len(self.ph) < n:
            return 0.0, 0.0
        prices = [p for _, p in self.ph[-n:]]
        mid = sum(prices) / n
        std_v = math.sqrt(sum((p - mid) ** 2 for p in prices) / n)
        cur = self.ph[-1][1]
        if cur < mid - std * std_v:
            return 1.0, 0.0
        elif cur > mid + std * std_v:
            return 0.0, 1.0
        return 0.0, 0.0

    def ema_cross(self):
        if len(self.ph) < 26:
            return 0.0, 0.0
        prices = [p for _, p in self.ph]
        e12 = self._ema(prices, 12)[-1]
        e26 = self._ema(prices, 26)[-1]
        diff = e12 - e26
        pct = abs(diff) / (prices[-1] + 1e-9)
        if pct < 0.0003:
            return 0.0, 0.0
        return (
            (min(1.0, pct / 0.001), 0.0) if diff > 0 else (0.0, min(1.0, pct / 0.001))
        )

    def all(self):
        rsi_up, rsi_dn = self.rsi()
        mom_up, mom_dn = self.mom()
        ema_up, ema_dn = self.ema_cross()

        up = 1.5 * rsi_up + 1.6 * mom_up + 1.1 * ema_up
        down = 1.5 * rsi_dn + 1.6 * mom_dn + 1.1 * ema_dn
        total = 3.0 + 3.2 + 2.2

        if up - down > total * 0.080:
            dir = "Up"
        elif down - up > total * 0.04:
            dir = "Down"
        else:
            dir = "Neutral"

        conf = abs(up - down) / total
        return {
            "direction": dir,
            "confidence": conf,
            "up": up / total,
            "down": down / total,
        }


class SimEngine:
    WARMUP = 30

    def __init__(
        self,
        dur=3600,
        bankroll=100.0,
        delta_thresh=5.0,
        conf_thresh=0.10,
        ens_thresh=0.08,
        spend_ratio=0.10,
        interval="1m",
        multi_trade=False,
        trade_cooldown=0,
    ):
        self.dur = dur
        self.bankroll = bankroll
        self.starting = bankroll
        self.delta_thresh = delta_thresh
        self.conf_thresh = conf_thresh
        self.ens_thresh = ens_thresh
        self.spend_ratio = spend_ratio
        self.interval = interval
        self.multi_trade = multi_trade
        self.trade_cooldown = trade_cooldown
        self.se = SignalEngine()
        self.trades = []
        self.wins = self.losses = self.blocks = 0
        self.total_pnl = 0.0
        self.t0 = time.time()

    def _fetch(self, start, end):
        candles = []
        cur = start
        while cur < end:
            try:
                r = requests.get(
                    BINANCE_KLINES,
                    timeout=30,
                    params={
                        "symbol": "BTCUSDT",
                        "interval": self.interval,
                        "startTime": int(cur * 1000),
                        "endTime": int(end * 1000),
                        "limit": 1000,
                    },
                )
                r.raise_for_status()
                ks = r.json()
                if not ks:
                    break
                for k in ks:
                    candles.append(
                        {
                            "t": float(k[0]) / 1000,
                            "o": float(k[1]),
                            "c": float(k[4]),
                            "h": float(k[2]),
                            "l": float(k[3]),
                        }
                    )
                last = float(ks[-1][0]) / 1000
                if last <= cur:
                    break
                cur = last + (300 if self.interval == "5m" else 60)
            except Exception as e:
                print(f"[ERR] {e}")
                break
        return candles

    def _windows(self, start, end, dur=300):
        ws = []
        t = start
        while t + dur <= end + 1:
            ws.append(
                {
                    "s": t,
                    "e": t + dur,
                    "sp": None,
                    "ep": None,
                    "dir": None,
                    "d": 0.0,
                    "h": None,
                    "l": None,
                }
            )
            t += dur
        return ws

    def _pop_windows(self, ws, candles, dur=300):
        for w in ws:
            wc = [c for c in candles if w["s"] <= c["t"] < w["e"]]
            if wc:
                w["sp"] = wc[0]["o"]
                w["ep"] = wc[-1]["c"]
                w["h"] = max(c["h"] for c in wc)
                w["l"] = min(c["l"] for c in wc)
                w["d"] = w["ep"] - w["sp"]
                w["dir"] = "Up" if w["d"] > 0 else "Down"

    def run(self):
        print(f"\n{'=' * 50}")
        print(f"BTC SNIPER V3  dur={self.dur}s bankroll=${self.starting:.2f}")
        print(
            f"Delta>${self.delta_thresh:.0f}  Conf>={self.conf_thresh:.2f}  "
            f"Ens>{self.ens_thresh:.0%}  Int={self.interval}  "
            f"Warmup={self.WARMUP}"
        )
        print(f"{'=' * 50}\n")

        t_now = time.time()
        t_sim = t_now - self.dur
        t_warm = t_sim - (self.WARMUP * (300 if self.interval == "5m" else 60))

        warm = self._fetch(t_warm, t_sim)
        sim = self._fetch(t_sim, t_now)

        if len(sim) < 2:
            print("[FATAL] No sim candles.")
            return

        wd_dur = 300 if self.interval == "5m" else 60
        for c in warm:
            self.se.update(c["c"], c["t"])

        wins2 = self._windows(t_sim, t_now, wd_dur)
        self._pop_windows(wins2, sim, wd_dur)

        print(
            f"[SIM] Warmup={len(warm)}c ({len(warm) * 5 if self.interval == '5m' else len(warm)}min)  "
            f"Sim={len(sim)}c ({len(sim) * 5 if self.interval == '5m' else len(sim)}min)"
        )
        print(f"[SIM] Sim price: \${sim[0]['c']:.2f} → \${sim[-1]['c']:.2f}")
        print(
            f"[SIM] {len(wins2)} windows, {sum(1 for w in wins2 if w['sp'])} with data\n"
        )

        sc = {"Up": 0, "Down": 0, "Neutral": 0}
        traded = {}
        checks = 0

        for i, c in enumerate(sim):
            self.se.update(c["c"], c["t"])

            w = next((x for x in wins2 if x["s"] <= c["t"] < x["e"]), None)
            if w is None:
                continue

            checks += 1
            sig = self.se.all()
            sc[sig["direction"]] += 1
            wid = id(w)

            can_trade = True
            if self.multi_trade:
                last_t = traded.get(wid, 0)
                if c["t"] - last_t < self.trade_cooldown:
                    can_trade = False
            else:
                if wid in traded:
                    can_trade = False

            if (
                can_trade
                and w["sp"]
                and sig["direction"] != "Neutral"
                and sig["confidence"] >= 0.01
            ):
                delta = c["c"] - w["sp"]
                if abs(delta) >= self.delta_thresh:
                    dir_ok = (sig["direction"] == "Up" and delta > 0) or (
                        sig["direction"] == "Down" and delta < 0
                    )
                    if dir_ok:
                        self._trade(sig, delta, c["c"], w, wid, traded, c["t"])

            if i > 0 and i % 60 == 0:
                el = c["t"] - t_sim
                t = self.wins + self.losses
                wr = self.wins / t if t > 0 else 0
                print(
                    f"[SIM] {el / self.dur * 100:4.0f}% | "
                    f"Trades:{t}(W:{self.wins} L:{self.losses}) "
                    f"WR:{wr:.0%} | Bk:${self.bankroll:.2f}"
                )

        self._report(sc, checks)

    def _trade(self, sig, delta, price, window, wid, traded, ts):
        max_bet = self.starting * 0.50
        spend = min(max(MIN_SPEND, self.bankroll * self.spend_ratio), max_bet)
        if spend > self.bankroll:
            self.blocks += 1
            return

        direction = "Up" if delta > 0 else "Down"
        pp = poly_price(abs(delta), price)
        won = direction == window["dir"]

        if won:
            pnl = spend * (1.0 / pp - 1) * (1 - POLYFEE)
        else:
            pnl = -spend

        self.trades.append(
            {
                "time": window["s"],
                "dir": direction,
                "spend": spend,
                "pp": pp,
                "delta": delta,
                "win_dir": window["dir"],
                "won": won,
                "pnl": pnl,
                "conf": sig["confidence"],
            }
        )
        self.bankroll += pnl
        self.total_pnl += pnl
        if won:
            self.wins += 1
        else:
            self.losses += 1
        traded[wid] = ts
        e = "🟢" if won else "🔴"
        print(
            f"  {e} {direction} | ${spend:.2f} | "
            f"Δ${delta:+8.2f} | c={sig['confidence']:.2f} | "
            f"{'WIN' if won else 'LOSS'} ${pnl:+7.2f} | Bk:${self.bankroll:.2f}"
        )

    def _report(self, sc, checks):
        el = time.time() - self.t0
        tt = self.wins + self.losses
        wr = self.wins / tt if tt > 0 else 0
        pnl_p = self.total_pnl / self.starting * 100
        print(f"\n{'=' * 50}")
        print(f"RESULTS  ({el:.1f}s)")
        print(f"{'=' * 50}")
        print(f"Checks:  {checks}  Signals: {sc}")
        print(f"Trades:  {tt}  W:{self.wins} L:{self.losses} Bl:{self.blocks}")
        print(f"WinRate: {wr:.1%}")
        print(f"PnL:     ${self.total_pnl:+.2f}  ({pnl_p:+.2f}%)")
        print(f"Bankroll:${self.bankroll:.2f} / ${self.starting:.2f}")
        print(f"{'=' * 50}\n")
        if self.trades:
            print("Last 5:")
            for t in self.trades[-5:]:
                e = "🟢" if t["won"] else "🔴"
                print(
                    f"  {e} {t['dir']} | ${t['spend']:.2f} | "
                    f"Δ${t['delta']:+8.2f} | c={t['conf']:.2f} | "
                    f"{'WIN' if t['won'] else 'LOSS'} ${t['pnl']:+7.2f}"
                )
        out = os.path.join(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")), "tmp", "autoresearch", "sniper_sim_results.json")
        result = {
            "duration": self.dur,
            "bankroll_start": self.starting,
            "bankroll_end": self.bankroll,
            "pnl": self.total_pnl,
            "pnl_pct": pnl_p,
            "trades": tt,
            "wins": self.wins,
            "losses": self.losses,
            "win_rate": wr,
            "blocked": self.blocks,
            "signals": sc,
            "checks": checks,
            "params": {
                "delta_thresh": self.delta_thresh,
                "conf_thresh": self.conf_thresh,
                "ens_thresh": self.ens_thresh,
                "spend_ratio": self.spend_ratio,
                "interval": self.interval,
            },
            "trades_detail": self.trades,
        }
        with open(out, "w") as f:
            json.dump(result, f, indent=2, default=str)
        print(f"\n→ {out}")


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "sim"
    dur = int(sys.argv[2]) if len(sys.argv) > 2 else 3600
    if mode == "sim":
        SimEngine(
            dur=dur,
            delta_thresh=3.0,
            conf_thresh=0.08,
            ens_thresh=0.08,
            spend_ratio=0.2,
            interval="1m",
            multi_trade=False,
            trade_cooldown=60,
        ).run()
