"""
GYM Simplicity Criterion + Polar Express magic constants.

Two autoreseach principles hardwired into the GYM pipeline:

1. POLAR_EXPRESS_COEFFS: hardcoded float literals from train.py.
   Used by gym_simplify.py to generate hypothesis bias scores.

2. SIMPLICITY_BIAS: proposals that delete code get +0.1 bonus.
   Proposals that add complexity get -(added_lines / 50) * 0.05 penalty.

3. HARVEY_HOME_GROWTH_BUDGET: fixed growth rate on plugin/code.
   GYM-improved code cannot grow indefinitely. New lines have a cost.
"""

from dataclasses import dataclass
from pathlib import Path
from typing import List, Tuple

HARVEY_HOME = Path.home() / "MAKAKOO"

# ── Polar Express orthogonal initialization coefficients ──────────────────────
# Copied verbatim from train.py's polar_express_coeffs list.
# Format: (a, b, c) per power-iteration step.

POLAR_EXPRESS_COEFFS: List[Tuple[float, float, float]] = [
    (8.156554524902, -22.483292957795, 15.878769915207),
    (4.042929351667, -2.808917465714, 0.500017845105),
    (3.891667802292, -2.772484153217, 0.506064817850),
    (3.285753657755, -2.368129493342, 0.464490242330),
    (2.346541325859, -1.709782838268, 0.423235511693),
]

# ── Simplicity criterion ───────────────────────────────────────────────
# Rationale: autoreseach values code deletion + eval harness. A 0.001 val_bpb win
# from deleting 20 lines > a 0.001 win from adding 200 lines. GYM inherits the same
# principle. Proposals that shrink the bootstrap/global.md + codebase wins.

SIMPLICITY_BONUS_DELETED = 0.1       # flat bonus for any deletion hypothesis
SIMPLICITY_COST_PER_LINE = 0.001    # penalty per net-new line added
SIMPLICITY_COST_COMPLEXITY_BIAS = True  # autoreseach-enforced simplicity criterion

# ── Growth budget ───────────────────────────────────────────────────
# Total Harvey OS bootstrap code growth cap. Hard limit.
# Authoritative: bootstrap/global.md must stay under MAX_BOOTSTRAP_LINES lines.
# Auto-improved additions must stay under MAX_ADDITIONS_PER_HYPOTHESIS lines.

MAX_BOOTSTRAP_LINES = 600          # hard cap (bootstrap/global.md)
MAX_ADDITIONS_PER_HYPOTHESIS = 200  # per approved improvement

# ── Gym eval harness defaults ──────────────────────────────────────────
#autoreseach-style fixed budget: harness runs for FIXED_EVAL_SECONDS then scores.
# Autoreseach trains for 5 min on the single task, scores val_bpb.  GYM runs
# a benchmark prompt set.  Both share the same fixed-time principle.

FIXED_EVAL_SECONDS = 300        # 5-minute budget matching autoreseach train.py
FIXED_EVAL_METRIC = "bits_per_byte"  # only eval_bpb from autoreseach matches
