"""Dangerous command detection and approval system for Harvey OS.

This module provides pattern-based detection of dangerous shell commands,
with NFKC Unicode normalization to prevent obfuscation bypass, and
smart LLM-powered approval for low-risk false positives.

Adapted from hermes-agent/tools/approval.py (670 lines).
"""

import logging
import os
import re
import threading
import unicodedata
from typing import Optional

logger = logging.getLogger(__name__)

# =========================================================================
# ANSI Escape Sequence Stripping (ECMA-48)
# =========================================================================

ANSI_ESCAPE_PATTERN = re.compile(
    r"""
    \x1b\[           # CSI (Control Sequence Introducer)
    [\x30-\x3f]*     # Parameter bytes
    [\x20-\x2f]*     # Intermediate bytes  
    [\x40-\x7e]      # Final byte
    |
    \x1b\]           # OSC (Operating System Command)
    [\x00-\x07]*     # Parameter bytes
    \x1b\\           # ST (String Terminator)
    |
    \x1b[P-Z\[-\_]   # Single-byte sequences
    |
    \x9b             # CSI (8-bit, converted from 7-bit)
    """,
    re.VERBOSE,
)


def strip_ansi(command: str) -> str:
    """Strip all ANSI escape sequences from command string.

    Removes: CSI sequences, OSC, DCS, ECMA-48 8-bit C1 controls,
    and single-byte escape sequences.
    """
    return ANSI_ESCAPE_PATTERN.sub("", command)


# =========================================================================
# Sensitive path patterns
# =========================================================================

_SSH_SENSITIVE_PATH = r"(?:~|\$home|\$\{home\})/\.ssh(?:/|$)"
_HERMES_ENV_PATH = (
    r"(?:~\/\.hermes/|"
    r"(?:\$home|\$\{home\})/\.hermes/|"
    r"(?:\$hermes_home|\$\{hermes_home\})/)"
    r"\.env\b"
)
_SENSITIVE_WRITE_TARGET = (
    r"(?:/etc/|/dev/sd|"
    rf"{_SSH_SENSITIVE_PATH}|"
    rf"{_HERMES_ENV_PATH})"
)


# =========================================================================
# Dangerous command patterns (40+ patterns)
# =========================================================================

