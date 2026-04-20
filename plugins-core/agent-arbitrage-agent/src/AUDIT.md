# Arbitrage Agent v2 — Complete System Audit

**Date:** 2026-03-31
**Auditor:** Harvey

---

## System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    ARBITRAGE AGENT v2                            │
│                                                                 │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────────────┐  │
│  │  DATA LAYER │  │  STRATEGIES  │  │   AUTO-IMPROVEMENT     │  │
│  │             │  │              │  │                        │  │
│  │ RTDSStreamer│  │ Momentum     │  │ AutoImprove (GBM sim)  │  │
│  │ Scanner     │  │ TA (RSI/BB)  │  │ autoimprove.py (AI+GA) │  │
│  │ config.py   │  │ Market-Making│  │ continuous_improve.py   │  │
│  └──────┬──────┘  └──────┬───────┘  └──────────┬─────────────┘  │
│         │               │                     │                 │
│         ▼               ▼                     ▼                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    TRADERS (pick one)                     │   │
│  │                                                          │   │
│  │  intraday_trader.py  ← ACTIVE (BTC momentum + TA/RSI)   │   │
│  │  paper_trader.py     ← DORMANT (Polymarket binary mkts)  │   │
│  │  executor.py         ← DORMANT (real CLOB orders)        │   │
│  └──────────────────────────────────────────────────────────┘   │
│                          │                                      │
│                          ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    STATE (persistence)                    │   │
│  │  state/intraday_trades.json    ← portfolio + positions   │   │
│  │  state/intraday_journal.jsonl  ← all closed trades       │   │
│  │  state/best_intraday_params.json ← evolved parameters    │   │
│  │  state/best_genome.json        ← market-making genome    │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

---

## File-by-File Breakdown

### 1. `config.py` — Global constants
Plain key-value config. Defines API URLs (Gamma, CLOB), fee rates (maker 0.1%, taker 2%), genetic algorithm defaults (20 pop, 30 gens), and safety limits ($5 max live trade, DRY_RUN=True). Nothing runs here — just imported by other files.

### 2. `rtds_streamer.py` — Live price feed (standalone version)
Connects to `wss://ws-live-data.polymarket.com` via WebSocket. Subscribes to `crypto_prices` topic. Stores:
- `prices[sym]` → latest price
- `history[sym]` → last 1000 `(timestamp, price)` ticks

Has a `momentum()` helper that computes % deviation from 60-tick average. **Not actually used** — `intraday_trader.py` has its own embedded copy of `RTDSStreamer` with more features (signal detection, ATR vol, breakout classification).

### 3. `scanner.py` — Polymarket market discovery

```
Gamma API (REST)                CLOB API (REST)
    │                               │
    ▼                               ▼
fetch_binary_markets()         get_orderbook()
    │                               │
    └──────────┬────────────────────┘
               ▼
        Market dataclass
    (id, question, yes/no price,
     spread, liquidity, volume,
     best bid/ask, opp_score)
```

Fetches active binary markets from Gamma API, enriches each with real CLOB orderbook depth. Filters by liquidity, volume, spread, price range. Returns ranked list by `opportunity_score` (weighted combo of volume + liquidity + spread). Used by `paper_trader.py` and `executor.py`. **NOT used by `intraday_trader.py`** (which trades crypto prices, not binary prediction markets).

### 4. `strategy_genome.py` — Evolvable parameter DNA (for market-making)

```
StrategyGenome (18 parameters)
├── Market selection: min_liquidity, min_spread, min_volume, max_price, max_legs
├── Order placement: bid_offset_bps, ask_offset_bps, post_both_sides
├── Position sizing: max_position_pct, min/max_position_size, kelly_fraction
├── Risk: max_daily_loss_pct, max_positions, cancel_after_seconds
└── Market making: fill_probability, spread_multiplier, rebate_capture_bps

Operations:
  .mutate(rate=0.2)  → random perturbation of each param
  .crossover(a, b)   → single-point crossover
  .random_population(size) → fully randomized starting pop
```

