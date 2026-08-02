---
name: dspy
description: Compile prompts offline with DSPy 3.x (signatures, MIPROv2/GEPA optimizers) and ship the result as dependency-free string constants — the pattern validated on Makakoo's intelligent router
version: 2.0.0
author: Makakoo OS
license: MIT
dependencies: [dspy]
metadata:
  hermes:
    tags: [Prompt Optimization, DSPy, MIPROv2, GEPA, Compile Offline, Eval Discipline, LM Programming]

---

# DSPy 3.x: Compile Prompts Offline, Ship Strings

Everything in this skill was verified against **dspy 3.2.1** during the
2026-08 router-compiler sprint. The worked, production-proven example lives in
this repo: `harvey-os/core/evals/t2_compile_router.py` (compile),
`t2_bake.py` (bake), `t2_evaluate_candidate.py` (held-out eval). Result:
router intent accuracy 36% → 80% on a frozen test split, with zero new
runtime dependencies.

## When to Use

- You have a **prompt with a measurable output** (classification, extraction,
  short structured answers) and at least ~40 labeled examples.
- You can spend LM calls **offline** to buy quality **at runtime**.
- You want the runtime artifact to be **plain strings**, reviewable in a PR.

When *not* to use: judgment tasks with no ground truth, agents whose "output"
is a long interaction, or anywhere you'd need a custom subprocess `BaseLM`
adapter — DSPy 3.x is mid-migration on the `BaseLM.forward` contract
("legacy" vs "typed_lm"), so custom adapters break across minor versions.
Point `dspy.LM` at any OpenAI-compatible endpoint instead.

## Install (offline venv only — never a runtime dependency)

```bash
python3.12 -m venv tmp/dspy-venv
# litellm ships a Rust extension that fails to build on older rustc;
# force the prebuilt wheel. optuna is required by MIPROv2 but not declared.
tmp/dspy-venv/bin/pip install --only-binary litellm "dspy==3.2.1" optuna
```

## Core Model (3.x API)

```python
import dspy
from typing import Literal

# Any OpenAI-compatible endpoint; model routing is the operator's choice.
lm = dspy.LM("openai/<model>", api_base=BASE, api_key=KEY,
             temperature=0.0, max_tokens=800)
dspy.configure(lm=lm, adapter=dspy.JSONAdapter())   # not dspy.settings.configure

class RouteIntent(dspy.Signature):
    """One-line task description — the optimizer evolves this."""
    request: str = dspy.InputField(desc="the user's request, verbatim")
    intent: Literal["research", "image", "archive", "minimal", "unknown"] = \
        dspy.OutputField()

program = dspy.Predict(RouteIntent)        # or dspy.ChainOfThought(RouteIntent)
```

- **Signatures** are typed input→output declarations; `Literal` output fields
  give you a closed label space enforced at parse time.
- **JSONAdapter** renders the request messages and parses the JSON reply;
  its exact rendering is what you must reproduce if you bake (below).

## Optimize (MIPROv2)

```python
def metric(example, prediction, trace=None):
    return example.intent == prediction.intent

optimizer = dspy.MIPROv2(
    metric=metric, auto="light", seed=seed,
    prompt_model=big_budget_lm,   # instruction proposer writes long text —
    task_model=lm,                #   reasoning models truncate under the
)                                 #   task budget; give proposals ≥8k tokens
compiled = optimizer.compile(program, trainset=train,
                             requires_permission_to_run=False)
compiled.save("compiled_seedN.json")
```

Run **multiple seeds** and select on a dev split you never optimized on;
score with `dspy.Evaluate(devset=dev, metric=metric)`. In the router sprint,
three seeds spread 60–80% dev accuracy — a single seed is a coin flip.

`dspy.GEPA` is the reflective alternative: its metric returns
`dspy.Prediction(score=..., feedback="...")` and textual feedback (including
parse failures) becomes training signal. Prefer it when failures carry
explainable structure; MIPROv2 is the cheaper default.

## Bake: Ship Strings, Not DSPy

The compiled program = evolved instructions + selected demos. Render it once
with DSPy's own adapter, freeze the result as constants, golden-test the
freeze:

```python
adapter = dspy.JSONAdapter()
messages = adapter.format(compiled.signature, compiled.demos,
                          {"request": probe})
# system + demo messages are constant; the final user message is
# prefix + request + suffix. Emit a stdlib-only module with those strings
# and a render(request) function, plus the adapter output for several
# probes as a golden corpus. See t2_bake.py for the working generator.
```

The golden tests are the contract: if the stdlib `render()` ever disagrees
with what DSPy rendered, the runtime is no longer sending the prompt that
was measured, and every recorded score stops applying.

## Eval Discipline (what makes the number real)

The compile is the easy half. Non-negotiables, all implemented in
`harvey-os/core/evals/`:

1. **Frozen splits** — train (optimize), dev (select), test (decide). Test is
   read once, under a locked decision id; the scoreboard refuses a second run.
2. **Answer-key provenance** — human labels, or committee-unanimous as the
   documented fallback; a single model may never label the test split.
3. **Paired comparison** — candidate vs incumbent on the same examples:
   bootstrap CI on the accuracy delta, McNemar, per-class regressions.
4. **Measured model identity** — record `(resolved_model,
   system_fingerprint)` per response; refuse to promote on unstable identity.
   Gateway pool aliases are round-robin: never assume, always measure.

## Failure Modes Hit in Practice

| Symptom | Cause | Fix |
|---|---|---|
| `AdapterParseError` mid-compile, reply truncated mid-JSON | reasoning model spends the token budget thinking | separate `prompt_model` with ≥8k `max_tokens` |
| `ImportError: MIPROv2 requires 'optuna'` | undeclared optional dep | install `optuna` in the venv |
| litellm wheel build fails (maturin/rustc) | Rust extension vs old toolchain | `pip install --only-binary litellm` |
| compiled instruction omits a label | proposer summarizes the label space | keep the full `Literal` contract in the adapter format; golden-test that every label stays reachable |
| runtime scores below compile-time dev score | baked renderer drifts from adapter output | golden corpus rendered by DSPy itself, asserted in CI |

## References

- `references/optimizers.md` — optimizer selection notes
- Worked example (this repo): `harvey-os/core/evals/t2_compile_router.py`,
  `t2_bake.py`, `t2_evaluate_candidate.py`
- Upstream docs: https://dspy.ai/
