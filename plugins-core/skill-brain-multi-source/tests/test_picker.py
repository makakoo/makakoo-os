"""Tests for the interactive picker (batched + confirm behavior)."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

import config as cfg  # noqa: E402
import picker  # noqa: E402


@pytest.fixture
def tmp_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("HARVEY_HOME", raising=False)
    (tmp_path / "data" / "Brain" / "pages").mkdir(parents=True)
    (tmp_path / "data" / "Brain" / "pages" / "seed.md").write_text("# seed")
    return tmp_path


def _feed_inputs(monkeypatch, answers: list[str]):
    """Patch builtins.input to return successive canned answers."""
    it = iter(answers)
    def fake_input(prompt: str = "") -> str:
        print(prompt, end="")
        return next(it)

    monkeypatch.setattr("builtins.input", fake_input)


def _obsidian_available(monkeypatch):
    monkeypatch.setattr(picker, "_detect_obsidian_app", lambda: (True, "test obsidian"))


def _obsidian_missing(monkeypatch):
    monkeypatch.setattr(picker, "_detect_obsidian_app", lambda: (False, "not detected"))


def test_non_interactive_mode_seeds_default_only(tmp_home, capsys):
    rc = picker.run_interactive(non_interactive=True)
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]


def test_decline_all_prompts_leaves_only_default(tmp_home, monkeypatch, capsys):
    _feed_inputs(monkeypatch, ["n", "n"])  # no obsidian, no plain folder
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]



def test_obsidian_app_missing_warns_and_skips_by_default(tmp_home, monkeypatch, capsys):
    _obsidian_missing(monkeypatch)
    _feed_inputs(monkeypatch, [
        "y",  # asks to add Obsidian
        "n",  # do not register a vault path anyway
        "n",  # no plain folder
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]
    out = capsys.readouterr().out
    assert "Obsidian app was not detected" in out
    assert "Register an existing Obsidian vault path anyway?" in out


def test_obsidian_path_no_is_treated_as_skip(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    _feed_inputs(monkeypatch, [
        "y",  # add Obsidian
        "n",  # path prompt: no means skip, not a literal path named n
        "n",  # no plain folder
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]
    out = capsys.readouterr().out
    assert "Skipping Obsidian source registration" in out
    assert "register 'obsidian'" not in out


def test_missing_obsidian_path_requires_confirmation(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    missing = tmp_home / "missing_vault"
    _feed_inputs(monkeypatch, [
        "y",              # add Obsidian
        str(missing),     # vault path does not exist
        "n",              # do not register missing path
        "n",              # no plain folder
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]
    out = capsys.readouterr().out
    assert "doesn't exist" in out
    assert "Register this missing path anyway?" in out

def test_abort_at_final_confirmation_writes_nothing(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    (vault / "note.md").write_text("# hi")
    _feed_inputs(monkeypatch, [
        "y",            # add obsidian
        str(vault),     # vault path
        "n",            # no plain folder
        "",             # keep current default
        "n",            # abort at final confirmation
    ])
    rc = picker.run_interactive()
    assert rc == 0
    # Config has ONLY the baseline default — obsidian was NOT persisted
    registry = cfg.load_registry()
    assert "obsidian" not in registry.names()
    assert registry.names() == ["default"]
    out = capsys.readouterr().out
    assert "Aborted" in out


def test_confirm_writes_all_pending_adds(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    (vault / "note.md").write_text("# hi")
    _feed_inputs(monkeypatch, [
        "y",            # add obsidian
        str(vault),     # vault path
        "n",            # no plain folder
        "",             # blank = keep current default
        "y",            # confirm
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert "default" in registry.names()
    assert "obsidian" in registry.names()


def test_summary_shown_before_write(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    _feed_inputs(monkeypatch, [
        "y",
        str(vault),
        "n",
        "",
        "y",
    ])
    picker.run_interactive()
    out = capsys.readouterr().out
    assert "Pending changes" in out
    assert "+ register" in out


def test_post_write_sync_reports_counts(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    (vault / "a.md").write_text("# a")
    (vault / "b.md").write_text("# b")
    _feed_inputs(monkeypatch, [
        "y",
        str(vault),
        "n",
        "",
        "y",
    ])
    picker.run_interactive()
    out = capsys.readouterr().out
    assert "Initial sync" in out
    assert "obsidian:" in out


def test_change_default_via_picker(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    _feed_inputs(monkeypatch, [
        "y",              # add obsidian
        str(vault),
        "n",              # no plain
        "obsidian",       # change default to obsidian
        "y",              # confirm
    ])
    picker.run_interactive()
    registry = cfg.load_registry()
    assert registry.default_name == "obsidian"
