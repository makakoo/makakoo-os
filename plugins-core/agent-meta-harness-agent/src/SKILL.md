# Meta Harness Agent — SKILL.md

**Category:** meta
**Producer:** meta-harness-agent
**Type:** Agent scaffold + skill compliance evaluation harness

## What This Agent Does

Behavioral skill compliance evaluation using real agent execution. Based on Stanford IRIS Lab's Meta-Harness (Terminal-Bench 2.0: 76.4% on Claude Opus 4.6).

**Replaces** the mock LLM scoring in `evaluate_skill.py` with **real agent behavior measurement** — executes Harvey in a tmux sandbox against pressure test scenarios, measures actual compliance delta with vs. without the skill loaded.

## Core Innovation

**Environment bootstrapping:** Before the agent loop starts, gather a snapshot of the sandbox environment (working directory, file listing, available languages/tools, package managers) and inject it into the initial prompt. This eliminates 2-5 early exploration turns.

## Integration with Autoresearch

```
evaluate_skill.py (autoresearch loop)
       │
       ├── BASELINE:  run_meta_harness(skill=None, scenario=task)
       │              → execution without SKILL.md → score_0
       │
       ├── IMPROVE:  improve_gap(skill_content, gap_id)
       │              → LLM edits SKILL.md → improved_content
       │
       └── IMPROVED:  run_meta_harness(skill=improved_content, scenario=task)
                       → execution with SKILL.md → score_1
                       → delta = score_1 - score_0
                       → git commit if delta > 0, else git reset
```

## Trigger Phrases

| Phrase | Effect |
|--------|--------|
| `test skill compliance` | Run full meta-harness evaluation on a skill |
| `benchmark skill` | Run Terminal-Bench tasks against a skill |
| `run meta harness` | Execute meta-harness agent for a specific scenario |
| `skill pressure test` | Stress-test a skill with hard tasks |

## Operating Procedure

### Test a single skill (full compliance evaluation)

```bash
python3 agents/meta-harness-agent/run_skill_evaluation.py \
    --skill dev/writing-skills \
    --scenario "Implement a fibonacci function with TDD" \
    --with-skill \
    --model minimax:MiniMax-M2.7
```

### Compare baseline vs. improved (autoresearch loop)

```bash
# Without skill (baseline)
python3 agents/meta-harness-agent/run_skill_evaluation.py \
    --skill dev/writing-skills \
    --scenario tbench:medium/55 \
    --with-skill false

# With improved skill
python3 agents/meta-harness-agent/run_skill_evaluation.py \
    --skill dev/writing-skills \
    --scenario tbench:medium/55 \
    --with-skill true \
    --skill-content "$(cat harvey-os/skills/dev/writing-skills/SKILL.md)"
```

### Run a Terminal-Bench task directly

```bash
python3 agents/meta-harness-agent/run_skill_evaluation.py \
    --scenario "Create a Python HTTP server that returns 'Hello World'" \
    --max-turns 20
```

## Architecture

```
meta-harness-agent/
├── SKILL.md                    ← this file
├── README.md                   ← setup and usage guide
├── requirements.txt           ← Python deps
├── config.py                  ← Harvey OS paths, AI gateway config
├── agent.py                   ← AgentHarness: tmux + OpenAI SDK (switchAILocal), 3 tools
├── run_skill_evaluation.py    ← main entry: baseline vs. improved eval
├── skill_environment.py        ← SkillEnv: sandbox setup, SKILL.md injection
└── tbench_integration.py      ← Terminal-Bench 2.0 task loader
```

## Tools (3 native tools)

| Tool | Description |
|------|-------------|
| `execute_commands` | Send keystrokes to tmux session, poll for completion with marker |
| `task_complete` | Double-confirmation task completion ( checklist before finalizing ) |
| `image_read` | Base64 image → multimodal LLM analysis |

## Data

- **State:** `data/meta-harness-agent/state/` — per-evaluation run state
- **Logs:** `data/meta-harness-agent/logs/` — tmux output, trajectories, metrics
- **Results:** `data/meta-harness-agent/results.tsv` — evaluation results (skill, scenario, score, delta)

## Configuration

All config via environment or `config.py`:
- `SWITCHAI_URL` / `SWITCHAI_KEY` — AI gateway (Harvey's switchAILocal at localhost:18080)
- `LLM_MODEL` — model to use (default: `minimax:MiniMax-M2.7`)
- `TBENCH_DATA_DIR` — path to Terminal-Bench 2.0 data (optional)

## Dependencies

- Python 3.11+
- tmux (must be installed)
- openai (OpenAI SDK — talks directly to switchAILocal at localhost:18080)
- All dependencies in `requirements.txt`

## Terminal-Bench 2.0

Benchmark data must be obtained separately from [tbench.ai](https://tbench.ai). The agent works without it — you can provide custom scenarios as free-form task descriptions.

89 tasks across 3 difficulty tiers:
| Tier | Count | Score (Claude Opus 4.6) |
|------|------:|------------------------:|
| Easy | 4 | 100.0 |
| Medium | 55 | 81.1 |
| Hard | 30 | 64.7 |
| **All** | **89** | **76.4** |
