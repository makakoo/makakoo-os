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


def _feed_inputs_with_interrupt(monkeypatch, answers: list[object]):
    """Patch input; use KeyboardInterrupt / EOFError classes to simulate aborts."""
    it = iter(answers)

    def fake_input(prompt: str = "") -> str:
        print(prompt, end="")
        value = next(it)
        if value is KeyboardInterrupt:
            raise KeyboardInterrupt
        if value is EOFError:
            raise EOFError
        return str(value)

    monkeypatch.setattr("builtins.input", fake_input)


def _obsidian_available(monkeypatch):
    monkeypatch.setattr(picker, "_detect_obsidian_app", lambda: (True, "test obsidian"))


def _obsidian_missing(monkeypatch):
    monkeypatch.setattr(picker, "_detect_obsidian_app", lambda: (False, "not detected"))
    monkeypatch.setattr(picker, "_obsidian_install_command", lambda: None)


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



def test_obsidian_app_missing_warns_and_can_skip(tmp_home, monkeypatch, capsys):
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


def test_final_confirmation_keyboard_interrupt_writes_no_pending_source(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    _feed_inputs_with_interrupt(monkeypatch, [
        "y",            # add obsidian
        str(vault),     # vault path
        "n",            # do not allow writes
        "n",            # no plain folder
        "",             # keep current default
        KeyboardInterrupt,
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]
    out = capsys.readouterr().out
    assert "Aborted. No changes written." in out


def test_obsidian_path_keyboard_interrupt_does_not_accept_default_guess(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "guessed_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    monkeypatch.setattr(picker, "_guess_obsidian_vault", lambda: str(vault))
    _feed_inputs_with_interrupt(monkeypatch, [
        "y",              # add obsidian
        KeyboardInterrupt,
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]


def test_obsidian_app_missing_offers_install(tmp_home, monkeypatch, capsys):
    state = {"installed": False}

    def fake_detect():
        if state["installed"]:
            return True, "test installed obsidian"
        return False, "not detected"

    monkeypatch.setattr(picker, "_detect_obsidian_app", fake_detect)
    monkeypatch.setattr(picker, "_obsidian_install_command", lambda: ["brew", "install", "--cask", "obsidian"])

    def fake_install():
        state["installed"] = True
        return True

    monkeypatch.setattr(picker, "_install_obsidian_app", fake_install)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    (vault / "note.md").write_text("# hi")
    _feed_inputs(monkeypatch, [
        "y",  # add obsidian
        "y",  # install app
        str(vault),
        "n",  # do not allow writes
        "n",  # no plain folder
        "",   # keep current default
        "y",  # confirm
    ])

    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert "obsidian" in registry.names()
    out = capsys.readouterr().out
    assert "Install Obsidian now?" in out
    assert "Obsidian app detected: test installed obsidian" in out


def test_obsidian_install_keyboard_interrupt_aborts_without_write(tmp_home, monkeypatch, capsys):
    monkeypatch.setattr(picker, "_detect_obsidian_app", lambda: (False, "not detected"))
    monkeypatch.setattr(picker, "_obsidian_install_command", lambda: ["brew", "install", "--cask", "obsidian"])

    def fake_run(*args, **kwargs):
        raise KeyboardInterrupt

    monkeypatch.setattr(picker.subprocess, "run", fake_run)
    _feed_inputs(monkeypatch, [
        "y",  # add obsidian
        "y",  # try install app
    ])
    rc = picker.run_interactive()
    assert rc == 0
    registry = cfg.load_registry()
    assert registry.names() == ["default"]
    out = capsys.readouterr().out
    assert "Obsidian install cancelled." in out
    assert "Aborted. No changes written." in out


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
        "n",            # do not allow writes
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


def test_commit_failure_returns_nonzero(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    _feed_inputs(monkeypatch, [
        "y",            # add obsidian
        str(vault),     # vault path
        "n",            # do not allow writes
        "n",            # no plain folder
        "",             # keep current default
        "y",            # confirm
    ])

    def fail_apply_changes(adds, default_name=None):
        raise RuntimeError("disk full")

    monkeypatch.setattr(cfg, "apply_changes", fail_apply_changes)
    rc = picker.run_interactive()
    assert rc == 1
    err = capsys.readouterr().err
    assert "Brain setup finished with errors" in err
    assert "disk full" in err


def test_confirm_writes_all_pending_adds(tmp_home, monkeypatch, capsys):
    _obsidian_available(monkeypatch)
    vault = tmp_home / "my_vault"
    vault.mkdir()
    (vault / ".obsidian").mkdir()
    (vault / "note.md").write_text("# hi")
    _feed_inputs(monkeypatch, [
        "y",            # add obsidian
        str(vault),     # vault path
        "n",            # do not allow writes
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
        "y",              # allow writes so it can become default
        "n",              # no plain
        "obsidian",       # change default to obsidian
        "y",              # confirm
    ])
    picker.run_interactive()
    registry = cfg.load_registry()
    assert registry.default_name == "obsidian"
