#!/usr/bin/env python3
"""Auto-Improve Service v2 — Polymarket intraday strategy optimizer.
 Calls local AI (localhost:18080 MiniMax) to analyze trade journal,
 then evolves GBM backtested parameters. Designed to run nightly via cron.
"""
import os, sys, json, time, subprocess, random
from datetime import datetime
from pathlib import Path

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
DATA_DIR = Path(os.path.join(HARVEY_HOME, "data", "arbitrage-agent", "v2"))
JOURNAL_FILE = DATA_DIR / "state" / "intraday_journal.jsonl"
STATE_FILE = DATA_DIR / "state" / "intraday_trades.json"
BEST_PARAMS = DATA_DIR / "state" / "best_intraday_params.json"
LOG_FILE = DATA_DIR / "logs" / "autoimprove.log"
AI_URL = "http://localhost:18080/v1/chat/completions"
AI_KEY = "sk-test-123"
AI_MODEL = "minimax:MiniMax-M2.7"

def log(msg):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = "[%s] %s" % (ts, msg)
    print(line)
    with open(LOG_FILE, "a") as fh:
        fh.write(line + "\n")

def ai_complete(prompt, max_tokens=400):
    payload = {
        "model": AI_MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "thinking": {"type": "disable"},
        "tools": [],
        "tool_choice": "auto",
    }
    try:
        r = subprocess.run(
            ["curl", "-s", "-X", "POST", AI_URL,
             "-H", "Content-Type: application/json",
             "-H", "Authorization: Bearer %s" % AI_KEY,
             "-d", json.dumps(payload)],
            capture_output=True, text=True, timeout=60
        )
        if r.returncode == 0 and r.stdout:
            resp = json.loads(r.stdout)
            choices = resp.get("choices", [{}])
            if choices:
                content = choices[0].get("message", {}).get("content", "")
                return content
    except Exception as e:
        log("AI call failed: %s" % e)
    return ""

def load_trade_stats():
    stats = {
        "total_trades": 0, "wins": 0, "losses": 0, "breakeven": 0,
        "total_pnl": 0.0, "win_rate": 0.5, "avg_pnl": 0.0,
        "best_trade": 0.0, "worst_trade": 0.0,
    }
    if JOURNAL_FILE.exists():
        trades = []
        with open(JOURNAL_FILE) as fh:
            for line in fh:
                line = line.strip()
                if line:
                    try:
                        trades.append(json.loads(line))
                    except Exception:
                        pass
        if trades:
            pnls = [t.get("pnl", 0) for t in trades]
            stats["total_trades"] = len(trades)
            stats["total_pnl"] = sum(pnls)
            stats["avg_pnl"] = stats["total_pnl"] / len(trades)
            stats["best_trade"] = max(pnls) if pnls else 0.0
            stats["worst_trade"] = min(pnls) if pnls else 0.0
            wins = [t for t in trades if t.get("result") == "win"]
            losses = [t for t in trades if t.get("result") == "loss"]
            stats["wins"] = len(wins)
            stats["losses"] = len(losses)
            stats["win_rate"] = len(wins) / max(len(wins) + len(losses), 1)
    return stats

