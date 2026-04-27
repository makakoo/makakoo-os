"""
Harvey's Mascot GYM — self-improvement flywheel.

Five layers (v0.2 — adds CODE-class code patch generation):
  1. capture.py    — error funnel (this package boundary)
  2. classifier.py — rules-based error labeling + clustering
                      (adds CODE class: ImportError, AttributeError, KeyError, etc.)
  3. hypothesis.py — autoimprover (SKILL) + LLM diff generation (CODE)
                      Two hypothesis types: skill-class → SKILL.md edits;
                      code-class → unified diff patches for Python files
  4. lope_gate.py  — validator-pool consensus gate (both sprint types)
  5. approval.py   — human-in-the-loop merge CLI
                      Skill patches: write improved SKILL.md + git commit
                      Code patches: apply unified diff + py_compile verify + git commit

Nothing here merges code autonomously. The merge button lives in approval.py
behind an explicit `harvey improve approve <id>` command. That constraint is
load-bearing — do not route around it.

See development/sprints/SPRINT-HARVEY-GYM.md for the full design.
"""

from core.gym.capture import log_error, log_errors, ErrorSource

# NOTE: add_flag is intentionally not re-exported here. Importing
# core.gym.flag at package-load time triggers a RuntimeWarning when
# `python -m core.gym.flag` runs from the CLI (the module gets imported
# twice via different routes). Import add_flag directly from core.gym.flag.
GYM_VERSION = "0.2"

__all__ = ["log_error", "log_errors", "ErrorSource", "GYM_VERSION"]
