---
name: autoimprove
description: "Use when asked to auto-improve, optimize, or evolve any Harvey agent. Runs ALL improvement powers: Superbrain knowledge, strategy evolution, SKILL.md quality, LLM autoresearch."
version: 1.0.0
tags: [self-improvement, autoresearch, karpathy, meta-harness, genetic-algorithm, optimization]
---

# Unified Auto-Improve

## What It Does

When you say "autoimprove trading agent" (or any agent), this runs **four improvement phases** using ALL of Harvey's improvement powers:

| Phase | What | How |
|-------|------|-----|
| **1. Knowledge** | Gather context about failures + wins | Superbrain queries Brain journals, video transcripts, multimodal docs |
| **2. Strategy** | Evolve parameters via genetic algorithm | Agent's `autoimprove.py` + GBM backtest (trading agents) |
| **3. Skills** | Improve SKILL.md documentation quality | Meta-harness behavioral eval + LLM rewriting |
| **4. Research** | LLM-driven analysis + new ideas | switchAILocal analyzes failures, suggests 3 concrete improvements |

Everything is logged to the Brain journal.

## Trigger Commands

| Phrase | Effect |
|--------|--------|
| `autoimprove trading agent` | Full 4-phase improvement on arbitrage-agent |
| `autoimprove career agent` | Full improvement on career-manager |
| `autoimprove trading --only strategy` | Just strategy parameter evolution |
| `autoimprove trading --only skills` | Just SKILL.md quality |
| `autoimprove trading --only research` | Just LLM autoresearch |

## Usage

```bash
# Full autoimprove (all 4 phases)
python3 $HARVEY_HOME/harvey-os/core/superbrain/autoimprove.py trading

# Just strategy evolution
python3 $HARVEY_HOME/harvey-os/core/superbrain/autoimprove.py trading --only strategy

# Career agent
python3 $HARVEY_HOME/harvey-os/core/superbrain/autoimprove.py career
```

## Agent Aliases

| Alias | Agent |
|-------|-------|
| `trading`, `trader`, `arbitrage` | arbitrage-agent |
| `career`, `crm` | career-manager |
| `knowledge`, `docs` | multimodal-knowledge |
| `pg`, `postgres` | pg-watchdog |
| `extractor` | knowledge-extractor |

## The Improvement Chain

```
"autoimprove trading agent"
  ↓
PHASE 1: KNOWLEDGE (Superbrain)
  → "What trades failed recently?" → Brain journals
  → "What strategies worked?" → Brain pages + video transcripts
  → "What does research say?" → Meta Harness video chunks
  ↓
PHASE 2: STRATEGY (Genetic Algorithm)
  → Load current params from config
  → AI suggests new params based on trade history
  → Evolve 16 variants via mutation
  → GBM backtest each variant
  → Keep best (if better than current)
  ↓
PHASE 3: SKILLS (Meta-Harness)
  → Analyze SKILL.md for gaps (9 gap types)
  → Score baseline in tmux sandbox (0-100)
  → LLM improves each gap
  → Score improved version
  → Keep if delta > 0
  ↓
PHASE 4: RESEARCH (LLM Autoresearch)
  → Feed failures + insights + research to LLM
  → Get 3 concrete recommendations:
    1. Parameter change (specific numbers)
    2. Strategy change (new approach)
    3. Risk management improvement
  → Log to Brain journal for human review
  ↓
LOG TO BRAIN
  → Summary of all phases
  → Specific suggestions for human review
```

## What Powers It Uses

| Power | Source | Used In |
|-------|--------|---------|
| Superbrain | `core/superbrain/superbrain.py` | Phase 1: knowledge gathering |
| Genetic Algorithm | `agents/arbitrage-agent/autoimprove.py` | Phase 2: strategy evolution |
| GBM Backtest | `agents/arbitrage-agent/autoimprove.py` | Phase 2: fitness evaluation |
| Meta-Harness | `agents/meta-harness-agent/agent.py` | Phase 3: behavioral eval |
| Gap Analyzer | `skills/meta/autoimprover/evaluate_skill.py` | Phase 3: skill quality |
| Video Knowledge | PG multimodal_documents (55 transcribed chunks) | Phase 1 + Phase 3 |
| switchAILocal | localhost:18080 (383 models) | Phase 3 + Phase 4 |
| Brain Journal | `data/Brain/journals/` | All phases: logging |