def ai_suggest_params(stats, recent_trades):
    lines = []
    for t in (recent_trades or [])[-15:]:
        how = t.get("how", "?")
        side = t.get("side", "?")
        ep = t.get("entry_price", 0)
        xp = t.get("exit_price", 0)
        pnl = t.get("pnl", 0)
        res = t.get("result", "?")
        dur = int(t.get("duration_secs", 0))
        line = "  %s | %s | entry=%.4f exit=%.4f pnl=$%.4f | %s (%ds)" % (how, side, ep, xp, pnl, res, dur)
        lines.append(line)
    if not lines:
        lines = ["  (no trades yet"]

    prompt_lines = [
        "You are a quant analyzing a Polymarket BTC momentum strategy.",
        "",
        "Recent trades:",
    ] + lines + [
        "",
        "Stats: %d trades | WR=%.0f%% | PnL=$%.4f" % (
            stats["total_trades"], stats["win_rate"] * 100, stats["total_pnl"]),
        "",
        "Tune these params: window(5-120), mom_th(0.05-1.0), vol_th(0.03-0.5),",
        "stop_loss_pct(0.5-5.0), take_profit_pct(0.5-5.0), size_pct(0.02-0.20),",
        "max_hold_secs(60-1800), allow_breakout/down/dip/rip bools.",
        "",
        'Return ONLY JSON: {"window":20,"mom_th":0.1,"vol_th":0.1,',
        '"stop_loss_pct":2.0,"take_profit_pct":3.0,"size_pct":0.1,',
        '"max_hold_secs":600,"allow_breakout":true,"allow_breakdown":true,',
        '"allow_dip_buy":true,"allow_rip_sell":false,"reasoning":"one sentence",
    ]

    prompt = "\n".join(prompt_lines)
    log("Asking AI for suggestions...")
    response = ai_complete(prompt, max_tokens=400)
    log("AI response: %s..." % response[:200])

    try:
        start = response.find("{")
        end = response.rfind("}") + 1
        if start >= 0 and end > start:
            params = json.loads(response[start:end])
            log("AI suggested: %s" % params.get("reasoning", ""))
            return params
    except Exception as e:
        log("AI parse failed: %s" % e)
    return {}

def gbm_backtest(params, seed=42):
    vol = 1.5
    base_px = 66500.0
    rng = random.Random(seed)
    FEE = 0.0005

    path = [base_px]
    for _ in range(720):
        dW = rng.gauss(0, 1) * vol * 0.1
        drift = rng.choice([-0.1, 0.0, 0.0, 0.1, -0.05, 0.05])
        prev = path[-1]
        new_px = prev * (1 + drift / 100 + dW / 100)
        new_px = max(base_px * 0.8, min(base_px * 1.2, new_px))
        path.append(new_px)

    capital = 100.0
    positions = []
    W = int(params.get("window", 20))
    MOM_TH = float(params.get("mom_th", 0.1))
    VOL_TH = float(params.get("vol_th", 0.1))
    SL = float(params.get("stop_loss_pct", 2.0))
    TP = float(params.get("take_profit_pct", 3.0))
    SZ = float(params.get("size_pct", 0.1))
    HOLD = int(params.get("max_hold_secs", 600))
    allow_bo = bool(params.get("allow_breakout", True))
    allow_bd = bool(params.get("allow_breakdown", True))
    allow_db = bool(params.get("allow_dip_buy", True))
    allow_rs = bool(params.get("allow_rip_sell", False))

    for tick in range(W + 1, 720):
        window = path[tick - W - 1:tick]
        if len(window) < 2:
            continue
        cur = path[tick]
        avg = sum(window) / len(window)
        mom = (cur - avg) / max(avg, 0.001) * 100
        rng_val = (max(window) - min(window)) / max(min(window), 0.001) * 100

        has_pos = any(p["tick"] > tick - HOLD for p in positions)

        if not has_pos and len(positions) < 5:
            entry = None
            if mom < -MOM_TH and rng_val > VOL_TH and allow_bd:
                entry = ("SHORT", tick)
            elif mom > MOM_TH and rng_val > VOL_TH and allow_bo:
                entry = ("LONG", tick)
            elif mom < -MOM_TH * 2 and allow_db:
                entry = ("LONG", "dip_buy")
            elif mom > MOM_TH * 2 and allow_rs:
                entry = ("SHORT", "rip_sell")

            if entry:
                side, _ = entry
                sz = max(1, int(capital * SZ / cur))
                val = sz * cur
                if val <= capital * 0.90:
                    positions.append({
                        "tick": tick, "side": side,
                        "entry": cur, "size": sz, "value": val,
                        "stop": SL, "take": TP,
                    })
                    capital -= val

        next_pos = []
        for p in positions:
            age = tick - p["tick"]
            pct = (path[tick] - p["entry"]) / p["entry"] * 100
            if p["side"] == "SHORT":
                pct = -pct
            exit_now = False
            how = ""
            if pct >= p["take"]:
                exit_now = True; how = "tp"
            elif pct <= -p["stop"]:
                exit_now = True; how = "sl"
            elif age >= HOLD:
                exit_now = True; how = "time"
            if exit_now:
                fee = p["value"] * FEE
                net = (pct / 100) * p["value"] - fee
                capital += p["value"] + net
            else:
                next_pos.append(p)
        positions = next_pos

    final_pnl = capital - 100.0
    score = final_pnl * 10
    return {"pnl": round(final_pnl, 4), "score": round(score, 4)}

