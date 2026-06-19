"""Phase 1 (F1, F2) command-execution security regression tests.

F1 — ``harvey_agent.tool_run_command`` must never reach a shell and must
     reject shell-injection vectors while still allowing the read-only
     allowlist. The originally-confirmed bypass was
     ``curl http://localhost:18080/health$(whoami)``.

F2 — the two multimodal ingest modules must invoke ``ffprobe`` with a fixed
     argv (no ``shell=True``, no f-string-interpolated path) and must not
     write chunk files to predictable ``/tmp`` paths. Those modules pull
     heavy optional deps (numpy/genai/qdrant), so we assert the property
     statically against their source rather than importing them.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

_HERE = Path(__file__).resolve()
_SRC = next(
    p for p in _HERE.parents if (p / "core" / "agent" / "harvey_agent.py").exists()
)
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from core.agent.harvey_agent import (  # noqa: E402
    _is_safe_command,
    _validate_command,
    tool_run_command,
)


class ValidateCommandRejects(unittest.TestCase):
    """F1: injection vectors and write/destructive commands are rejected."""

    REJECTED = [
        # the originally-confirmed bypass
        "curl http://localhost:18080/health$(whoami)",
        # command substitution / backticks / param expansion on an allowed binary
        "ls $(whoami)",
        "ls `whoami`",
        "df ${HOME}",
        # shell control operators
        "ls; rm -rf /",
        "ls && rm x",
        "ls | grep secret",
        "uname > /tmp/x",
        "ls\nrm x",
        # globbing
        "ls *",
        "ls ?.txt",
        # arbitrary network — curl is no longer an allowed prefix
        "curl http://evil.example/x",
        "curl -s http://169.254.169.254/latest/meta-data",
        # binaries that are not on the allowlist
        "cat /etc/passwd",
        "echo hi",
        "bash -c id",
        # write / destructive git subcommands
        "git push",
        "git commit -m x",
        "git add .",
        # help/version banners (pager side effects)
        "git --help",
        "ls --help",
        # quoting tricks
        "ls '/etc'",
        'ls "/etc"',
        # empty / overlong
        "",
        "ls " + "a" * 600,
    ]

    def test_rejected(self):
        for cmd in self.REJECTED:
            with self.subTest(cmd=cmd):
                self.assertIsNone(_validate_command(cmd))
                self.assertFalse(_is_safe_command(cmd))


class ValidateCommandAllows(unittest.TestCase):
    """F1: the legitimate read-only allowlist still passes, as a fixed argv."""

    ALLOWED = {
        "ls": ["ls"],
        "ls -la": ["ls", "-la"],
        "ps aux": ["ps", "aux"],
        "uname": ["uname"],
        "df -h": ["df", "-h"],
        "uptime": ["uptime"],
        "git status": ["git", "status"],
        "git log --oneline -5": ["git", "log", "--oneline", "-5"],
        "git branch": ["git", "branch"],
        "git remote -v": ["git", "remote", "-v"],
        "launchctl list": ["launchctl", "list"],
        "crontab -l": ["crontab", "-l"],
        "curl -s http://localhost:18080/health": [
            "curl",
            "-s",
            "http://localhost:18080/health",
        ],
        "curl -s http://localhost:12315/health": [
            "curl",
            "-s",
            "http://localhost:12315/health",
        ],
    }

    def test_allowed(self):
        for cmd, argv in self.ALLOWED.items():
            with self.subTest(cmd=cmd):
                self.assertEqual(_validate_command(cmd), argv)
                self.assertTrue(_is_safe_command(cmd))


class ToolRunCommandBehaviour(unittest.TestCase):
    """F1: end-to-end — rejection message + shell-free execution."""

    def test_rejection_message(self):
        out = tool_run_command("curl http://localhost:18080/health$(whoami)")
        self.assertIn("rejected", out.lower())

    def test_allowed_executes(self):
        # `uname` exists on every CI platform we target (macOS/Linux).
        out = tool_run_command("uname")
        self.assertTrue(out.strip())
        self.assertNotIn("rejected", out.lower())

    def test_injection_rejected_not_executed(self):
        # A metacharacter-bearing command against an allowed binary must be
        # rejected outright, never handed to a shell. (The rejection message
        # echoes the input, so we assert on the verdict, not output tokens.)
        for payload in ("hostname $(id)", "ls `id`", "df ${HOME}/x", "uname; id"):
            out = tool_run_command(payload)
            self.assertIn("rejected", out.lower(), payload)


class IngestNoShellInjection(unittest.TestCase):
    """F2: multimodal ingest must not shell out for ffprobe or use /tmp chunks."""

    _REPO = next(p for p in _HERE.parents if (p / "plugins-core").is_dir())
    _FILES = [
        _REPO / "plugins-core/agent-multimodal-knowledge/src/ingest.py",
        _REPO / "plugins-core/skill-ai-ml-multimodal-knowledge/src/ingest.py",
    ]

    def test_files_exist(self):
        for f in self._FILES:
            self.assertTrue(f.is_file(), f)

    def test_no_shell_true(self):
        for f in self._FILES:
            self.assertNotIn("shell=True", f.read_text(), f)

    def test_ffprobe_is_argv(self):
        # ffprobe must be invoked as a fixed argv list, not a shell string.
        # (shell=True absence, asserted above, is what actually kills the
        # injection; this confirms the call was rebuilt correctly.)
        for f in self._FILES:
            src = f.read_text()
            self.assertIn('["ffprobe", "-v", "quiet"', src, f)

    def test_no_predictable_tmp_chunks(self):
        for f in self._FILES:
            src = f.read_text()
            for bad in ("/tmp/audio_chunk_", "/tmp/video_chunk_", "/tmp/pdf_chunk_"):
                self.assertNotIn(bad, src, f"{bad} still present in {f}")


if __name__ == "__main__":
    unittest.main()
