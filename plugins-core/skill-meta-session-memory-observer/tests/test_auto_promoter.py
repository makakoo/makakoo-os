"""Tests for the auto-promoter."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import auto_promoter  # noqa: E402


def _make_draft(dir_: Path, name: str, mem_type: str, body: str, desc: str = "sample") -> Path:
    path = dir_ / f"2026-04-23_{name}.md"
    path.write_text(
        f"---\n"
        f"name: {name}\n"
        f"description: {desc}\n"
        f"type: {mem_type}\n"
        f"---\n\n"
        f"{body}\n"
    )
    return path


def _setup(monkeypatch, tmp_path) -> tuple[Path, Path, Path]:
    """Point auto_promoter constants at a tmp dir."""
    drafts = tmp_path / "drafts"
    drafts.mkdir()
    root = tmp_path
    index = tmp_path / "MEMORY.md"
    index.write_text(
        "- [existing_entry](project_existing.md) — something already indexed\n"
    )
    monkeypatch.setattr(auto_promoter, "DRAFTS_DIR", drafts)
    monkeypatch.setattr(auto_promoter, "MEMORY_ROOT", root)
    monkeypatch.setattr(auto_promoter, "MEMORY_INDEX", index)
    return drafts, root, index


def _structured_body(chars: int = 300) -> str:
    """Build a body that passes the structured-memory gate."""
    return "## Decisions\n- Something decided\n\n" + ("x " * (chars // 2))


def test_user_type_promoted(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    _make_draft(drafts, "contract_prefs", "user", _structured_body(), desc="Sebastian prefers B2B")

    tally = auto_promoter.auto_promote(drafts)
    assert tally["promoted"] == 1

    # Live memory file exists
    assert (root / "user_contract_prefs.md").exists()
    # Index updated
    assert "contract_prefs" in index.read_text()
    assert "Sebastian prefers B2B" in index.read_text()
    # Draft removed, marker written
    assert not (drafts / "2026-04-23_contract_prefs.md").exists()
    assert (drafts / "2026-04-23_contract_prefs.promoted").exists()


def test_project_type_kept_as_draft(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    draft = _make_draft(drafts, "sprint_foo", "project", _structured_body())

    tally = auto_promoter.auto_promote(drafts)
    assert tally["kept_draft"] == 1
    assert tally["promoted"] == 0
    # Draft still present, no live file written
    assert draft.exists()
    assert not (root / "project_sprint_foo.md").exists()


def test_feedback_type_promoted(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    _make_draft(drafts, "test_rule", "feedback", _structured_body(250))
    tally = auto_promoter.auto_promote(drafts)
    assert tally["promoted"] == 1
    assert (root / "feedback_test_rule.md").exists()


def test_reference_type_promoted(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    _make_draft(drafts, "grafana_link", "reference", _structured_body(250))
    tally = auto_promoter.auto_promote(drafts)
    assert tally["promoted"] == 1
    assert (root / "reference_grafana_link.md").exists()


def test_thin_body_rejected(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    draft = _make_draft(drafts, "tiny", "user", "hi")
    tally = auto_promoter.auto_promote(drafts)
    assert tally["rejected_thin"] == 1
    assert not draft.exists()
    assert draft.with_suffix(".rejected").exists()
    assert not (root / "user_tiny.md").exists()


def test_duplicate_name_gets_timestamp_suffix(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    # Pre-existing live memory
    (root / "user_foo.md").write_text("already here")
    _make_draft(drafts, "foo", "user", _structured_body())

    tally = auto_promoter.auto_promote(drafts)
    assert tally["promoted"] == 1
    # Original preserved
    assert (root / "user_foo.md").read_text() == "already here"
    # Suffixed file created
    suffixed = [p for p in root.glob("user_foo_*.md")]
    assert len(suffixed) == 1


def test_skip_existing_markers(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    draft = _make_draft(drafts, "already_done", "user", _structured_body())
    draft.with_suffix(".promoted").write_text("from last run")
    tally = auto_promoter.auto_promote(drafts)
    assert tally["skipped_marker"] == 1
    assert tally["promoted"] == 0


def test_tool_call_leakage_rejected(monkeypatch, tmp_path):
    """Raw tool-call content (LLM didn't actually extract) must NOT auto-promote."""
    drafts, root, index = _setup(monkeypatch, tmp_path)
    noise_body = (
        "I'll read the files first.\n"
        "[TOOL_CALL]\n"
        "{tool => \"ReadFiles\", args => {\n"
        "  --paths [\"/foo\", \"/bar\"]\n"
        "}}\n"
        "[/TOOL_CALL]\n"
        "Then I'll proceed with the task.\n" + ("filler " * 40)
    )
    draft = _make_draft(drafts, "bad_draft", "user", noise_body)
    tally = auto_promoter.auto_promote(drafts)
    assert tally.get("rejected_unstructured", 0) == 1
    assert tally["promoted"] == 0
    assert not draft.exists()  # removed
    assert draft.with_suffix(".rejected").exists()
    assert not (root / "user_bad_draft.md").exists()


def test_structured_memory_gate_requires_headers():
    """_is_structured_memory should accept section headers and reject raw prose."""
    assert auto_promoter._is_structured_memory("## Decisions\n- Something")
    assert auto_promoter._is_structured_memory("**Why:** context was X\n**How to apply:** do Y")
    assert not auto_promoter._is_structured_memory("just a wall of text with no sections")
    assert not auto_promoter._is_structured_memory("[TOOL_CALL]\nfoo\n[/TOOL_CALL]\n## Decisions")


def test_duplicate_index_line_not_repeated(monkeypatch, tmp_path):
    drafts, root, index = _setup(monkeypatch, tmp_path)
    _make_draft(drafts, "test", "user", _structured_body(), desc="test desc")
    auto_promoter.auto_promote(drafts)
    # Now create another draft that would produce the same dest filename
    _make_draft(drafts, "test", "user", _structured_body(), desc="duplicate")
    auto_promoter.auto_promote(drafts)

    # Index line for user_test.md should appear exactly once
    count = index.read_text().count("(user_test.md)")
    assert count == 1
