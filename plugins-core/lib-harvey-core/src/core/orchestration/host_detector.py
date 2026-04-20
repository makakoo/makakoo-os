"""
host_detector — Phase 1 of harvey:infect.

Detects which LLM CLI is currently invoking Harvey, returns a structured
HostInfo with name, version, capabilities, and a confidence score. Used by
mcp_registrar (Phase 3) to pick the right registration mechanism, and by
context_shadow (Phase 2) to record which host the shadow file was generated
for.

Detection signal sources, in order of priority:

  1. Environment variables — most reliable; the host CLI sets these when
     it spawns the agent loop. Examples: CLAUDECODE=1, OPENCODE_*,
     CRUSH_DATA_DIR. GEMINI_API_KEY is intentionally weak (users set it
     independently of the gemini CLI).
  2. Parent process inspection — `ps -p $PPID -o comm=` returns the parent
     command name; useful when env vars are absent or stripped.
  3. Capability probe (`<binary> --version`) — last resort, costly because
     it spawns subprocesses for every candidate. Only used to fill in the
     `version` field of an already-detected host, never to discover one.

Confidence is the maximum signal weight matched, combined via noisy-OR
when multiple independent signals (env + ppid) match the same host. A
single strong signal gives full confidence; a weak signal alone gives
partial and may push detection into the uncertain band.

Returns HostInfo{name, version, capabilities, confidence, signals_matched}.
Never raises from detect_host(). Use detect_host_strict() for the
HostUncertain semantics.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass, field
from enum import Enum
from typing import List, Optional


# ─── Data ────────────────────────────────────────────────────────


class HostType(Enum):
    CLAUDE_CODE = "claude-code"
    OPENCODE = "opencode"
    CRUSH = "crush"
    GEMINI_CLI = "gemini-cli"
    CODEX = "codex"
    VIBE = "mistral-vibe"  # v3: Mistral Vibe CLI
    CURSOR = "cursor"      # v3: Cursor
    QWEN = "qwen-code"     # v7: Qwen Code (Gemini-CLI fork, uses ~/.qwen/QWEN.md)
    PI = "pi"              # v9: pi coding agent (pi.dev)
    DYNAMIC = "dynamic"    # v8: runtime-registered via `harvey onboard <name>`
                            # — actual host name lives in GlobalSlot.display_name
    UNKNOWN = "unknown"


@dataclass
class HostInfo:
    name: HostType
    version: str = ""
    capabilities: List[str] = field(default_factory=list)
    confidence: float = 0.0
    signals_matched: List[str] = field(default_factory=list)
    legacy_alias: str = ""  # e.g. "opencode" for crush

    def is_known(self) -> bool:
        return self.name != HostType.UNKNOWN and self.confidence > 0.0


class HostUncertain(Exception):
    """Raised by detect_host_strict() when confidence is in [0.5, 0.8].

    Caller resolves by either prompting the user or setting CONFIRM_HOST=1
    env var to accept the suggestion.
    """

    def __init__(self, host_info: HostInfo, suggestion: str):
        self.host_info = host_info
        self.suggestion = suggestion
        super().__init__(
            f"host detection ambiguous (confidence={host_info.confidence:.2f}); "
            f"suggested: {suggestion}. Set CONFIRM_HOST=1 to accept."
        )


# ─── Signal definitions ─────────────────────────────────────────
#
# Each entry: (host_type, signal_kind, signal_value, weight)
#
# signal_kind: "env"  — env var name; matches if value is non-empty
#              "ppid" — substring match against parent process command
#
# weight: 0.0–1.0; the maximum matched weight is the per-host base score,
# combined via noisy-OR with other matched signals from the same host.
#
# Strong signals (≥0.9) are env vars the host sets exclusively. Weak
# signals (~0.4) are generic API key vars users set themselves.

_SIGNALS = [
    # Claude Code — verified against live env: CLAUDECODE=1 set in every session
    (HostType.CLAUDE_CODE, "env", "CLAUDECODE", 0.95),
    (HostType.CLAUDE_CODE, "env", "CLAUDE_CODE_ENTRYPOINT", 0.90),
    (HostType.CLAUDE_CODE, "env", "CLAUDE_CODE_EXECPATH", 0.90),
    (HostType.CLAUDE_CODE, "ppid", "claude", 0.85),

    # opencode v1.x — sets OPENCODE_* during runs
    (HostType.OPENCODE, "env", "OPENCODE_API_KEY", 0.90),
    (HostType.OPENCODE, "env", "OPENCODE_BASE_URL", 0.85),
    (HostType.OPENCODE, "env", "OPENCODE_MODEL", 0.85),
    (HostType.OPENCODE, "ppid", "opencode", 0.85),

    # Crush — opencode's successor (Charm)
    (HostType.CRUSH, "env", "CRUSH_DATA_DIR", 0.95),
    (HostType.CRUSH, "ppid", "crush", 0.85),

    # Gemini CLI
    (HostType.GEMINI_CLI, "env", "GEMINI_CLI_SESSION", 0.90),
    (HostType.GEMINI_CLI, "ppid", "gemini", 0.85),
    # GEMINI_API_KEY alone is weak — users set it independently
    (HostType.GEMINI_CLI, "env", "GEMINI_API_KEY", 0.40),

    # Codex
    (HostType.CODEX, "env", "CODEX_SESSION_ID", 0.95),
    (HostType.CODEX, "ppid", "codex", 0.85),

    # Qwen Code — Gemini CLI fork published by QwenLM at
    # github.com/QwenLM/qwen-code. Onboarded 2026-04-14 as the 7th
    # CLI host. Detection is by ppid substring since Qwen doesn't
    # set a unique env var yet.
    (HostType.QWEN, "ppid", "qwen", 0.85),

    # pi coding agent — pi.dev, sets PI_CODING_AGENT=true in env
    (HostType.PI, "env", "PI_CODING_AGENT", 0.95),
    (HostType.PI, "ppid", "pi", 0.85),
]


# ─── Public API ─────────────────────────────────────────────────


def detect_host(
    env: Optional[dict] = None,
    ppid_cmd: Optional[str] = None,
) -> HostInfo:
    """Detect which CLI is running. Never raises.

    Test injection points:
        env: dict of env vars (defaults to os.environ)
        ppid_cmd: parent process command (defaults to ps -p $PPID)

    Returns HostInfo. Caller decides whether to treat low confidence as
    unknown or prompt for confirmation.
    """
    env = env if env is not None else os.environ
    if ppid_cmd is None:
        ppid_cmd = _read_ppid_cmd()

    # Per-host: collect matched signals + their weights
    matched_weights: dict[HostType, list[float]] = {}
    matched_names: dict[HostType, list[str]] = {}

    for host, kind, value, weight in _SIGNALS:
        hit = False
        if kind == "env" and env.get(value):
            hit = True
        elif kind == "ppid" and ppid_cmd and value in ppid_cmd.lower():
            hit = True
        if hit:
            matched_weights.setdefault(host, []).append(weight)
            matched_names.setdefault(host, []).append(f"{kind}:{value}")

    if not matched_weights:
        return HostInfo(name=HostType.UNKNOWN, confidence=0.0)

    # Combine via noisy-OR per host: 1 - prod(1 - w_i)
    scores: dict[HostType, float] = {}
    for host, weights in matched_weights.items():
        combined = 1.0
        for w in weights:
            combined *= (1.0 - w)
        scores[host] = round(min(0.99, 1.0 - combined), 4)

    # Pick the highest-scoring host
    best_host = max(scores.items(), key=lambda x: x[1])[0]
    best_score = scores[best_host]

    # Crush vs opencode disambiguation: prefer crush only if the binary is
    # actually installed (else opencode env vars likely came from opencode v1.x).
    if best_host == HostType.CRUSH and HostType.OPENCODE in scores:
        if not shutil.which("crush"):
            best_host = HostType.OPENCODE
            best_score = scores[HostType.OPENCODE]

    info = HostInfo(
        name=best_host,
        confidence=best_score,
        signals_matched=matched_names.get(best_host, []),
        capabilities=_capabilities_for(best_host),
    )

    # Optional version probe — best-effort, doesn't affect confidence
    info.version = _probe_version(best_host)

    if best_host == HostType.CRUSH:
        info.legacy_alias = "opencode"

    return info


def detect_host_strict(
    env: Optional[dict] = None,
    ppid_cmd: Optional[str] = None,
) -> HostInfo:
    """Strict variant: raises HostUncertain when confidence is in [0.5, 0.8].

    Below 0.5 returns an UNKNOWN HostInfo without raising. The CLI uses
    this variant; library callers may use detect_host() directly.
    """
    env = env if env is not None else os.environ
    info = detect_host(env=env, ppid_cmd=ppid_cmd)
    confirm_env = env.get("CONFIRM_HOST", "").strip()

    if info.confidence < 0.5:
        return HostInfo(name=HostType.UNKNOWN, confidence=0.0)
    if 0.5 <= info.confidence < 0.8:
        if confirm_env:
            info.confidence = max(info.confidence, 0.8)
            info.signals_matched.append("env:CONFIRM_HOST")
            return info
        raise HostUncertain(info, suggestion=info.name.value)
    return info


# ─── Internal helpers ───────────────────────────────────────────


def _read_ppid_cmd() -> str:
    """Parent process command name via portable ps. Empty on failure."""
    try:
        ppid = os.getppid()
        result = subprocess.run(
            ["ps", "-p", str(ppid), "-o", "comm="],
            capture_output=True,
            text=True,
            timeout=2,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        pass
    return ""


def _probe_version(host: HostType) -> str:
    """Best-effort `<binary> --version`. Empty string on failure."""
    binary_map = {
        HostType.CLAUDE_CODE: "claude",
        HostType.OPENCODE: "opencode",
        HostType.CRUSH: "crush",
        HostType.GEMINI_CLI: "gemini",
        HostType.CODEX: "codex",
    }
    binary = binary_map.get(host)
    if not binary or not shutil.which(binary):
        return ""
    try:
        result = subprocess.run(
            [binary, "--version"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0:
            return result.stdout.strip().split("\n")[0]
    except (OSError, subprocess.TimeoutExpired):
        pass
    return ""


def _capabilities_for(host: HostType) -> List[str]:
    """Static capability table per host. Used by mcp_registrar to pick the
    right registration path. Verified against v2.1 sprint capability matrix."""
    table = {
        HostType.CLAUDE_CODE: ["mcp", "agents-md", "settings-json"],
        HostType.OPENCODE: ["mcp", "config-json"],
        HostType.CRUSH: ["mcp", "crush-json"],  # uses 'mcp' key, not 'mcpServers'
        HostType.GEMINI_CLI: ["mcp", "settings-json"],  # MCP support added in 0.30+
        HostType.CODEX: ["agents-md"],  # no MCP support today
    }
    return list(table.get(host, []))
