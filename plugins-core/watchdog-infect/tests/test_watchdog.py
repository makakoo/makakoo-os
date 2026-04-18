"""Unit tests for watchdog-infect.

Mocks `makakoo` subprocess calls via monkey-patching `run_makakoo` so
tests never spawn a real binary. Each test verifies the exit code +
journal side effect (or lack thereof) for one path through `tick`.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))

import watchdog  # noqa: E402


def _cp(stdout: str = "", returncode: int = 0, stderr: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(
        args=[], returncode=returncode, stdout=stdout, stderr=stderr
    )


def _clean_payload() -> str:
    return json.dumps({
        "clean": True,
        "dirty_count": 0,
        "targets": [
            {"name": "claude", "clean": True, "issues": []},
            {"name": "cursor", "clean": True, "issues": []},
        ],
    })


def _dirty_payload(issues: dict[str, list[str]]) -> str:
    targets = [
        {
            "name": name,
            "clean": not iss,
            "issues": iss,
        }
        for name, iss in issues.items()
    ]
    return json.dumps({
        "clean": all(t["clean"] for t in targets),
        "dirty_count": sum(1 for t in targets if not t["clean"]),
        "targets": targets,
    })


@pytest.fixture
def fake_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.setenv("HARVEY_HOME", str(tmp_path))
    return tmp_path


@pytest.fixture
def journal_after(fake_home):
    def _read() -> str:
        p = watchdog.journal_path()
        return p.read_text() if p.exists() else ""
    return _read


# --- Happy path ------------------------------------------------------


def test_clean_tick_writes_nothing(fake_home, journal_after, monkeypatch):
    calls = []

    def fake_run(_mk, *args):
        calls.append(args)
        return _cp(_clean_payload(), 0)

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 0
    assert journal_after() == ""
    # Only one call: the initial verify. No heal, no re-verify.
    assert len(calls) == 1
    assert calls[0] == ("infect", "--verify", "--json")


def test_drift_triggers_heal_and_journal(fake_home, journal_after, monkeypatch):
    sequence = [
        _cp(_dirty_payload({"cursor": ["mcp-stale-command"]}), 1),  # verify-dirty
        _cp("infected 7 targets\n", 0),                              # global-heal
        _cp(_clean_payload(), 0),                                    # post-verify clean
    ]

    def fake_run(_mk, *_args):
        return sequence.pop(0)

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 0

    entry = journal_after()
    assert "[[Makakoo Watchdog]] caught infect drift" in entry
    assert "cursor (mcp-stale-command)" in entry
    assert "Post-heal verify: clean" in entry


def test_heal_retry_absorbs_transient_failure(fake_home, journal_after, monkeypatch):
    sequence = [
        _cp(_dirty_payload({"vibe": ["memory-symlink-broken"]}), 1),  # initial dirty
        _cp("infected\n", 0),                                          # heal
        _cp(_dirty_payload({"vibe": ["memory-symlink-broken"]}), 1),  # post-verify still dirty (flake)
        _cp(_clean_payload(), 0),                                      # retry clean
    ]

    def fake_run(_mk, *_args):
        return sequence.pop(0)

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 0, "retry should have confirmed heal"
    assert "Post-heal verify: clean" in journal_after()


def test_heal_fails_nonzero_when_still_dirty(fake_home, journal_after, monkeypatch):
    dirty = _cp(_dirty_payload({"cursor": ["mcp-stale-command"]}), 1)
    sequence = [
        dirty,                # initial verify
        _cp("tried\n", 0),    # global-heal
        dirty,                # post-verify still dirty
        dirty,                # retry still dirty
    ]

    def fake_run(_mk, *_args):
        return sequence.pop(0)

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 1, "still-dirty after heal+retry should exit 1"
    assert "Post-heal verify: STILL DIRTY" in journal_after()


# --- Escalation + safety ---------------------------------------------


def test_max_repairs_triggers_critical_flag(fake_home, journal_after, monkeypatch):
    # 4 distinct drift issues across two targets (> MAX_DRIFT_FOR_CRITICAL=3)
    sequence = [
        _cp(_dirty_payload({
            "cursor": ["mcp-stale-command", "mcp-stale-env"],
            "vibe": ["memory-symlink-broken", "recursive-symlink-in-memory"],
        }), 1),
        _cp("heal\n", 0),
        _cp(_clean_payload(), 0),
    ]

    def fake_run(_mk, *_args):
        return sequence.pop(0)

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 0
    entry = journal_after()
    assert "⚠️ critical drift conflict" in entry


def test_json_parse_error_fails_loud(fake_home, journal_after, monkeypatch):
    def fake_run(_mk, *_args):
        return _cp("this is not json", 0)

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 2
    assert "parse error" in journal_after()


def test_stale_binary_audit_fails_cleanly(fake_home, journal_after, monkeypatch):
    # Regression: dogfood 2026-04-18 hit a stale makakoo on PATH that
    # rejected `--json`, wrote to stderr, exited 2. Empty stdout + bad
    # exit must report "audit failed / exit N" — not the misleading
    # "parse error" which implies the flag is broken, not missing.
    def fake_run(_mk, *_args):
        return _cp(
            stdout="",
            stderr="error: unexpected argument '--json' found\n",
            returncode=2,
        )

    monkeypatch.setattr(watchdog, "run_makakoo", fake_run)
    code = watchdog.tick("/fake/makakoo")
    assert code == 2
    entry = journal_after()
    assert "audit failed" in entry
    assert "exit 2" in entry
    assert "--json" in entry
    assert "parse error" not in entry


def test_makakoo_binary_missing_fails_loud(fake_home, journal_after, monkeypatch):
    monkeypatch.setattr(watchdog, "resolve_makakoo", lambda: None)
    # `main` is the layer that calls resolve_makakoo — invoke it directly.
    # argparse.parse_args() reads sys.argv, so clear it.
    monkeypatch.setattr(sys, "argv", ["watchdog.py"])
    code = watchdog.main()
    assert code == 2
    assert "binary not found" in journal_after()


# --- Supporting unit tests -------------------------------------------


def test_summarise_drift_counts_issues_and_targets():
    payload = json.loads(_dirty_payload({
        "cursor": ["mcp-stale-command"],
        "vibe": ["memory-symlink-broken", "recursive-symlink-in-memory"],
        "claude": [],
    }))
    count, desc = watchdog.summarise_drift(payload)
    assert count == 3  # 1 + 2
    assert len(desc) == 2  # claude is clean, not in desc
    assert "cursor (mcp-stale-command)" in desc
    assert any("vibe" in d and "memory-symlink-broken" in d for d in desc)
