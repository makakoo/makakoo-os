# DSPy 3.2.x Optimizer Selection

Short, opinionated, and limited to what we have verified or what the 3.2.1
source actually exports. The older guide described 1.x-era teleprompters and
invented parameters; it is gone on purpose.

## Decision table

| You have | Use | Notes |
|---|---|---|
| ~40+ labels, classification/extraction | **MIPROv2** (`auto="light"`) | our default; needs `optuna`; run ≥3 seeds |
| Failures that can be *explained* in text | **GEPA** | metric returns `dspy.Prediction(score, feedback)`; feedback (incl. parse errors) drives reflection |
| Very few labels, want a fast floor | **BootstrapFewShot** | demo selection only, no instruction search |
| Instruction-only tuning, no demos wanted | **COPRO** | cheaper than MIPROv2, usually weaker |
| An epochal budget and a metric worth it | **SIMBA** | stochastic introspective mini-batch ascent |

## MIPROv2 — the verified path

```python
optimizer = dspy.MIPROv2(
    metric=metric,
    auto="light",              # or "medium"/"heavy"; scales trial count
    seed=seed,
    prompt_model=proposal_lm,  # big max_tokens (≥8k): proposals are long text
    task_model=runtime_lm,     # exactly the runtime candidate config
)
compiled = optimizer.compile(program, trainset=train,
                             requires_permission_to_run=False)
```

Hard-won specifics (router sprint, 2026-08):

- **`optuna` is required but not declared** — install it explicitly.
- **Split the two LM roles.** The instruction proposer writes free-form prose;
  a reasoning model truncates it under the task budget and the compile dies
  with `AdapterParseError`. The task model must stay at the runtime config,
  because its scores are only meaningful for the config that will ship.
- **Seeds disagree.** 13/42/77 scored 80/60/70 on dev. Select on dev, never
  on train, and never on test.
- **Wrap each seed in try/except** — one failed seed should cost you one
  seed, not the run.

## GEPA — when feedback is the signal

```python
def metric(gold, pred, trace=None, pred_name=None, pred_trace=None):
    score = float(gold.intent == pred.intent)
    feedback = "correct" if score else (
        f"expected {gold.intent}, got {pred.intent}; "
        "the request mentions a screenshot, which is the image signal"
    )
    return dspy.Prediction(score=score, feedback=feedback)
```

GEPA reflects on the feedback text to evolve instructions. Parse failures are
legitimate feedback — tell it *why* the output failed to parse. Costlier than
MIPROv2; reach for it when MIPROv2 plateaus and you can articulate failures.

## After optimizing — always

1. Evaluate per seed on **dev**, pick one winner.
2. **Bake** the winner to string constants with a DSPy-rendered golden corpus
   (see `harvey-os/core/evals/t2_bake.py`).
3. One **held-out test eval**, paired against the incumbent, under a locked
   decision id (see `t2_evaluate_candidate.py`).
