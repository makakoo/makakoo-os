---
name: arbitrage-agent
description: Polymarket BTC/ETH intraday trading bot — genetic evolution, real market prices, paper trading, auto-improvement. Trades real Polymarket binary markets when BTC/ETH short-term price prediction markets appear.
version: 7.0
author: Harvey OS
tags: [polymarket, trading, intraday, btc, crypto, evolution, paper-trading]
---

# Arbitrage Agent v7 — Polymarket BTC/ETH Intraday Trading Bot

## Core Philosophy

**Real Polymarket data, virtual money, genetic evolution.** The goal is a profitable intraday trading bot that:
1. Paper-trades real Polymarket BTC/ETH price-target markets (e.g., "Will BTC be above $76,000 on April 2?")
2. Evolves strategy parameters via genetic algorithms against live data
3. Auto-improves nightly
4. Graduates to real money trading when bankroll is sufficient

---

## Directory Structure

```
agents/arbitrage-agent/
├── SKILL.md                      ← This file
├── requirements.txt
│
├── Core Trading
├── polymarket_paper_trader.py    ← Paper trader v5: real binary math, 2% fee
├── crypto_signals.py             ← BTC price vs target signal logic (NEW v7)
├── crypto_price_scanner.py       ← BTC/ETH market scanner (fixed $1m parsing)
├── scanner.py                    ← Generic Polymarket scanner
├── strategy_genome.py            ← Evolvable genome + genetic operators (+max_hours)
│
├── Orchestration
├── intraday_monitor.py           ← Continuous trading daemon (NEW v7)
├── autoimprove_live.py           ← Evolution loop: 12 mutants × 30min sessions
│
├── Legacy (do not use)
├── paper_trader.py               ← BROKEN: fake BTC spot prices
├── intraday_trader.py            ← OLD: momentum-based, wrong for price-target markets
└── btc_sniper_v5.py              ← OLD: momentum-based sniper
```

---

## How to Run

### Continuous Monitor (production daemon)
```bash
cd ~/MAKAKOO/agents/arbitrage-agent
nohup python3 intraday_monitor.py --capital 100 >> logs/intraday_monitor.log 2>&1 &
```
Polls every 30s. Uses best evolved genome. Falls back to default params if none found.

### Manual Paper Trader
```bash
python3 polymarket_paper_trader.py --capital 100 --poll 30
```

### Evolution (one generation = 12 mutants × 30min sessions = ~6 hours)
```bash
cd ~/MAKAKOO/agents/arbitrage-agent
python3 autoimprove_live.py
```
Runs nightly at 5 AM via cron. Uses PolymarketPaperTrader + CryptoPriceScanner.

---

## Signal Logic (v7 — BTC Price vs Target)

**Critical insight:** Polymarket BTC markets are phrased as price-target questions:
- "Will the price of Bitcoin be above $76,000 on April 2?"
- "Will Bitcoin dip to $66,000 March 30-April 5?"

Momentum-based signals (v5) are WRONG for these markets. The only signal that matters is:
- **Current BTC price vs target price**

```python
# Example:
# BTC at $74,000, market "Will BTC be above $76,000?" → YES implied prob = 0.48
# Model: BTC needs +2.7% in remaining time → reasonable probability
# If market implies < model probability → BUY YES
# If market implies > model probability → SELL NO (fade)
```

The `crypto_signals.py` module implements this with:
- `btc_price_signal()` — compares BTC price to target, returns TradingSignal
- `momentum_confirmation()` — filters false breakouts
- `should_trade()` — genome-based gate for confidence/liquidity filters

---

## Polymarket Binary Market Math

```
Market: "Will BTC be above $76,000 on April 2?"
YES price = 0.48, NO price = 0.52

Buy YES at $0.48:
  cost = 0.48 × 100 shares = $48
  fee = $48 × 2% = $0.96
  Net cost = $48.96

If BTC > $76,000 at expiry → YES resolves $1.00
  payout = 100 × $1.00 = $100
  gross_pnl = $100 - $48 = +$52
  net_pnl = $52 - $0.96 = +$51.04

If BTC ≤ $76,000 at expiry → YES resolves $0.00
  payout = 0
  net_pnl = -$48.96
```

---

## Key Parameters (StrategyGenome)

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `max_hours` | 48.0 | 1-200 | Max hours until market resolution |
| `max_position_pct` | 10% | 2-30% | Capital per position |
| `kelly_fraction` | 25% | 5-100% | Kelly criterion sizing |
| `fill_probability` | 30% | 5-80% | Maker fill probability |
| `cancel_after_seconds` | 600 | 60-900 | Limit order TTL |
| `max_positions` | 5 | 1-10 | Max concurrent positions |
| `min_liquidity` | $1000 | $100-5000 | Min market liquidity |

---

## Market Availability

**Polymarket does NOT always have short-term BTC/ETH markets.** The bot monitors continuously and trades when they appear. When no crypto markets exist:
- Monitor daemon continues polling (every 30s)
- Evolution runs on whatever markets appear during sessions
- The system generates P&L data from all available Polymarket markets

Typical Polymarket crypto markets:
- "Will Bitcoin be above $X on [date]?" (price-target)
- "Will Ethereum reach $X in [month]?" (price-target)
- "Will BTC dip to $X before [date]?" (dip market)
- "Up or Down" markets for DOGE/ALT (direction-only)

---

## State & Data

Runtime state in `~/MAKAKOO/data/arbitrage-agent/v2/`:
- `state/best_genome.json` — best evolved genome (score, timestamp)
- `state/intraday_journal.jsonl` — all paper trades (net_pnl + pnl keys)
- `logs/intraday_monitor.log` — continuous monitor output
- `logs/autoimprove_live.log` — evolution output

---

## Environment

For live trading (not yet enabled):
- `POLYMARKET_PRIVATE_KEY`
- `POLYMARKET_FUNDER_ADDRESS`

Current status: **Paper trading only.** Bankroll is virtual.

---

## Cron Jobs

```cron
# Nightly evolution (5 AM) — one generation per night
0 5 * * * cd ~/MAKAKOO/agents/arbitrage-agent && python3 autoimprove_live.py >> logs/autoimprove_live.log 2>&1

# Continuous intraday monitor (restart every 6h to pick up new genomes)
*/10 * * * * pgrep -f intraday_monitor || (cd ~/MAKAKOO/agents/arbitrage-agent && nohup python3 intraday_monitor.py >> logs/intraday_monitor.log 2>&1)
```

---

## Important

- **v4 paper_trader.py is BROKEN** — used fake BTC spot prices ($66,887), wrong fee (5bps), position sizing in satoshis. The 77 trades / 21% WR journal is garbage.
- **v5 polymarket_paper_trader.py is CORRECT** — real Polymarket Gamma API, correct binary math, 2% taker fee.
- **Signal logic (v7) is correct** — BTC price vs target, NOT momentum.
- Bankroll: virtual $100. Live trading requires $20-50 minimum and explicit enablement.
