#!/usr/local/opt/python@3.11/bin/python3.11
"""
BTC Sniper — 2-Hour Autoresearch Loop
======================================
Runs for 2 hours, iterating parameter combinations,
tracking the best result across multiple simulations.
"""

import json, time, os, sys, subprocess, random
from datetime import datetime, timezone

HARVEY_HOME = os.path.expanduser(os.environ.get("HARVEY_HOME", "~/MAKAKOO"))
START_BUDGET = 100.0
SIM_DURATION = 7200  # 2 hours in seconds
RESULTS_FILE = os.path.join(HARVEY_HOME, "tmp", "autoresearch", "sniper_sim_results.json")
STRATEGY_FILE = os.path.join(HARVEY_HOME, "tmp", "autoresearch", "sniper_strategy.py")
RESULTS_TSV = os.path.join(HARVEY_HOME, "tmp", "autoresearch", "sniper_results.tsv")

PARAM_GRID = [
    # delta_thresh, conf_thresh, ens_thresh, spend_ratio, multi_trade, trade_cooldown, signal_mode
    {
        "delta_thresh": 3.0,
        "conf_thresh": 0.08,
        "ens_thresh": 0.08,
        "spend_ratio": 0.20,
        "multi_trade": False,
        "trade_cooldown": 60,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 2.0,
        "conf_thresh": 0.06,
        "ens_thresh": 0.06,
        "spend_ratio": 0.20,
        "multi_trade": False,
        "trade_cooldown": 60,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 1.0,
        "conf_thresh": 0.05,
        "ens_thresh": 0.05,
        "spend_ratio": 0.20,
        "multi_trade": False,
        "trade_cooldown": 60,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.5,
        "conf_thresh": 0.04,
        "ens_thresh": 0.04,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 30,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.03,
        "ens_thresh": 0.03,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 30,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.02,
        "ens_thresh": 0.02,
        "spend_ratio": 0.25,
        "multi_trade": True,
        "trade_cooldown": 20,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.01,
        "ens_thresh": 0.01,
        "spend_ratio": 0.25,
        "multi_trade": True,
        "trade_cooldown": 15,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 5.0,
        "conf_thresh": 0.10,
        "ens_thresh": 0.10,
        "spend_ratio": 0.20,
        "multi_trade": False,
        "trade_cooldown": 60,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 3.0,
        "conf_thresh": 0.05,
        "ens_thresh": 0.05,
        "spend_ratio": 0.20,
        "multi_trade": False,
        "trade_cooldown": 60,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 2.0,
        "conf_thresh": 0.04,
        "ens_thresh": 0.04,
        "spend_ratio": 0.20,
        "multi_trade": False,
        "trade_cooldown": 60,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 1.5,
        "conf_thresh": 0.03,
        "ens_thresh": 0.03,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 45,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 1.0,
        "conf_thresh": 0.03,
        "ens_thresh": 0.03,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 30,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.02,
        "ens_thresh": 0.02,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 20,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.01,
        "ens_thresh": 0.01,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 15,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.01,
        "ens_thresh": 0.01,
        "spend_ratio": 0.30,
        "multi_trade": True,
        "trade_cooldown": 10,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.005,
        "ens_thresh": 0.005,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 10,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.005,
        "ens_thresh": 0.005,
        "spend_ratio": 0.25,
        "multi_trade": True,
        "trade_cooldown": 10,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.005,
        "ens_thresh": 0.005,
        "spend_ratio": 0.30,
        "multi_trade": True,
        "trade_cooldown": 5,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.002,
        "ens_thresh": 0.002,
        "spend_ratio": 0.20,
        "multi_trade": True,
        "trade_cooldown": 5,
        "signal_mode": "ensemble",
    },
    {
        "delta_thresh": 0.0,
        "conf_thresh": 0.002,
        "ens_thresh": 0.002,
        "spend_ratio": 0.30,
        "multi_trade": True,
        "trade_cooldown": 5,
        "signal_mode": "ensemble",
    },
]


def git_commit(msg):
    autoresearch_dir = os.path.join(HARVEY_HOME, "tmp", "autoresearch")
    subprocess.run(
        f"cd {autoresearch_dir} && git add sniper_strategy.py && git commit -m "
        + json.dumps(msg),
        shell=True,
        capture_output=True,
    )


def read_results():
    try:
        with open(RESULTS_FILE) as f:
            return json.load(f)
    except:
        return None


def run_sim(params):
    """Apply params to strategy and run 2hr simulation, return results."""
    apply_params(params)
    result = subprocess.run(
        ["/usr/local/opt/python@3.11/bin/python3.11", STRATEGY_FILE, "sim", "7200"],
        capture_output=True,
        text=True,
        timeout=300,
    )
    return read_results()


def apply_params(params):
    """Modify sniper_strategy.py with given parameters."""
    with open(STRATEGY_FILE) as f:
        code = f.read()

    # Modify SimEngine call
    old_call = """    if mode == "sim":
        SimEngine(
            dur=dur,
            delta_thresh=3.0,
            conf_thresh=0.08,
            ens_thresh=0.08,
            spend_ratio=0.20,
            interval="1m",
            multi_trade=False,
            trade_cooldown=60,
        ).run()"""

    new_call = f"""    if mode == "sim":
        SimEngine(
            dur=dur,
            delta_thresh={params["delta_thresh"]},
            conf_thresh={params["conf_thresh"]},
            ens_thresh={params["ens_thresh"]},
            spend_ratio={params["spend_ratio"]},
            interval="1m",
            multi_trade={params["multi_trade"]},
            trade_cooldown={params["trade_cooldown"]},
        ).run()"""

    code = code.replace(old_call, new_call)

    # Modify signal ensemble threshold
    ens_val = params["ens_thresh"]
    old_ens = "if up - down > total * 0.04:"
    new_ens = f"if up - down > total * {ens_val:.3f}:"
    code = code.replace(old_ens, new_ens)

    with open(STRATEGY_FILE, "w") as f:
        f.write(code)


