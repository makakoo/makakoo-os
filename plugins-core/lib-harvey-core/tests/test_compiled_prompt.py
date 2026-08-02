"""Golden tests pinning the baked router prompt to DSPy's actual output.

``router_compiled_prompt.py`` is generated; these tests are what stops it from
drifting. ``golden_renders.json`` was rendered by DSPy's JSONAdapter for the
winning compiled program (by t2_bake.py in the offline compile toolchain) — if the
stdlib ``render()`` ever disagrees with it, the runtime is no longer sending
the prompt that was measured, and every scoreboard number for this candidate
stops applying.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from core.orchestration import router_compiled_prompt as rcp  # noqa: E402

GOLDEN = json.loads(
    (Path(__file__).parent / "golden_renders.json").read_text(encoding="utf-8")
)


def test_golden_corpus_matches_candidate():
    assert GOLDEN["candidate_id"] == rcp.CANDIDATE_ID


def test_render_reproduces_every_golden_probe():
    for probe, expected in GOLDEN["renders"].items():
        assert rcp.render(probe) == expected, f"drift for probe {probe!r}"


def test_render_embeds_arbitrary_request_verbatim():
    request = 'weird "input"\nwith [[ ## fields ## ]] and unicode ✓'
    messages = rcp.render(request)
    assert messages[-1]["role"] == "user"
    assert request in messages[-1]["content"]


def test_message_shape_is_system_demos_user():
    messages = rcp.render("x")
    roles = [m["role"] for m in messages]
    assert roles[0] == "system"
    assert roles[-1] == "user"
    assert roles[1:-1] == ["user", "assistant"] * len(rcp.DEMOS)


def test_all_intents_reachable_in_contract():
    """The instruction text may highlight fewer labels, but the format
    contract the model answers under must enumerate the full label space."""
    system_and_user = rcp.SYSTEM + rcp.USER_SUFFIX
    for intent in ("research", "image", "archive", "minimal", "unknown"):
        assert intent in system_and_user
