# Meta Harness Agent

**Behavioral skill compliance evaluation using real agent execution.**
Based on Stanford IRIS Lab's Meta-Harness (76.4% on Terminal-Bench 2.0).

## What It Does

Replaces the mock LLM scoring in Harvey's `evaluate_skill.py` autoresearch loop with **real behavioral measurement** — executes the agent in a tmux sandbox and measures actual compliance delta with vs. without a skill loaded.

```
Baseline:  agent WITHOUT skill → tmux sandbox → score_0
Improved:  agent WITH skill    → tmux sandbox → score_1
Delta:     score_1 - score_0  → git commit if > 0
```

## Quick Start

```bash
# Install dependencies
pip install -r requirements.txt

# Baseline (no skill)
python3 run_skill_evaluation.py --scenario "Create a Fibonacci function in Python" --baseline

# With skill loaded
python3 run_skill_evaluation.py --skill dev/writing-skills --with-skill

# Compare delta
python3 run_skill_evaluation.py --skill dev/writing-skills --baseline
python3 run_skill_evaluation.py --skill dev/writing-skills --with-skill

# Custom scenario
python3 run_skill_evaluation.py --scenario "Implement a TDD workflow for a Python module"

# Terminal-Bench 2.0 task (requires https://tbench.ai data)
TBENCH_DATA_DIR=./tbench2 python3 run_skill_evaluation.py --scenario tbench:medium/12 --with-skill
```

## Architecture

```
meta-harness-agent/
├── agent.py                  ← AgentHarness: tmux + OpenAI SDK (switchAILocal), 3 native tools
├── skill_environment.py      ← SkillEnv: sandbox setup, SKILL.md injection
├── tbench_integration.py     ← Terminal-Bench 2.0 task loader
├── run_skill_evaluation.py   ← CLI: baseline vs. improved evaluation
├── SKILL.md                 ← Skill manifest
├── README.md                ← this file
├── requirements.txt         ← openai
└── config.py               ← Harvey OS paths, AI gateway config
```

## Integration with Autoresearch

The `run_skill_evaluation.py` script is the bridge between Harvey's `evaluate_skill.py` and real agent execution:

```
evaluate_skill.py
  │
  ├── baseline_score = run_skill_evaluation.py --baseline
  │                     → score_0 (without skill)
  │
  ├── improve_gap() → LLM edits SKILL.md
  │
  └── improved_score = run_skill_evaluation.py --with-skill --improved-content "..."
                       → score_1 (with improved skill)
                       → delta = score_1 - score_0
                       → git commit if delta > 0, else git reset
```

## Terminal-Bench 2.0

Obtain data from [https://tbench.ai](https://tbench.ai) (89 tasks):

```bash
mkdir -p data/tbench2
# Download and extract tbench2 data to data/tbench2/
# Expected structure: easy/, medium/, hard/ with JSON files
TBENCH_DATA_DIR=./data/tbench2 python3 run_skill_evaluation.py \
    --scenario tbench:medium/12 --with-skill
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SWITCHAI_URL` | `http://localhost:18080/v1` | AI gateway |
| `SWITCHAI_KEY` | `sk-test-123` | API key |
| `LLM_MODEL` | `minimax:MiniMax-M2.7` | Model |
| `TBENCH_DATA_DIR` | `data/tbench2` | Terminal-Bench data path |

## Dependencies

- Python 3.11+
- tmux (must be installed)
- openai (OpenAI SDK — talks directly to switchAILocal at localhost:18080)