def evolve(ai_suggestion, n_variants=16):
    base = dict(ai_suggestion) if ai_suggestion else {
        "window": 20, "mom_th": 0.1, "vol_th": 0.1,
        "stop_loss_pct": 2.0, "take_profit_pct": 3.0, "size_pct": 0.1,
        "max_hold_secs": 600, "allow_breakout": True, "allow_breakdown": True,
        "allow_dip_buy": True, "allow_rip_sell": False,
    }

    candidates = [dict(base)]
    for _ in range(n_variants - 1):
        c = dict(base)
        if random.random() < 0.3:
            c["window"] = max(5, min(120, c["window"] + random.choice([-10, -5, 5, 10])))
        if random.random() < 0.3:
            c["mom_th"] = max(0.05, min(1.0, c["mom_th"] + random.choice([-0.05, 0.05, 0.1])))
        if random.random() < 0.3:
            c["vol_th"] = max(0.03, min(0.5, c["vol_th"] + random.choice([-0.05, 0.05])))
        if random.random() < 0.3:
            c["stop_loss_pct"] = max(0.5, min(5.0, c["stop_loss_pct"] + random.choice([-0.5, 0.5])))
        if random.random() < 0.3:
            c["take_profit_pct"] = max(0.5, min(5.0, c["take_profit_pct"] + random.choice([-0.5, 0.5])))
        if random.random() < 0.3:
            c["max_hold_secs"] = max(60, min(1800, c["max_hold_secs"] + random.choice([-120, 120])))
        if random.random() < 0.2:
            c["allow_breakout"] = not c["allow_breakout"]
        if random.random() < 0.2:
            c["allow_breakdown"] = not c["allow_breakdown"]
        if random.random() < 0.2:
            c["allow_dip_buy"] = not c["allow_dip_buy"]
        candidates.append(c)

    best = None
    best_score = -999.0
    for params in candidates:
        r = gbm_backtest(params)
        if r["score"] > best_score:
            best_score = r["score"]
            best = params

    log("Evolve: best score=%.4f" % best_score)
    return best

def run():
    log("=" * 60)
    log("AUTO-IMPROVE STARTING")
    log("=" * 60)

    stats = load_trade_stats()
    log("Stats: %d trades WR=%.0f%% PnL=$%.4f" % (
        stats["total_trades"], stats["win_rate"] * 100, stats["total_pnl"]))

    recent = []
    if JOURNAL_FILE.exists():
        with open(JOURNAL_FILE) as fh:
            for line in fh:
                line = line.strip()
                if line:
                    try:
                        recent.append(json.loads(line))
                    except Exception:
                        pass

    ai_suggestion = {}
    try:
        ai_suggestion = ai_suggest_params(stats, recent)
    except Exception as e:
        log("AI analysis error: %s" % e)

    best = evolve(ai_suggestion, n_variants=16)

    with open(BEST_PARAMS, "w") as fh:
        json.dump({
            "params": best,
            "stats": stats,
            "timestamp": datetime.now().isoformat(),
        }, fh, indent=2)

    log("Saved %s" % BEST_PARAMS)
    log("Best window=%d mom_th=%.3f vol_th=%.3f SL=%.1f TP=%.1f sz=%.0f%% hold=%ds" % (
        best["window"], best["mom_th"], best["vol_th"],
        best["stop_loss_pct"], best["take_profit_pct"],
        best["size_pct"] * 100, best["max_hold_secs"]))
    log("DONE")

if __name__ == "__main__":
    run()
