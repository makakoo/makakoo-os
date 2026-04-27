"""Tests for the heuristic memory-type classifier."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import classifier  # noqa: E402


def test_classify_user_preference_strong():
    body = """
    Sebastian prefers B2B contracts over employed roles.
    His rate floor is €60/hour.
    He's based in Spain and Madrid on-site is fine.
    I prefer terse responses.
    """
    assert classifier.classify(body) == "user"


def test_classify_project_sprint():
    body = """
    ## Decisions
    - Shipped SPRINT-FOO-BAR v0.6.1 with commit abc123
    - Phase 3 deliverable ready
    - Milestone released as tag v0.6.2

    ## Action Items
    - [ ] Finish phase 4 rollout [[Harvey Brain]]
    """
    assert classifier.classify(body) == "project"


def test_classify_feedback_corrections():
    body = """
    ## Mistakes Made
    - Shipped wrong install command, caught after publish
    - Should have tested from clean dir first
    - Lesson: never skip dogfood step
    - Gotcha: pre-commit hook was bypassed accidentally
    """
    assert classifier.classify(body) == "feedback"


def test_classify_reference_external_systems():
    body = """
    ## Facts Learned
    - Pipeline bugs tracked in Linear project INGEST
    - Grafana dashboard at https://grafana.internal/d/api-latency
    - See also https://notion.so/some-runbook
    - The #oncall Slack channel has the runtime
    - Confluence page has the deploy runbook
    """
    assert classifier.classify(body) == "reference"


def test_classify_thin_content_defaults_user():
    body = "one word"
    assert classifier.classify(body) == "user"


def test_reclassify_draft_updates_frontmatter(tmp_path):
    draft = tmp_path / "sample.md"
    draft.write_text(
        "---\n"
        "name: foo\n"
        "description: test\n"
        "type: user\n"
        "created: 2026-04-23T10:00:00Z\n"
        "---\n\n"
        "## Decisions\n"
        "- Shipped SPRINT-XYZ phase 1\n"
        "- Tagged v0.6.2 [[Makakoo OS]]\n"
        "- Deliverable ready for milestone\n"
    )
    result = classifier.reclassify_draft(draft)
    assert result == ("user", "project")
    assert "type: project" in draft.read_text()
    # Everything else preserved
    assert "name: foo" in draft.read_text()
    assert "Shipped SPRINT-XYZ" in draft.read_text()


def test_reclassify_draft_noop_when_matching(tmp_path):
    draft = tmp_path / "sample.md"
    body = (
        "---\n"
        "name: foo\n"
        "type: user\n"
        "---\n\n"
        "just a short note\n"
    )
    draft.write_text(body)
    assert classifier.reclassify_draft(draft) is None
    assert draft.read_text() == body


def test_reclassify_draft_missing_frontmatter_noop(tmp_path):
    draft = tmp_path / "no_fm.md"
    draft.write_text("just body, no frontmatter")
    assert classifier.reclassify_draft(draft) is None


def test_reclassify_all_tallies_by_new_type(tmp_path):
    (tmp_path / "a.md").write_text(
        "---\nname: a\ntype: user\n---\n\nshipped SPRINT-A v1.0 [[X]] [[Y]] phase 2\n"
    )
    (tmp_path / "b.md").write_text(
        "---\nname: b\ntype: user\n---\n\nmistake: should not have run git reset --hard, lesson learned, caveat\n"
    )
    (tmp_path / "c.md").write_text(
        "---\nname: c\ntype: user\n---\n\njust a short note\n"
    )
    tally = classifier.reclassify_all(tmp_path)
    assert tally.get("project", 0) == 1
    assert tally.get("feedback", 0) == 1
    # c stays user → not in tally (no change)
    assert "user" not in tally or tally.get("user", 0) == 0