DANGEROUS_PATTERNS = [
    # Recursive delete in root or dangerous paths
    (r"\brm\s+(-[^\s]*\s+)*/", "delete in root path"),
    (r"\brm\s+-[^\s]*r", "recursive delete"),
    (r"\brm\s+--recursive\b", "recursive delete (long flag)"),
    # World-writable permissions
    (
        r"\bchmod\s+(-[^\s]*\s+)*(777|666|o\+[rwx]*w|a\+[rwx]*w)\b",
        "world/other-writable permissions",
    ),
    (
        r"\bchmod\s+--recursive\b.*(777|666|o\+[rwx]*w|a\+[rwx]*w)",
        "recursive world/other-writable (long flag)",
    ),
    # Recursive chown to root
    (r"\bchown\s+(-[^\s]*)?R\s+root", "recursive chown to root"),
    (r"\bchown\s+--recursive\b.*root", "recursive chown to root (long flag)"),
    # Filesystem destruction
    (r"\bmkfs\b", "format filesystem"),
    (r"\bdd\s+.*if=", "disk copy"),
    (r">\s*/dev/sd", "write to block device"),
    (r"\bparted\b", "partition manipulation"),
    (r"\bfdisk\b", "partition manipulation"),
    # SQL destructive commands
    (r"\bDROP\s+(TABLE|DATABASE)\b", "SQL DROP"),
    (r"\bDELETE\s+FROM\b(?!.*\bWHERE\b)", "SQL DELETE without WHERE"),
    (r"\bTRUNCATE\s+(TABLE)?\s*\w", "SQL TRUNCATE"),
    (r"\bALTER\s+.*\s+DROP\b", "SQL ALTER DROP"),
    # System file overwrites
    (r">\s*/etc/", "overwrite system config"),
    (r"\b(cp|mv|install)\b.*\s/etc/", "copy/move file into /etc/"),
    (r"\bsed\s+-[^\s]*i.*\s/etc/", "in-place edit of system config"),
    (r"\bsed\s+--in-place\b.*\s/etc/", "in-place edit of system config (long flag)"),
    (rf'\btee\b.*["\']?{_SENSITIVE_WRITE_TARGET}', "overwrite system file via tee"),
    (
        rf'>>?\s*["\']?{_SENSITIVE_WRITE_TARGET}',
        "overwrite system file via redirection",
    ),
    # Service manipulation
    (r"\bsystemctl\s+(stop|disable|mask)\b", "stop/disable system service"),
    # Process killing
    (r"\bkill\s+-9\s+-1\b", "kill all processes"),
    (r"\bpkill\s+-9\b", "force kill processes"),
    # Fork bomb
    (r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:", "fork bomb"),
    # Shell invocation via -c / -e flags
    (r"\b(bash|sh|zsh|ksh)\s+-[^\s]*c(\s+|$)", "shell command via -c/-lc flag"),
    (r"\b(python[23]?|perl|ruby|node)\s+-[ec]\s+", "script execution via -e/-c flag"),
    # Pipe to shell
    (r"\b(curl|wget)\b.*\|\s*(ba)?sh\b", "pipe remote content to shell"),
    (
        r"\b(bash|sh|zsh|ksh)\s+<\s*<?\s*\(\s*(curl|wget)\b",
        "execute remote script via process substitution",
    ),
    # xargs with rm
    (r"\bxargs\s+.*\brm\b", "xargs with rm"),
    (r"\bfind\b.*-exec\s+(/\S*/)?rm\b", "find -exec rm"),
    (r"\bfind\b.*-delete\b", "find -delete"),
    # Gateway protection: never start gateway outside systemd management
    (
        r"\bgateway\s+run\b.*(&\s*$|&\s*;|\bdisown\b|\bsetsid\b)",
        "start gateway outside systemd",
    ),
    (r"\bnohup\b.*gateway\s+run\b", "start gateway via nohup"),
    # Self-termination protection: prevent agent from killing its own process
    (
        r"\b(pkill|killall)\b.*\b(hermes|gateway|cli\.py)\b",
        "kill hermes/gateway process (self-termination)",
    ),
    # SSH/authorized_keys backdoor detection
    (r"\bauthorized_keys\b", "SSH authorized_keys manipulation"),
    (r"\.ssh/authorized_keys", "SSH authorized_keys file access"),
    # Network-based file downloads with overwrite
    (r"\b(wget|curl)\b.*-O\s+/", "download to root path"),
    (r"\b(wget|curl)\b.*--output-document=.*/", "download to absolute path"),
]


# =========================================================================
# Pattern key aliases for backwards compatibility
# =========================================================================


def _legacy_pattern_key(pattern: str) -> str:
    """Reproduce the old regex-derived approval key for backwards compatibility."""
    return pattern.split(r"\b")[1] if r"\b" in pattern else pattern[:20]


_PATTERN_KEY_ALIASES: dict[str, set[str]] = {}
for _pattern, _description in DANGEROUS_PATTERNS:
    _legacy_key = _legacy_pattern_key(_pattern)
    _canonical_key = _description
    _PATTERN_KEY_ALIASES.setdefault(_canonical_key, set()).update(
        {_canonical_key, _legacy_key}
    )
    _PATTERN_KEY_ALIASES.setdefault(_legacy_key, set()).update(
        {_legacy_key, _canonical_key}
    )


def _approval_key_aliases(pattern_key: str) -> set[str]:
    """Return all approval keys that should match this pattern."""
    return _PATTERN_KEY_ALIASES.get(pattern_key, {pattern_key})


# =========================================================================
# Detection
# =========================================================================


def _normalize_command_for_detection(command: str) -> str:
    """Normalize a command string before dangerous-pattern matching.

    Strips ANSI escape sequences (full ECMA-48 via strip_ansi),
    null bytes, and normalizes Unicode fullwidth characters so that
    obfuscation techniques cannot bypass the pattern-based detection.
    """
    # Strip all ANSI escape sequences (CSI, OSC, DCS, 8-bit C1, etc.)
    command = strip_ansi(command)
    # Strip null bytes
    command = command.replace("\x00", "")
    # Normalize Unicode (fullwidth Latin, halfwidth Katakana, etc.)
    command = unicodedata.normalize("NFKC", command)
    return command


def detect_dangerous_command(command: str) -> tuple:
    """Check if a command matches any dangerous patterns.

    Returns:
        (is_dangerous, pattern_key, description) or (False, None, None)
    """
    command_lower = _normalize_command_for_detection(command).lower()
    for pattern, description in DANGEROUS_PATTERNS:
        if re.search(pattern, command_lower, re.IGNORECASE | re.DOTALL):
            pattern_key = description
            return (True, pattern_key, description)
    return (False, None, None)


# =========================================================================
# Per-session approval state (thread-safe)
# =========================================================================

_lock = threading.Lock()
_pending: dict[str, dict] = {}
_session_approved: dict[str, set] = {}
_permanent_approved: set = set()


def submit_pending(session_key: str, approval: dict):
    """Store a pending approval request for a session."""
    with _lock:
        _pending[session_key] = approval


def pop_pending(session_key: str) -> Optional[dict]:
    """Retrieve and remove a pending approval for a session."""
    with _lock:
        return _pending.pop(session_key, None)


def has_pending(session_key: str) -> bool:
    """Check if a session has a pending approval request."""
    with _lock:
        return session_key in _pending


def approve_session(session_key: str, pattern_key: str):
    """Approve a pattern for this session only."""
    with _lock:
        _session_approved.setdefault(session_key, set()).add(pattern_key)


def is_approved(session_key: str, pattern_key: str) -> bool:
    """Check if a pattern is approved (session-scoped or permanent).

    Accept both the current canonical key and the legacy regex-derived key so
    existing command_allowlist entries continue to work after key migrations.
    """
    aliases = _approval_key_aliases(pattern_key)
    with _lock:
        if any(alias in _permanent_approved for alias in aliases):
            return True
        session_approvals = _session_approved.get(session_key, set())
        return any(alias in session_approvals for alias in aliases)


def approve_permanent(pattern_key: str):
    """Add a pattern to the permanent allowlist."""
    with _lock:
        _permanent_approved.add(pattern_key)


def load_permanent(patterns: set):
    """Bulk-load permanent allowlist entries from config."""
    with _lock:
        _permanent_approved.update(patterns)


def clear_session(session_key: str):
    """Clear all approvals and pending requests for a session."""
    with _lock:
        _session_approved.pop(session_key, None)
        _pending.pop(session_key, None)


def clear_all_approvals():
    """Clear all session and permanent approvals (for testing)."""
    with _lock:
        _session_approved.clear()
        _permanent_approved.clear()
        _pending.clear()


# =========================================================================
# Smart approval via LLM
# =========================================================================


def _smart_approve(command: str, description: str) -> str:
    """Use the auxiliary LLM to assess risk and decide approval.

    Returns 'approve' if the LLM determines the command is safe,
    'deny' if genuinely dangerous, or 'escalate' if uncertain.

    Uses Harvey's switchAI Local gateway via HTTP.
    """
    try:
        import requests

        switchai_url = os.getenv("SWITCHAI_URL", "http://localhost:18080")
        switchai_key = os.getenv("SWITCHAI_KEY", "")

        if not switchai_key:
            logger.debug("Smart approvals: no SWITCHAI_KEY available, escalating")
            return "escalate"

        prompt = f"""You are a security reviewer for an AI coding agent. A terminal command was flagged by pattern matching as potentially dangerous.

Command: {command}
Flagged reason: {description}

Assess the ACTUAL risk of this command. Many flagged commands are false positives — for example, `python -c "print('hello')"` is flagged as "script execution via -c flag" but is completely harmless.

Rules:
- APPROVE if the command is clearly safe (benign script execution, safe file operations, development tools, package installs, git operations, etc.)
- DENY if the command could genuinely damage the system (recursive delete of important paths, overwriting system files, fork bombs, wiping disks, dropping databases, etc.)
- ESCALATE if you're uncertain

Respond with exactly one word: APPROVE, DENY, or ESCALATE"""

        response = requests.post(
            f"{switchai_url}/v1/chat/completions",
            headers={
                "Authorization": f"Bearer {switchai_key}",
                "Content-Type": "application/json",
            },
            json={
                "model": "auto",
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 16,
                "temperature": 0,
            },
            timeout=30,
        )

        if response.status_code != 200:
            logger.debug(
                "Smart approvals: LLM call failed (%d), escalating",
                response.status_code,
            )
            return "escalate"

        answer = (
            (
                response.json()
                .get("choices", [{}])[0]
                .get("message", {})
                .get("content", "")
            )
            .strip()
            .upper()
        )

        if "APPROVE" in answer:
            return "approve"
        elif "DENY" in answer:
            return "deny"
        else:
            return "escalate"

    except ImportError:
        logger.debug("Smart approvals: requests library not available, escalating")
        return "escalate"
    except Exception as e:
        logger.debug("Smart approvals: LLM call failed (%s), escalating", e)
        return "escalate"


# =========================================================================
# Approval prompting
# =========================================================================


def _get_approval_config() -> dict:
    """Read the approvals config block from environment or config file."""
    return {
        "mode": os.getenv("HARVEY_APPROVAL_MODE", "manual"),
        "timeout": int(os.getenv("HARVEY_APPROVAL_TIMEOUT", "60")),
    }


def _get_approval_mode() -> str:
    """Read the approval mode from config. Returns 'manual', 'smart', or 'off'."""
    mode = _get_approval_config().get("mode", "manual")
    if isinstance(mode, bool):
        return "off" if mode is False else "manual"
    if isinstance(mode, str):
        normalized = mode.strip().lower()
        return normalized or "manual"
    return "manual"


def prompt_dangerous_approval(
    command: str,
    description: str,
    timeout_seconds: int = 60,
    allow_permanent: bool = True,
) -> str:
    """Prompt the user to approve a dangerous command (CLI only).

    Args:
        allow_permanent: When False, hide the [a]lways option.

    Returns: 'once', 'session', 'always', or 'deny'
    """
    import sys

    try:
        while True:
            print()
            print(f"  ⚠️  DANGEROUS COMMAND: {description}")
            print(f"      {command}")
            print()
            if allow_permanent:
                print("      [o]nce  |  [s]ession  |  [a]lways  |  [d]eny")
            else:
                print("      [o]nce  |  [s]ession  |  [d]eny")
            print()
            sys.stdout.flush()

            result = {"choice": ""}

            def get_input():
                try:
                    prompt = (
                        "      Choice [o/s/a/D]: "
                        if allow_permanent
                        else "      Choice [o/s/D]: "
                    )
                    result["choice"] = input(prompt).strip().lower()
                except (EOFError, OSError):
                    result["choice"] = ""

            thread = threading.Thread(target=get_input, daemon=True)
            thread.start()
            thread.join(timeout=timeout_seconds)

            if thread.is_alive():
                print("\n      ⏱ Timeout - denying command")
                return "deny"

            choice = result["choice"]
            if choice in ("o", "once"):
                print("      ✓ Allowed once")
                return "once"
            elif choice in ("s", "session"):
                print("      ✓ Allowed for this session")
                return "session"
            elif choice in ("a", "always"):
                if not allow_permanent:
                    print("      ✓ Allowed for this session")
                    return "session"
                print("      ✓ Added to permanent allowlist")
                return "always"
            else:
                print("      ✗ Denied")
                return "deny"

    except (EOFError, KeyboardInterrupt):
        print("\n      ✗ Cancelled")
        return "deny"
    finally:
        print()
        sys.stdout.flush()


# =========================================================================
# Main entry point
# =========================================================================


def check_dangerous_command(command: str, env_type: str = "local") -> dict:
    """Check if a command is dangerous and handle approval.

    This is a simplified entry point for detection only.

    Args:
        command: The shell command to check.
        env_type: Terminal backend type ('local', 'ssh', 'docker', etc.).

    Returns:
        {"approved": True/False, "message": str or None, ...}
    """
    if env_type in ("docker", "singularity", "modal", "daytona"):
        return {"approved": True, "message": None}

    # --yolo: bypass all approval prompts
    if os.getenv("HERMES_YOLO_MODE") or os.getenv("HARVEY_YOLO_MODE"):
        return {"approved": True, "message": None}

    is_dangerous, pattern_key, description = detect_dangerous_command(command)
    if not is_dangerous:
        return {"approved": True, "message": None}

    session_key = os.getenv(
        "HERMES_SESSION_KEY", os.getenv("HARVEY_SESSION_KEY", "default")
    )
    if is_approved(session_key, pattern_key):
        return {"approved": True, "message": None}

    return {
        "approved": False,
        "pattern_key": pattern_key,
        "description": description,
        "message": (
            f"⚠️ This command is potentially dangerous ({description}). "
            f"Approval required.\n\n**Command:**\n```\n{command}\n```"
        ),
    }


def check_all_command_guards(command: str, env_type: str = "local") -> dict:
    """Run all pre-exec security checks and return a single approval decision.

    This is the main entry point called before executing any command.
    It orchestrates detection, session checks, and prompting.

    Args:
        command: The shell command to check.
        env_type: Terminal backend type ('local', 'ssh', 'docker', etc.).

    Returns:
        {"approved": True/False, "message": str or None, ...}
    """
    # Skip containers for both checks
    if env_type in ("docker", "singularity", "modal", "daytona"):
        return {"approved": True, "message": None}

    # --yolo or approvals.mode=off: bypass all approval prompts
    approval_mode = _get_approval_mode()
    if (
        os.getenv("HERMES_YOLO_MODE")
        or os.getenv("HARVEY_YOLO_MODE")
        or approval_mode == "off"
    ):
        return {"approved": True, "message": None}

    # Dangerous command check (detection only, no approval)
    is_dangerous, pattern_key, description = detect_dangerous_command(command)

    session_key = os.getenv(
        "HERMES_SESSION_KEY", os.getenv("HARVEY_SESSION_KEY", "default")
    )

    # Nothing to warn about
    if not is_dangerous:
        return {"approved": True, "message": None}

    # Check if already approved
    if is_approved(session_key, pattern_key):
        return {"approved": True, "message": None}

    # Smart approval mode: ask LLM before prompting user
    if approval_mode == "smart":
        verdict = _smart_approve(command, description)
        if verdict == "approve":
            approve_session(session_key, pattern_key)
            logger.debug(
                "Smart approval: auto-approved '%s' (%s)", command[:60], description
            )
            return {"approved": True, "message": None, "smart_approved": True}
        elif verdict == "deny":
            return {
                "approved": False,
                "message": f"BLOCKED by smart approval: {description}. "
                "The command was assessed as genuinely dangerous. Do NOT retry.",
                "smart_denied": True,
            }
        # verdict == "escalate" → fall through to manual prompt

    # Non-interactive environment: just report the issue
    is_cli = os.getenv("HERMES_INTERACTIVE") or os.getenv("HARVEY_INTERACTIVE")
    is_gateway = os.getenv("HERMES_GATEWAY_SESSION") or os.getenv(
        "HARVEY_GATEWAY_SESSION"
    )

    if not is_cli and not is_gateway:
        submit_pending(
            session_key,
            {
                "command": command,
                "pattern_key": pattern_key,
                "description": description,
            },
        )
        return {
            "approved": False,
            "pattern_key": pattern_key,
            "status": "approval_required",
            "command": command,
            "description": description,
            "message": (
                f"⚠️ This command is potentially dangerous ({description}). "
                f"Asking the user for approval.\n\n**Command:**\n```\n{command}\n```"
            ),
        }

    # CLI interactive: prompt user
    choice = prompt_dangerous_approval(command, description)

    if choice == "deny":
        return {
            "approved": False,
            "message": f"BLOCKED: User denied this potentially dangerous command (matched '{description}' pattern). "
            "Do NOT retry this command - the user has explicitly rejected it.",
            "pattern_key": pattern_key,
            "description": description,
        }

    if choice == "session":
        approve_session(session_key, pattern_key)
    elif choice == "always":
        approve_session(session_key, pattern_key)
        approve_permanent(pattern_key)

    return {"approved": True, "message": None}