This is the genetic representation for the **market-making** strategy (binary markets, spread capture). It is the DNA for `paper_trader.py` and `executor.py`. **It's NOT used by `intraday_trader.py`** which has its own simpler param dict.

`FitnessTracker` handles tournament selection + elite preservation across generations.

### 5. `engine.py` — Market-making simulation engine

```
SimulationEngine
    │
    ├── simulate_tick(genome)
    │   ├── Every 5 ticks: scanner.scan() → refresh markets
    │   ├── _place_orders() → maker limit orders inside spread
    │   │   (bid = mid - spread*multiplier*offset)
    │   │   (ask = mid + spread*multiplier*offset)
    │   └── _process_orders() → probabilistic fills or expire
    │       ├── Fill? → earn maker rebate (0.01%), add to positions
    │       └── Expire? → return locked capital
    │
    └── score_genome(genome, 120min)
        → Run N ticks, return (total_rebates / capital) * 100
```

Models Polymarket market-making with probabilistic fills. The key insight baked in: **makers earn rebates, takers pay fees**. Score = rebates earned as % of capital. This is how genomes are ranked during evolution. Used by `simulate.py`.

### 6. `simulate.py` — Genetic algorithm runner (for market-making)

```
for generation in range(30):
    for genome in population:
        score = engine.score_genome(genome, 120min)
    population = tracker.evaluate_and_select(population, scores)
    # top 3 survive (elitism) + tournament selection + crossover + mutation
save best genome → state/best_genome.json
```

Full evolutionary loop. Fetches live market snapshot, then runs each genome through 120-minute simulation cycles. Persists generation history to JSONL. **This is the v1 auto-improvement system for binary market-making.** Not connected to the intraday BTC trader.

### 7. `paper_trader.py` — Virtual money binary market trader

```
Loop every 30s:
  scanner.scan() → get live Polymarket markets
  _tick_pending() → probabilistic order fills (simulated)
  _resolve_positions() → check if market resolved → settle P&L
  _open_orders() → place new maker orders using genome params
  _save() → persist state
```

Places virtual maker orders on real Polymarket binary markets, simulates fills probabilistically, settles when markets resolve. Uses `StrategyGenome` params. Persists to `state/paper_trades.json`. **Currently dormant** — hasn't run since March 29.

### 8. `executor.py` — Real money CLOB executor

```
LiveExecutor
├── _init_client() → py_clob_client with Polygon wallet
├── place_maker_order() → real CLOB limit order (or dry-run)
├── cancel_order() → cancel open order
├── scan_and_trade() → scan markets + place per genome
└── run() → main loop with daily spend limits

Safety: DRY_RUN=True by default, MAX_DAILY_SPEND=$20
Needs: POLYMARKET_PRIVATE_KEY, POLYMARKET_FUNDER_ADDRESS
Status: Bankroll ~$0.71 (needs $20-50 minimum)
```

The real-money executor. Takes the best evolved genome and places actual CLOB orders. Has daily spend cap, graceful shutdown (cancels all orders on SIGINT). **Never activated with real trades** — insufficient bankroll.

### 9. `intraday_trader.py` — THE ACTIVE SYSTEM (1033 lines, 4 classes)

This is the file that's actually been running. It contains **everything bundled together**:

