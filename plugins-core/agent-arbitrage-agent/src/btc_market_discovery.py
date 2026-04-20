#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Market Discovery Daemon — TB-2 of SPRINT_001_AUDIT

Continuously scans Polymarket for BTC-related markets, maps them to live BTC
technical signals, and fires alerts when our edge exceeds threshold.

Usage:
    python3 btc_market_discovery.py [--poll-interval 300] [--edge-threshold 0.05]

Alert channels:
    - Log to btc_discovery.log
    - Send via gws gmail (if credentials configured)
    - Log to data/arbitrage-agent/v2/state/discovery_alerts.jsonl
"""

import argparse
import json
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(__file__).rsplit("/", 1)[0])

DATA_DIR = (
    Path(os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO")))
    / "data"
    / "arbitrage-agent"
    / "v2"
)
STATE_DIR = DATA_DIR / "state"
LOG_FILE = STATE_DIR / "logs" / "btc_discovery.log"
ALERT_FILE = DATA_DIR / "state" / "discovery_alerts.jsonl"
LAST_ALERT_FILE = DATA_DIR / "state" / "last_discovery.json"

STATE_DIR.mkdir(parents=True, exist_ok=True)
(DATA_DIR / "state" / "logs").mkdir(parents=True, exist_ok=True)


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_FILE, "a") as f:
        f.write(line + "\n")


def get_alert_key(opp) -> str:
    return f"{opp.market_id[:16]}_{opp.position_side}"


def load_last_alerts() -> dict:
    if LAST_ALERT_FILE.exists():
        try:
            with open(LAST_ALERT_FILE) as f:
                return json.load(f)
        except Exception:
            pass
    return {"alerted": {}}


def save_last_alerts(data: dict):
    try:
        with open(LAST_ALERT_FILE, "w") as f:
            json.dump(data, f, indent=2)
    except Exception:
        pass


def log_alert(opp):
    try:
        with open(ALERT_FILE, "a") as f:
            row = {
                "ts": datetime.now(timezone.utc).isoformat(),
                "market_id": opp.market_id,
                "question": opp.question,
                "position_side": opp.position_side,
                "yes_price": opp.yes_price,
                "no_price": opp.no_price,
                "edge": round(opp.edge, 4),
                "ev": round(opp.expected_value, 4),
                "size_pct": opp.position_size_pct,
                "liquidity": opp.liquidity,
                "hours_remaining": round(opp.hours_remaining, 1),
                "reasoning": opp.reasoning,
                "btc_direction": opp.btc_direction,
            }
            f.write(json.dumps(row) + "\n")
    except Exception as e:
        log(f"  [WARN] Could not log alert: {e}")


def send_gws_alert(opportunities: list, btc_sig=None):
    if not opportunities:
        return

    body_lines = [
        f"Harvey BTC Discovery Alert — {datetime.now().strftime('%Y-%m-%d %H:%M UTC')}",
        "",
    ]

    if btc_sig:
        body_lines.append(
            f"BTC Signal: {btc_sig.direction.upper()} | Confidence: {btc_sig.confidence:.0%}"
        )
        body_lines.append(
            f"BTC Price: ${btc_sig.btc_price:,.0f} | P(up)={btc_sig.probability_up:.0%} | Score={btc_sig.score:.0f}"
        )
        body_lines.append("")

    for opp in opportunities[:5]:
        body_lines.append(f"[{opp.position_side}] {opp.question[:80]}")
        body_lines.append(
            f"  YES={opp.yes_price:.3f} NO={opp.no_price:.3f} | Edge={opp.edge:.1%} | EV={opp.expected_value:.4f}"
        )
        body_lines.append(
            f"  Size: {opp.position_size_pct:.0%} of bankroll | {opp.hours_remaining:.0f}h until resolve"
        )
        body_lines.append(f"  → {opp.reasoning}")
        body_lines.append("")

    body_lines.append("— Harvey BTC Discovery Daemon")

    body = "\n".join(body_lines)

    subject = (
        f"[HARVEY] BTC Polymarket Alert — {len(opportunities)} opportunity(ies) found"
    )

    try:
        import subprocess
        import base64

        from email.mime.multipart import MIMEBase
        from email.mime.text import MIMEText
        from email.header import Header
        from email.utils import formataddr
        import json as _json

        msg = MIMEBase("mixed")
        msg["Subject"] = Header(subject, "utf-8")
        msg["From"] = formataddr(("Harvey Bot", "me"))
        msg["To"] = formataddr(("Sebastian", "sebastian@schkudlara.com"))

        body_part = MIMEText(body, "plain", "utf-8")
        msg.attach(body_part)

        raw_bytes = msg.as_bytes()
        raw_b64 = base64.urlsafe_b64encode(raw_bytes).decode("ascii")

        payload = _json.dumps({"raw": raw_b64})

        result = subprocess.run(
            [
                "gws",
                "gmail",
                "users",
                "me",
                "messages",
                "send",
                "--json",
                payload,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            log(f"  Email alert sent successfully")
        else:
            log(f"  [WARN] Email alert failed: {result.stderr[:200]}")
    except FileNotFoundError:
        log("  [INFO] gws not available — skipping email alert")
    except Exception as e:
        log(f"  [WARN] Email alert error: {e}")


def run_discovery(
    poll_interval: int = 300, edge_threshold: float = 0.05, send_alerts: bool = True
):
    from btc_signals import BTCSignalGenerator
    from btc_price_feed import BTCPriceFeed
    from polymarket_signal_mapper import PolymarketSignalMapper

    log(f"=== BTC Market Discovery Daemon Started ===")
    log(
        f"  Poll interval: {poll_interval}s | Edge threshold: {edge_threshold:.0%} | Alerts: {send_alerts}"
    )

    engine = BTCPriceFeed().start()
    sig_gen = BTCSignalGenerator(min_prices=30)
    mapper = PolymarketSignalMapper(min_edge=edge_threshold, min_liquidity=1000)

    wait_time = 15
    log(f"Waiting {wait_time}s for candle warmup...")
    time.sleep(wait_time)

    last_sig = None

    while True:
        try:
            log("--- Discovery cycle ---")

            sig = sig_gen.generate(engine)
            if sig is None:
                log("  No BTC signal yet — need more candle data, sleeping...")
                time.sleep(60)
                continue

            last_sig = sig
            log(
                f"  BTC Signal: {sig.direction} | Confidence: {sig.confidence:.0%} | P(up)={sig.probability_up:.0%} | ${sig.btc_price:,.0f}"
            )

            opportunities = mapper.map_signal(sig, sig.btc_price)
            log(
                f"  Found {len(opportunities)} opportunity(ies) with edge > {edge_threshold:.0%}"
            )

            if opportunities:
                for opp in opportunities[:5]:
                    log(f"  [{opp.position_side}] {opp.question[:70]}")
                    log(
                        f"    YES={opp.yes_price:.3f} NO={opp.no_price:.3f} | Edge={opp.edge:.1%} | EV={opp.expected_value:.4f} | Size={opp.position_size_pct:.0%}"
                    )
                    log(
                        f"    {opp.hours_remaining:.0f}h left | ${opp.liquidity:,.0f} liq"
                    )

                last_alerts = load_last_alerts()
                new_opps = []
                for opp in opportunities:
                    key = get_alert_key(opp)
                    if key not in last_alerts.get("alerted", {}):
                        new_opps.append(opp)
                        last_alerts.setdefault("alerted", {})[key] = (
                            datetime.now().isoformat()
                        )

                if new_opps and send_alerts:
                    send_gws_alert(new_opps, sig)
                    for opp in new_opps:
                        log_alert(opp)
                    save_last_alerts(last_alerts)
                else:
                    log(
                        f"  No new opportunities since last alert — skipping notification"
                    )
            else:
                log(
                    "  No actionable opportunities — BTC signal doesn't overlap with Polymarket markets"
                )

        except Exception as e:
            log(f"  [ERROR] Discovery cycle failed: {e}")

        log(f"Sleeping {poll_interval}s until next cycle...")
        time.sleep(poll_interval)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="BTC Polymarket Market Discovery Daemon"
    )
    parser.add_argument(
        "--poll-interval",
        type=int,
        default=300,
        help="Seconds between scans (default: 300 = 5min)",
    )
    parser.add_argument(
        "--edge-threshold",
        type=float,
        default=0.05,
        help="Minimum edge to alert (default: 0.05 = 5%)",
    )
    parser.add_argument(
        "--no-alerts", action="store_true", help="Run without sending email alerts"
    )
    args = parser.parse_args()

    run_discovery(
        poll_interval=args.poll_interval,
        edge_threshold=args.edge_threshold,
        send_alerts=not args.no_alerts,
    )
