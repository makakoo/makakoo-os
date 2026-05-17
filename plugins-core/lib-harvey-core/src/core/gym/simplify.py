"""
GYM Simplicity Criterion — autoreseach's simplicity_enforced principle.

autoreseach program.md:
  "Simplicity criterion: a simpler solution that works just as well
   should be preferred over a more complex one. [...] Delete code."

GYM rule:
  - Proposals that DELETE code get +0.1 bias toward approval
  - Proposals that ADD code get -(net_new_lines / 50) * 0.05 penalty
  - Deleting 10+ lines AND improving quality → strong approve signal
  - Adding >100 lines without quality gain → auto-reject signal

Usage:
    delta = simplicity_delta(old_path, new_path)  # → float bias
    if delta > 0.05: approve_simplification(id)
"""
from pathlib import Path
import difflib


def count_non_empty_lines(text: str) -> int:
    return sum(1 for line in text.splitlines() if line.strip())


def compute_simplicity_delta(old_text: str, new_text: str) -> float:
    """
    Returns a bias score in [-0.4, +0.1].

    Positive = simplification (good, autoreseach-aligned).
    Negative = complication (penalized).
    Zero = no net change.
    """
    old_lines = count_non_empty_lines(old_text)
    new_lines = count_non_empty_lines(new_text)
    delta = new_lines - old_lines  # positive = added, negative = deleted

    if delta < 0:
        # Deletion is good — flat +0.1 bonus (autoreseach simplicity criterion)
        return 0.1
    elif delta == 0:
        # No net change — neutral
        return 0.0
    else:
        # Added lines: linear penalty
        # 50 lines added → -0.05 penalty
        # 100 lines added → -0.10 penalty (cap)
        penalty = min(0.4, (delta / 50.0) * 0.05)
        return -penalty


def generate_diff(old_path: Path, new_path: Path) -> str:
    """Return a unified diff string for review."""
    old_text = old_path.read_text()
    new_text = new_path.read_text()
    return "\n".join(difflib.unified_diff(
        old_text.splitlines(keepends=True),
        new_text.splitlines(keepends=True),
        fromfile=str(old_path),
        tofile=str(new_path),
    ))


def summarize_delta(old_lines: int, new_lines: int) -> str:
    """Human-readable delta summary."""
    delta = new_lines - old_lines
    if delta < 0:
        return f"DELETED {abs(delta)} lines (+0.1 simplicity bias)"
    elif delta == 0:
        return "No net line change"
    else:
        penalty = min(0.4, (delta / 50.0) * 0.05)
        return f"ADDED {delta} lines (-{penalty:.2f} bias)"


if __name__ == "__main__":
    import sys
    old = Path(sys.argv[1]).read_text()
    new = Path(sys.argv[2]).read_text() if len(sys.argv) > 2 else ""
    print(f"Delta: {compute_simplicity_delta(old, new):.4f}")