```
┌─────────────────────────────────────────────────────────────┐
│  intraday_trader.py (monolith)                              │
│                                                             │
│  ┌─────────────────┐  ┌─────────────────────────────┐       │
│  │  RTDSStreamer    │  │  Technical Indicators       │       │
│  │  (embedded copy) │  │  _rsi(), _macd(),           │       │
│  │  WS → prices[]   │  │  _bollinger()               │       │
│  │  history[sym][]  │  │                             │       │
│  │  signal()        │  │  TAStrategy.check_entry()    │       │
│  └────────┬────────┘  └──────────────┬──────────────┘       │
│           │                          │                       │
│           ▼                          ▼                       │
│  ┌───────────────────────────────────────────────────┐       │
│  │  IntradayTrader (main loop)                       │       │
│  │                                                   │       │
│  │  Every 5s:                                        │       │
│  │    _tick_positions()  → check SL/TP/timeout       │       │
│  │  Every 30s:                                       │       │
│  │    _scan_and_entry()  → momentum signals          │       │
│  │    OR _scan_and_entry_ta() → RSI/MACD/BB signals  │       │
│  │  Every 50s:                                       │       │
│  │    _save()  → persist to JSON                     │       │
│  └───────────────────────────────────────────────────┘       │
│                                                             │
│  ┌───────────────────────────────────────────────────┐       │
│  │  AutoImprove (GBM backtester)                     │       │
│  │                                                   │       │
│  │  GRID = {window: [10,20,60], mom_th: [...], ...}  │       │
│  │                                                   │       │
│  │  score_params():                                  │       │
│  │    Generate 720-tick GBM price path (synthetic)   │       │
│  │    Simulate trades with given params              │       │
│  │    Return (pnl, win_rate, n_trades, score)        │       │
│  │                                                   │       │
│  │  run(gens=5, pop=20):                             │       │
│  │    Random population from GRID                    │       │
│  │    For each generation:                           │       │
│  │      Score all → sort → top 5 survive             │       │
│  │      Mutate children → next generation            │       │
│  │    Return best_params                             │       │
│  └───────────────────────────────────────────────────┘       │
│                                                             │
│  Two entry strategies:                                       │
│  ┌──────────────┐         ┌───────────────────────┐          │
│  │ LEGACY       │         │ TA MODE (current)     │          │
│  │ Momentum     │         │                       │          │
│  │ breakout/    │    OR   │ overbought_oversold   │          │
│  │ breakdown/   │         │ mean_reversion        │          │
│  │ dip_buy/     │         │ breakout              │          │
│  │ rip_sell     │         │ multi_tf_confirm      │          │
│  └──────────────┘         └───────────────────────┘          │
└─────────────────────────────────────────────────────────────┘
```

**Currently active strategy:** `overbought_oversold` (RSI-only)
- RSI < 35 → go LONG ("oversold")
- RSI > 65 → go SHORT ("overbought")
- Stop loss: 0.2%, Take profit: 0.4%, Max hold: 600s
- Position size: 20% of capital per trade
- Only trades BTC/USD

### 10. `autoimprove.py` — AI-assisted auto-improvement

```
FLOW:
  1. Load trade journal stats (wins, losses, PnL)
  2. Call local AI (MiniMax via switchAILocal:18080)
     → "You're a quant, here are my last 15 trades, suggest params"
     → AI returns JSON with new params
  3. Run evolve() — 16 GBM backtests, mutating AI suggestion
  4. Save best → state/best_intraday_params.json

This is the Karpathy-style auto-research idea:
  Real trade data → AI analysis → parameter suggestion → GBM validation → deploy
```

**BUG:** The `ai_complete()` function has a malformed auth header — `-H "Bearer sk-..."` instead of `-H "Authorization: Bearer sk-..."`. AI suggestions fail silently, falling back to random evolution.

### 11. `continuous_improve.py` — Simple heuristic tuner

```
Every 10 minutes:
  Load state → check capital/PnL
  If winning big → tighten SL/TP, increase size
  If losing badly → widen RSI bands, reduce size
  If breakeven → random small tweak
  Clamp RSI to [30-45] / [55-70] range
  Save updated params
```

No AI, no backtesting — just rule-based adjustment. Currently **not running** (no process found).

---

## How the Pieces Connect (and Don't)

```
TWO SEPARATE SYSTEMS that DON'T TALK TO EACH OTHER:

System A: Binary Market-Making (DORMANT)            System B: Intraday BTC Trading (ACTIVE)
────────────────────────────────                     ──────────────────────────────────────
scanner.py → paper_trader.py                         RTDSStreamer → IntradayTrader
strategy_genome.py → engine.py → simulate.py         AutoImprove → GBM backtest
executor.py (real money, never used)                 autoimprove.py → AI + GBM
                                                     continuous_improve.py → heuristic tuning
state/best_genome.json                               state/best_intraday_params.json
state/paper_trades.json                              state/intraday_trades.json
```

