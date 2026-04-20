---
name: meta-harness
description: "Use when evaluating or improving Harvey skills via behavioral testing. Based on Stanford IRIS Lab's Meta-Harness (Terminal-Bench 2.0, 76.4% on Claude Opus 4.6). Runs agents in tmux sandboxes."
version: 1.0.0
tags: [self-improvement, evaluation, testing, autoresearch, meta]
---

# Meta-Harness Agent — Behavioral Skill Evaluator

## What It Does

Evaluates Harvey skills by running them in a sandboxed tmux session and scoring compliance 0-100. Based on Stanford IRIS Lab's Meta-Harness paper (Terminal-Bench 2.0).

Used by the **autoimprover** to measure whether skill improvements actually help.

## Trigger Commands

| Phrase | Effect |
|--------|--------|
| `evaluate skill dev/writing-skills` | Run behavioral evaluation on a skill |
| `run autoimprover` | Run the nightly improvement cycle manually |
| `improve skill blockchain/polymarket` | Evaluate + improve a specific skill |

## Architecture

```
autoimprover/run_improvements.py (cron: 4 AM daily)
  ↓
evaluate_skill.py
  ├─ analyze_skill_gaps() → find gaps in SKILL.md
  ├─ evaluate_with_llm() → score baseline (0-100)
  │   ├─ _run_meta_harness() → subprocess → tmux sandbox eval
  │   ├─ LLM scoring fallback
  │   └─ random fallback
  ├─ improve_gap() → LLM improves skill content
  │   └─ _query_video_knowledge() → PG query for Meta Harness video insights
  └─ evaluate_with_llm() → score improved (0-100)
      → delta > 0? → write improved SKILL.md
```

## The Evaluation Chain

### Level 1: Meta-Harness Agent (Real Behavioral)
```
agents/meta-harness-agent/run_skill_evaluation.py
  → Spawns tmux session
  → Injects SKILL.md into agent context
  → Agent executes scenario in sandbox
  → Scores: success rate, turn count, compliance
  → Returns 0-100 score
```

### Level 2: LLM Scoring (Fallback)
```
switchAILocal → rate skill compliance 0-100
```

### Level 3: Random Baseline (Last Resort)
```
No skill loaded: random 40-60
Skill loaded: random 70-90
```

## Video Knowledge Integration

The `improve_gap()` function queries PostgreSQL for transcribed video chunks from the "AI Self EVOLUTION (Meta Harness)" video (55 chunks, ~4500 words). This enriches the LLM improvement prompt with real insights from the Stanford paper presentation.

## Agent Components

| File | Purpose |
|------|---------|
| `agents/meta-harness-agent/agent.py` | AgentHarness: 3 tools (execute_commands, task_complete, image_read) |
| `agents/meta-harness-agent/config.py` | Harvey OS paths, switchAILocal config |
| `agents/meta-harness-agent/skill_environment.py` | SkillEnv: sandbox setup, SKILL.md injection |
| `agents/meta-harness-agent/run_skill_evaluation.py` | CLI: baseline vs improved eval, scoring |
| `agents/meta-harness-agent/tbench_integration.py` | Terminal-Bench 2.0 task loader |

## Autoimprover Integration

| File | Purpose |
|------|---------|
| `skills/meta/autoimprover/evaluate_skill.py` | Gap analysis + evaluation + improvement engine |
| `skills/meta/autoimprover/run_improvements.py` | Cron runner (processes 1-3 skills per run) |
| `skills/meta/autoimprover/skills_to_improve.md` | Priority list of skills to improve |
| `skills/meta/autoimprover/results.tsv` | Historical evaluation results |

## Cron Schedule

```
# Active in loops/SKILL.md
0 4 * * * python3.11 -u $HARVEY_HOME/harvey-os/skills/meta/autoimprover/run_improvements.py >> $HARVEY_HOME/data/logs/autoimprover.log 2>&1
```

## Dependencies

- **tmux** — `brew install tmux` (required for sandboxed execution)
- **openai** SDK — `pip install openai` (for switchAILocal calls)
- **psycopg2** — `pip install psycopg2-binary` (for video knowledge queries)

## Key Innovation (from Stanford Paper)

**Environment bootstrapping:** Before the agent loop starts, the harness snapshots the sandbox state (filesystem, running processes, environment variables). This is injected into the agent's first prompt, eliminating 2-5 exploration turns that would otherwise be wasted discovering the environment.
