#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Sniper — Live Sim Mode
==========================
Real Polymarket markets + real Binance data + virtual money.
Simulates trades against actual market resolution.
Usage: python3 sniper_live.py [duration_secs] [bankroll]
"""

import json, time, math, os, sys, requests, threading
from datetime import datetime, timezone
from collections import defaultdict

BINANCE_REST = "https://api.binance.com/api/v3/ticker/price?symbol=BTCUSDT"
BINANCE_24HR = "https://api.binance.com/api/v3/ticker/24hr?symbol=BTCUSDT"
BINANCE_KLINES = "https://api.binance.com/api/v3/klines"
POLYMARKET_CLOB = "https://clob.polymarket.com"
MIN_SPEND = 1.05
POLYFEE = 0.01
POLLMARKET_FEE = 0.01

VIRTUAL_BANKROLL = 100.0
SIM_DURATION = 3600


class LiveSimEngine:
    def __init__(
        self,
        starting_bankroll=100.0,
        delta_thresh=3.0,
        conf_thresh=0.08,
        ens_thresh=0.08,
        spend_ratio=0.20,
    ):
        self.starting = starting_bankroll
        self.bankroll = starting_bankroll
        self.delta_thresh = delta_thresh
        self.conf_thresh = conf_thresh
        self.ens_thresh = ens_thresh
        self.spend_ratio = spend_ratio
        self.max_bet = starting_bankroll * 0.50
        self.se = SignalEngine()
        self.trades = []
        self.wins = self.losses = self.blocks = 0
        self.total_pnl = 0.0
        self.running = True
        self.btc_price = None
        self.market_id = None
        self.market_question = None
        self.market_end_time = None
        self.window_start = None
        self.window_price = None
        self.pause_until = 0
        self.t0 = time.time()

    # ---- Polymarket API ----
    def fetch_markets(self):
        try:
            r = requests.get(
                f"{POLYMARKET_CLOB}/markets",
                params={"active": "true", "closed": "false", "limit": 500},
                headers={"Content-Type": "application/json"},
                timeout=10,
            )
            if r.status_code != 200:
                return None
            raw = r.json()
            markets = raw if isinstance(raw, list) else raw.get("data", [])
            btc = [
                m
                for m in markets
                if isinstance(m, dict)
                and "bitcoin" in m.get("question", "").lower()
                and (
                    "up" in m.get("question", "").lower()
                    or "5 min" in m.get("question", "").lower()
                    or "5min" in m.get("question", "").lower()
                    or "down" in m.get("question", "").lower()
                )
                and m.get("active", False)
                and not m.get("closed", True)
                and m.get("accepting_orders", False)
            ]
            return btc[0] if btc else None
        except Exception as e:
            print(f"[PM] Market fetch error: {e}")
            return None
            markets = r.json()
            btc = [
                m
                for m in markets
                if "bitcoin" in m.get("question", "").lower()
                and (
                    "up" in m.get("question", "").lower()
                    or "5 min" in m.get("question", "").lower()
                    or "5min" in m.get("question", "").lower()
                )
            ]
            return btc[0] if btc else None
        except Exception as e:
            print(f"[PM] Market fetch error: {e}")
            return None

    def get_orderbook(self, market_id):
        try:
            r = requests.get(f"{POLYMARKET_CLOB}/orderbook/{market_id}", timeout=5)
            r.raise_for_status()
            return r.json()
        except:
            return {"bids": [], "asks": []}

    def get_polio_price(self, market_id, side="Yes"):
        ob = self.get_orderbook(market_id)
        if side == "Yes":
            asks = ob.get("asks", [])
            if asks:
                return float(asks[0][0])
        else:
            bids = ob.get("bids", [])
            if bids:
                return float(bids[0][0])
        return 0.50

    def resolve_market(self, market_id):
        try:
            r = requests.get(f"{POLYMARKET_CLOB}/markets/{market_id}", timeout=5)
            r.raise_for_status()
            m = r.json()
            resolved = m.get("resolved", False)
            if resolved:
                return m.get("outcome", "")
            return None
        except:
            return None

    # ---- Binance ----
    def get_binance_price(self):
        try:
            r = requests.get(BINANCE_REST, timeout=5)
            r.raise_for_status()
            return float(r.json()["price"])
        except:
            return None

    def get_binance_ticker(self):
        try:
            r = requests.get(BINANCE_24HR, timeout=5)
            r.raise_for_status()
            d = r.json()
            return {
                "price": float(d["lastPrice"]),
                "bid": float(d["bidPrice"]),
                "ask": float(d["askPrice"]),
                "volume": float(d["volume"]),
            }
        except:
            return {}

    # ---- Main loop ----
    def run(self, duration=3600):
        print(f"\n{'=' * 60}")
        print(f"BTC SNIPER — LIVE SIM MODE")
        print(f"Duration:  {duration}s ({duration / 3600:.1f}h)")
        print(f"Bankroll:  ${self.starting:.2f} (VIRTUAL)")
        print(f"Delta>:    ${self.delta_thresh:.0f}")
        print(f"Conf>=:    {self.conf_thresh:.2f}")
        print(f"Ens>:      {self.ens_thresh:.0%}")
        print(f"{'=' * 60}\n")

        deadline = time.time() + duration

        # Fetch initial market
        market = self.fetch_markets()
        if market:
            self.market_id = market["id"]
            self.market_question = market.get("question", "?")
            self.market_end_time = float(market.get("expiry_seconds", 0)) or (
                time.time() + 300
            )
            print(f"[PM] Active market: {self.market_question}")
            print(f"[PM] Market ID: {self.market_id}")
            print(f"[PM] Ends at: {datetime.fromtimestamp(self.market_end_time)}")
        else:
            print("[PM] No active BTC 5-min market found — will retry each cycle")

        last_check = 0
        last_market = 0
        last_status = 0
        checks = 0
        sig_counts = defaultdict(int)
        traded_in_window = False
        current_window_start = None

        while time.time() < deadline and self.running:
            now = time.time()
            elapsed = now - self.t0

            # Refresh market every 30s if none
            if not self.market_id and now - last_market > 30:
                market = self.fetch_markets()
                if market:
                    self.market_id = market["id"]
                    self.market_question = market.get("question", "?")
                    self.market_end_time = float(market.get("expiry_seconds", 0)) or (
                        now + 300
                    )
                    traded_in_window = False
                    self.window_start = None
                    self.window_price = None
                    print(f"[PM] New market: {self.market_question}")
                last_market = now

            # Refresh market if expired
            if (
                self.market_id
                and self.market_end_time
                and now > self.market_end_time - 5
            ):
                resolved = self.resolve_market(self.market_id)
                if resolved:
                    print(f"[PM] Market resolved: '{resolved}'")
                    self.market_id = None
                    self.market_end_time = None
                    traded_in_window = False
                    self.window_start = None
                    self.window_price = None
                    continue

            # Get BTC price
            btc_price = self.get_binance_price()
            if not btc_price:
                time.sleep(1)
                continue

            self.btc_price = btc_price

            # New 5-min window detection
            window_ts = int(now / 300) * 300
            if self.window_start != window_ts:
                self.window_start = window_ts
                self.window_price = btc_price
                traded_in_window = False
                print(
                    f"\n[WIN] New window @{datetime.fromtimestamp(window_ts)} "
                    f"price=${btc_price:.2f}"
                )

            # Update signal engine
            self.se.update(btc_price, now)

            # Check signal every second
            if now - last_check >= 1:
                last_check = now
                checks += 1
                sig = self.se.all(self.ens_thresh)
                sig_counts[sig["direction"]] += 1

                if sig["direction"] == "Neutral":
                    continue

                # Throttle: one trade per window
                if traded_in_window:
                    continue

                # Cooldown between windows
                if now < self.pause_until:
                    continue

                delta = btc_price - self.window_price if self.window_price else 0

                # Delta threshold check
                if abs(delta) < self.delta_thresh:
                    continue

                # Confidence check
                if sig["confidence"] < self.conf_thresh:
                    continue

                # Direction match check
                direction_match = (sig["direction"] == "Up" and delta > 0) or (
                    sig["direction"] == "Down" and delta < 0
                )
                if not direction_match:
                    continue

                # Get Polymarket price
                outcome = "Yes" if sig["direction"] == "Up" else "No"
                poly_price = 0.50
                if self.market_id:
                    poly_price = self.get_polio_price(self.market_id, outcome)
                    if poly_price <= 0:
                        poly_price = 0.50

                spend = min(
                    max(MIN_SPEND, self.bankroll * self.spend_ratio), self.max_bet
                )
                if spend > self.bankroll:
                    self.blocks += 1
                    continue

                direction = sig["direction"]

                # SIMULATE trade resolution at window end
                # We'll mark it as pending and resolve at window close
                trade = {
                    "window_start": self.window_start,
                    "direction": direction,
                    "spend": spend,
                    "poly_price": poly_price,
                    "btc_delta": delta,
                    "btc_price_now": btc_price,
                    "conf": sig["confidence"],
                    "window_resolved": False,
                    "won": False,
                    "pnl": 0,
                    "time": now,
                }
                self.trades.append(trade)
                traded_in_window = True

                # Execute virtual bet
                emoji = "🟢"
                print(
                    f"  {emoji} BET: {direction} | "
                    f"ΔBTC=${delta:+8.2f} | poly={poly_price:.4f} | "
                    f"spend=${spend:.2f} | conf={sig['confidence']:.2f} | "
                    f"Bk=${self.bankroll:.2f}"
                )

            # Resolve completed windows
            for t in self.trades:
                if not t["window_resolved"]:
                    w_end = t["window_start"] + 300
                    if now >= w_end + 5:  # give 5s buffer
                        # Determine actual direction from price at window end
                        w_start_ts = int(t["window_start"])
                        try:
                            r = requests.get(
                                BINANCE_KLINES,
                                timeout=10,
                                params={
                                    "symbol": "BTCUSDT",
                                    "interval": "1m",
                                    "startTime": int(w_start_ts * 1000),
                                    "endTime": int((w_start_ts + 300) * 1000),
                                    "limit": 10,
                                },
                            )
                            if r.status_code == 200 and r.json():
                                klines = r.json()
                                window_close = (
                                    float(klines[0][4]) if klines else btc_price
                                )
                            else:
                                window_close = btc_price
                        except:
                            window_close = btc_price

                        window_delta = window_close - (
                            t["btc_price_now"] - t["btc_delta"]
                        )
                        actual_dir = "Up" if window_delta > 0 else "Down"
                        won = t["direction"] == actual_dir

                        if won:
                            pnl = (
                                t["spend"] * (1.0 / t["poly_price"] - 1) * (1 - POLYFEE)
                            )
                        else:
                            pnl = -t["spend"]

                        t["window_resolved"] = True
                        t["won"] = won
                        t["pnl"] = pnl
                        t["actual_dir"] = actual_dir
                        t["resolved_at"] = now

                        self.bankroll += pnl
                        self.total_pnl += pnl
                        if won:
                            self.wins += 1
                        else:
                            self.losses += 1

                        self.pause_until = now + 5

                        result_emoji = "🟢" if won else "🔴"
                        print(
                            f"       {result_emoji} RESOLVED: {actual_dir} | "
                            f"{'WIN' if won else 'LOSS'} ${pnl:+7.2f} | "
                            f"Bk=${self.bankroll:.2f}"
                        )

            # Status every 30s
            if now - last_status >= 30:
                tt = self.wins + self.losses
                wr = self.wins / tt if tt > 0 else 0
                elapsed_h = elapsed / 3600
                print(
                    f"[{datetime.fromtimestamp(now).strftime('%H:%M:%S')}] "
                    f"elapsed={elapsed_h:.1f}h trades={tt}(W:{self.wins} L:{self.losses}) "
                    f"WR={wr:.0%} Bk=${self.bankroll:.2f} "
                    f"BTC=${btc_price:.0f} signals={dict(sig_counts)}"
                )
                last_status = now

            time.sleep(0.5)

        self._report(checks, sig_counts)

    def _report(self, checks, sig_counts):
        elapsed = time.time() - self.t0
        tt = self.wins + self.losses
        wr = self.wins / tt if tt > 0 else 0
        pnl_p = (self.bankroll - self.starting) / self.starting * 100

        print(f"\n{'=' * 60}")
        print(f"LIVE SIM RESULTS  ({elapsed / 3600:.2f}h elapsed)")
        print(f"{'=' * 60}")
        print(f"Checks:    {checks}")
        print(f"Signals:  {dict(sig_counts)}")
        print(f"Trades:   {tt}  W:{self.wins} L:{self.losses} Blocks:{self.blocks}")
        print(f"Win rate: {wr:.1%}")
        print(f"Start Bk: ${self.starting:.2f}")
        print(f"Final Bk: ${self.bankroll:.2f}")
        print(f"Total PnL: ${self.total_pnl:+.2f}  ({pnl_p:+.2f}%)")
        print(f"{'=' * 60}\n")

        if self.trades:
            print("Trade log:")
            for t in self.trades:
                e = (
                    "🟢"
                    if t.get("won")
                    else ("⏳" if not t.get("window_resolved") else "🔴")
                )
                resolved = "RESOLVED" if t.get("window_resolved") else "PENDING"
                print(
                    f"  {e} [{resolved}] {t['direction']} | "
                    f"spend=${t['spend']:.2f} | poly={t['poly_price']:.4f} | "
                    f"ΔBTC=${t['btc_delta']:+8.2f} | "
                    f"{'WIN' if t.get('won') else ('LOSS' if t.get('window_resolved') else '???')} "
                    f"${t.get('pnl', 0):+7.2f}"
                )

        result = {
            "mode": "live_sim",
            "duration": elapsed,
            "bankroll_start": self.starting,
            "bankroll_end": self.bankroll,
            "total_pnl": self.total_pnl,
            "pnl_pct": pnl_p,
            "trades": tt,
            "wins": self.wins,
            "losses": self.losses,
            "win_rate": wr,
            "blocked": self.blocks,
            "signals": dict(sig_counts),
            "checks": checks,
            "params": {
                "delta_thresh": self.delta_thresh,
                "conf_thresh": self.conf_thresh,
                "ens_thresh": self.ens_thresh,
                "spend_ratio": self.spend_ratio,
            },
            "trades_detail": self.trades,
        }
        out = os.path.join(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")), "tmp", "autoresearch", "sniper_sim_results.json")
        with open(out, "w") as f:
            json.dump(result, f, indent=2, default=str)
        print(f"\n→ {out}")


# ---- Signal Engine (same as strategy) ----
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

    def all(self, ens_thresh=0.08):
        rsi_up, rsi_dn = self.rsi()
        mom_up, mom_dn = self.mom()
        ema_up, ema_dn = self.ema_cross()
        up = 1.5 * rsi_up + 1.6 * mom_up + 1.1 * ema_up
        down = 1.5 * rsi_dn + 1.6 * mom_dn + 1.1 * ema_dn
        total = 8.4
        if up - down > total * ens_thresh:
            dir = "Up"
        elif down - up > total * ens_thresh:
            dir = "Down"
        else:
            dir = "Neutral"
        conf = abs(up - down) / total
        return {"direction": dir, "confidence": conf}


if __name__ == "__main__":
    duration = int(sys.argv[1]) if len(sys.argv) > 1 else SIM_DURATION
    bankroll = float(sys.argv[2]) if len(sys.argv) > 2 else VIRTUAL_BANKROLL
    eng = LiveSimEngine(
        starting_bankroll=bankroll,
        delta_thresh=3.0,
        conf_thresh=0.08,
        ens_thresh=0.08,
        spend_ratio=0.20,
    )
    print(f"Starting live sim: duration={duration}s bankroll=${bankroll:.2f}")
    try:
        eng.run(duration)
    except KeyboardInterrupt:
        eng.running = False
        eng._report(0, {})