**System A** was the original approach: market-making on Polymarket binary prediction markets. Evolved genomes for spread capture and rebate harvesting.

**System B** is what actually runs: BTC price momentum/RSI trading using the Polymarket RTDS WebSocket. Virtual money. The "auto-improvement" happens in two ways:
1. `AutoImprove` class inside `intraday_trader.py` — genetic grid search over GBM-simulated price paths
2. `autoimprove.py` — calls local AI for suggestions, then validates with GBM

---

## The Auto-Improvement Loop (What Should Be Running)

```
                    ┌──────────────────────────┐
                    │   LIVE TRADER RUNNING     │
                    │   (intraday_trader.py)    │
                    │                           │
                    │   Real RTDS prices        │
                    │   Virtual trades          │
                    │   Writes journal.jsonl    │
                    └─────────┬────────────────┘
                              │
                    trade results accumulate
                              │
                              ▼
                    ┌──────────────────────────┐
                    │   AUTOIMPROVE            │
                    │   (autoimprove.py)        │
                    │                           │
                    │   1. Read journal stats   │
                    │   2. Ask AI for ideas  ←──── switchAILocal (MiniMax)
                    │   3. GBM backtest 16x    │
                    │   4. Save best params    │
                    └─────────┬────────────────┘
                              │
                    writes best_intraday_params.json
                              │
                              ▼
                    ┌──────────────────────────┐
                    │   TRADER PICKS UP        │
                    │   new params on restart  │
                    │   (or continuous_improve  │
                    │    tweaks live)            │
                    └──────────────────────────┘
```

---

## Current State (2026-03-31 08:20)

| Metric | Value |
|--------|-------|
| Capital | $94.94 (started $100 → -5.06% drawdown) |
| Total trades | 34 |
| Wins / Losses / Breakeven | 6 / 6 / 22 |
| Win rate | 50% (excluding breakevens) |
| Open positions | 1 |
| Strategy | RSI overbought/oversold on BTC/USD |

### Running Processes

| Process | Status | Notes |
|---------|--------|-------|
| `intraday_trader.py` | **STOPPED** | Last log at 08:00. No PID found. |
| `continuous_improve.py` | **STOPPED** | No PID found. |
| `autoimprove.py` | **STOPPED** | No PID found. |
| Monitor loop (PID 32157) | Running | Reads state JSON every 30min. Does NOT trade or improve. |

---

## Key Problems Found

1. **Nothing is actually trading.** The trader stopped ~2 hours ago. Only a passive monitoring shell loop survives.

2. **Auto-improvement is disconnected from live trading.** The trader loads params once at startup from `best_intraday_params.json`. It never hot-reloads. So even if `autoimprove.py` or `continuous_improve.py` write new params, the trader won't see them until restarted.

3. **AI suggestion is broken.** `autoimprove.py:39` has `-H "Bearer sk-test-123"` instead of `-H "Authorization: Bearer sk-test-123"`. The AI call silently fails.

4. **GBM backtest is synthetic, not real data.** Both AutoImprove classes generate random Geometric Brownian Motion paths — they don't use the actual BTC price history from RTDS. So the "auto-improvement" optimizes for synthetic random walks, not real market dynamics.

5. **Two genome systems that don't talk.** `StrategyGenome` (18 params for market-making) and the intraday param dict (12 params for momentum/TA) are completely separate. The Karpathy-style evolutionary loop (`simulate.py` + `engine.py` + `strategy_genome.py`) only works for the dormant binary market system.

6. **65% of trades exit on time limit** with near-zero PnL. The RSI(35/65) signals aren't finding strong enough reversals to hit the 0.2% SL / 0.4% TP before the 600s timeout.

7. **No process supervisor.** Everything relies on manual `python3 intraday_trader.py` in a terminal. When it dies, nothing restarts it.