def main():
    print(f"\n{'=' * 60}")
    print(f"BTC SNIPER — 2-HOUR AUTORESEARCH LOOP")
    print(f"{'=' * 60}")
    print(f"Start budget: ${START_BUDGET:.2f}")
    print(f"Sim duration: {SIM_DURATION / 3600:.0f} hours")
    print(f"Experiments to try: {len(PARAM_GRID)}")
    print(f"{'=' * 60}\n")

    deadline = time.time() + SIM_DURATION
    best_result = None
    best_params = None
    best_pnl_pct = -999
    experiment_count = 0
    round_num = 1

    # First: run current baseline
    print(
        f"\n[{datetime.now().strftime('%H:%M:%S')}] Round {round_num}: Baseline simulation..."
    )
    round_num += 1
    result = run_sim(PARAM_GRID[0])
    if result:
        experiment_count += 1
        pnl_pct = result.get("pnl_pct", 0)
        wr = result.get("win_rate", 0)
        trades = result.get("trades", 0)
        end_bk = result.get("bankroll_end", START_BUDGET)
        print(
            f"  → Trades:{trades} WR:{wr:.0%} PnL:{pnl_pct:+.1f}% Bankroll:${end_bk:.2f}"
        )
        if pnl_pct > best_pnl_pct:
            best_pnl_pct = pnl_pct
            best_result = result
            best_params = PARAM_GRID[0].copy()
            git_commit(
                f"autoresearch round {experiment_count}: best so far PnL={pnl_pct:.1f}%"
            )

    # Shuffle grid for variety, then loop until deadline
    grid = PARAM_GRID[1:]
    random.shuffle(grid)

    while time.time() < deadline:
        remaining = deadline - time.time()
        mins_left = int(remaining / 60)
        print(
            f"\n[{datetime.now().strftime('%H:%M:%S')}] Round {round_num} | "
            f"{mins_left}m left | Best: {best_pnl_pct:+.1f}% PnL "
            f"(${best_result.get('bankroll_end', START_BUDGET):.2f}) "
            f"| Exps run: {experiment_count}"
        )

        round_num += 1
        for params in grid:
            if time.time() >= deadline:
                break

            remaining = deadline - time.time()
            if remaining < 120:
                print(f"  < 2min left, stopping iteration")
                break

            experiment_count += 1
            p = params
            print(
                f"  → delta={p['delta_thresh']:.1f} conf={p['conf_thresh']:.3f} "
                f"ens={p['ens_thresh']:.3f} spend={p['spend_ratio']:.0%} "
                f"multi={p['multi_trade']} cd={p['trade_cooldown']}s",
                end=" ... ",
            )

            result = run_sim(params)
            if result:
                pnl_pct = result.get("pnl_pct", 0)
                wr = result.get("win_rate", 0)
                trades = result.get("trades", 0)
                end_bk = result.get("bankroll_end", START_BUDGET)
                pnl_std = pnl_pct / max(1, trades) if trades > 0 else 0

                print(
                    f"Trades:{trades} WR:{wr:.0%} PnL:{pnl_pct:+.1f}% "
                    f"PnL/trade:${pnl_std:.2f} Bk:${end_bk:.2f}"
                )

                is_best = False
                if pnl_pct > best_pnl_pct:
                    best_pnl_pct = pnl_pct
                    best_result = result
                    best_params = params.copy()
                    is_best = True
                    git_commit(
                        f"autoresearch exp {experiment_count}: PnL={pnl_pct:.1f}% "
                        f"WR={wr:.0%} trades={trades} delta={p['delta_thresh']} "
                        f"conf={p['conf_thresh']} ens={p['ens_thresh']} "
                        f"spend={p['spend_ratio']:.0%} multi={p['multi_trade']}"
                    )
                    print(f"     ★ NEW BEST ★")
            else:
                print(f"CRASHED")

        # Reshuffle for next pass
        random.shuffle(grid)

    # Final report
    print(f"\n{'=' * 60}")
    print(f"FINAL RESULTS — 2-HOUR AUTORESEARCH")
    print(f"{'=' * 60}")
    print(f"Starting budget: ${START_BUDGET:.2f}")
    print(f"Final bankroll:  ${best_result.get('bankroll_end', START_BUDGET):.2f}")
    print(
        f"Total PnL:        ${best_result.get('total_pnl', 0):+.2f} "
        f"({best_pnl_pct:+.2f}%)"
    )
    print(f"Trades:          {best_result.get('trades', 0)}")
    print(f"Win rate:        {best_result.get('win_rate', 0):.1%}")
    print(f"Best params:     {best_params}")
    print(f"Experiments run: {experiment_count}")
    print(f"{'=' * 60}\n")

    # Save final best result
    out = {
        "starting_budget": START_BUDGET,
        "final_budget": best_result.get("bankroll_end", START_BUDGET),
        "total_pnl": best_result.get("total_pnl", 0),
        "pnl_pct": best_pnl_pct,
        "trades": best_result.get("trades", 0),
        "win_rate": best_result.get("win_rate", 0),
        "best_params": best_params,
        "experiments_run": experiment_count,
        "best_result": best_result,
    }
    with open(os.path.join(HARVEY_HOME, "tmp", "autoresearch", "final_best.json"), "w") as f:
        json.dump(out, f, indent=2, default=str)

    return out


if __name__ == "__main__":
    random.seed(42)
    main()
