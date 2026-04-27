"""
Harvey OS Risk Classifier — tool and command risk classification.

Classifies every tool call and bash command into four risk tiers:
LOW, MEDIUM, HIGH, FORBIDDEN. Integrates with the hook system to block
FORBIDDEN commands outright and flag HIGH commands for approval.

Complements dangerous_command.py (pattern-based detection + approval flow)
by providing a structured risk level that other subsystems can act on.

Usage:
    from core.security.risk_classifier import classify_command, classify_tool, RiskLevel

    level = classify_command("rm -rf /")
    assert level == RiskLevel.FORBIDDEN

    level = classify_tool("bash", {"command": "ls -la"})
    assert level == RiskLevel.LOW

    # Register as a before-hook:
    from core.hooks.hooks import get_hooks
    from core.security.risk_classifier import register_risk_hooks
    register_risk_hooks(get_hooks())
"""

import fnmatch
import logging
import os
import re
from enum import IntEnum
from pathlib import PurePosixPath
from typing import Any, Dict, List, Optional, Tuple

log = logging.getLogger("harvey.security.risk")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


# ═══════════════════════════════════════════════════════════════
#  Risk Levels
# ═══════════════════════════════════════════════════════════════

class RiskLevel(IntEnum):
    """Risk tiers — higher value = higher risk."""
    LOW = 0        # Read-only, informational (ls, cat, git status)
    MEDIUM = 10    # State-changing but reversible (git commit, pip install)
    HIGH = 20      # Destructive or hard to undo (git push --force, kill -9)
    FORBIDDEN = 30 # Never execute autonomously (rm -rf /, fork bombs)


# ═══════════════════════════════════════════════════════════════
#  Tool-Level Defaults
# ═══════════════════════════════════════════════════════════════

# Default risk when a tool isn't in this map: MEDIUM (safe default for unknowns)
TOOL_RISK_MAP: Dict[str, RiskLevel] = {
    # Read-only tools
    "read":               RiskLevel.LOW,
    "file_read":          RiskLevel.LOW,
    "glob":               RiskLevel.LOW,
    "grep":               RiskLevel.LOW,
    "search":             RiskLevel.LOW,
    "list_files":         RiskLevel.LOW,
    # Write tools — reversible but state-changing
    "write":              RiskLevel.MEDIUM,
    "file_write":         RiskLevel.MEDIUM,
    "edit":               RiskLevel.MEDIUM,
    "file_edit":          RiskLevel.MEDIUM,
    "notebook_edit":      RiskLevel.MEDIUM,
    # Bash is classified per-command, not per-tool
    "bash":               RiskLevel.MEDIUM,
    # Web access
    "web_fetch":          RiskLevel.LOW,
    "web_search":         RiskLevel.LOW,
    "browser":            RiskLevel.MEDIUM,
}


# ═══════════════════════════════════════════════════════════════
#  Bash Command Patterns (ordered by risk level)
# ═══════════════════════════════════════════════════════════════

# Each entry: (compiled regex, description)
_PatternList = List[Tuple[re.Pattern, str]]


def _compile(patterns: List[Tuple[str, str]]) -> _PatternList:
    return [(re.compile(p, re.IGNORECASE), desc) for p, desc in patterns]


FORBIDDEN_PATTERNS = _compile([
    (r"\brm\s+(-[^\s]*\s+)*-[^\s]*r[^\s]*\s+/\s*$",  "recursive delete root"),
    (r"\brm\s+(-[^\s]*\s+)*/\s*$",                     "delete root"),
    (r"\bdd\s+.*if=/dev/zero\s+.*of=/dev/sd",          "wipe block device"),
    (r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:",      "fork bomb"),
    (r"\bchmod\s+(-[^\s]*\s+)*777\s+/\s*$",            "world-writable root"),
    (r"\bchmod\s+(-[^\s]*R[^\s]*\s+)*777\s+/",         "recursive world-writable root"),
    (r"\b(curl|wget)\b.*\|\s*(ba)?sh\b",               "pipe remote script to shell"),
    (r"\bmkfs\b",                                       "format filesystem"),
    (r">\s*/dev/sd[a-z]",                               "overwrite block device"),
    (r"\bdd\s+.*of=/dev/sd",                            "raw write to block device"),
])

HIGH_PATTERNS = _compile([
    (r"\bgit\s+push\s+.*--force",                       "force push"),
    (r"\bgit\s+push\s+-f\b",                            "force push (short flag)"),
    (r"\bgit\s+reset\s+--hard",                         "hard reset"),
    (r"\bgit\s+clean\s+-[^\s]*f",                       "git clean force"),
    (r"\bDROP\s+(TABLE|DATABASE)\b",                    "SQL DROP"),
    (r"\bDELETE\s+FROM\b",                              "SQL DELETE"),
    (r"\bTRUNCATE\s+(TABLE)?\s*\w",                     "SQL TRUNCATE"),
    (r"\bkill\s+-9\b",                                  "force kill process"),
    (r"\bkillall\b",                                    "kill all matching processes"),
    (r"\bpkill\b",                                      "pattern kill"),
    (r"\bdocker\s+rm\b",                                "remove docker container"),
    (r"\bdocker\s+rmi\b",                               "remove docker image"),
    (r"\bdocker\s+system\s+prune",                      "docker system prune"),
    (r"\brm\s+(-[^\s]*\s+)*-[^\s]*r",                   "recursive delete"),
    (r"\brm\s+--recursive\b",                           "recursive delete (long)"),
    (r"\bsystemctl\s+(stop|disable|mask)\b",            "stop/disable service"),
    (r">\s*/etc/",                                      "overwrite system config"),
    (r"\bsed\s+-[^\s]*i.*\s/etc/",                      "in-place edit system config"),
    (r"\b(cp|mv)\b.*\s/etc/",                           "write into /etc"),
    (r"\b(cp|mv)\b.*\s/usr/",                           "write into /usr"),
    (r"\bchown\s+(-[^\s]*)?R\s+root",                   "recursive chown root"),
    (r"\bnpm\s+publish\b",                              "publish npm package"),
    (r"\bpip\s+install\s+--force",                      "force pip install"),
])

MEDIUM_PATTERNS = _compile([
    (r"\bgit\s+commit\b",                               "git commit"),
    (r"\bgit\s+merge\b",                                "git merge"),
    (r"\bgit\s+rebase\b",                               "git rebase"),
    (r"\bgit\s+checkout\s",                              "git checkout"),
    (r"\bgit\s+stash\b",                                "git stash"),
    (r"\bpip\s+install\b",                              "pip install"),
    (r"\bnpm\s+install\b",                              "npm install"),
    (r"\byarn\s+add\b",                                 "yarn add"),
    (r"\bbrew\s+install\b",                             "brew install"),
    (r"\bdocker\s+run\b",                               "docker run"),
    (r"\bdocker\s+build\b",                             "docker build"),
    (r"\bdocker\s+compose\s+up\b",                      "docker compose up"),
    (r"\bmkdir\b",                                      "create directory"),
    (r"\btouch\b",                                      "create file"),
    (r"\btee\b",                                        "tee (write)"),
    (r"\bcurl\s+.*-X\s*(POST|PUT|PATCH|DELETE)",        "curl mutating request"),
    (r"\bwget\s+.*-O\b",                                "wget download"),
])

LOW_PATTERNS = _compile([
    (r"^\s*ls\b",           "list files"),
    (r"^\s*cat\b",          "cat file"),
    (r"^\s*head\b",         "head"),
    (r"^\s*tail\b",         "tail"),
    (r"^\s*wc\b",           "word count"),
    (r"^\s*grep\b",         "grep"),
    (r"^\s*rg\b",           "ripgrep"),
    (r"^\s*find\b",         "find"),
    (r"^\s*which\b",        "which"),
    (r"^\s*echo\b",         "echo"),
    (r"^\s*printf\b",       "printf"),
    (r"^\s*pwd\b",          "pwd"),
    (r"^\s*date\b",         "date"),
    (r"^\s*whoami\b",       "whoami"),
    (r"^\s*uname\b",       "uname"),
    (r"^\s*env\b",          "env"),
    (r"^\s*printenv\b",     "printenv"),
    (r"\bgit\s+status\b",   "git status"),
    (r"\bgit\s+log\b",      "git log"),
    (r"\bgit\s+diff\b",     "git diff"),
    (r"\bgit\s+show\b",     "git show"),
    (r"\bgit\s+branch\b",   "git branch (list)"),
    (r"\bgit\s+remote\s+-v", "git remote -v"),
    (r"\bjq\b",             "jq"),
    (r"^\s*file\b",         "file type check"),
    (r"^\s*stat\b",         "file stats"),
    (r"^\s*du\b",           "disk usage"),
    (r"^\s*df\b",           "disk free"),
    (r"^\s*python3?\s+--version", "python version"),
    (r"^\s*node\s+--version",     "node version"),
])


# ═══════════════════════════════════════════════════════════════
#  Protected Files
# ═══════════════════════════════════════════════════════════════

PROTECTED_FILE_PATTERNS: List[str] = [
    ".gitconfig",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".ssh/*",
    ".env",
    ".env.*",
    "credentials.json",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_ed25519",
    "known_hosts",
    "authorized_keys",
    ".netrc",
    ".npmrc",       # can contain auth tokens
    ".pypirc",      # can contain auth tokens
    "token.json",
    "secrets.yaml",
    "secrets.yml",
]

# System directories — writes here are automatically HIGH risk
SYSTEM_DIRS: List[str] = [
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/boot",
    "/lib",
    "/lib64",
    "/opt",
    "/var/log",
    "/System",      # macOS
    "/Library",     # macOS
]


# ═══════════════════════════════════════════════════════════════
#  Classification Functions
# ═══════════════════════════════════════════════════════════════

def is_protected_file(path: str) -> bool:
    """Check if a file path matches any protected file pattern.

    Args:
        path: Absolute or relative file path.

    Returns:
        True if the file should be treated as protected/sensitive.
    """
    if not path:
        return False

    # Normalize: resolve ~, get basename and full path
    expanded = os.path.expanduser(path)
    basename = os.path.basename(expanded)

    for pattern in PROTECTED_FILE_PATTERNS:
        # Pattern with directory component (e.g., ".ssh/*")
        if "/" in pattern:
            # Match against the full path
            if fnmatch.fnmatch(expanded, f"*/{pattern}"):
                return True
            if fnmatch.fnmatch(path, f"*/{pattern}"):
                return True
        else:
            # Match basename only
            if fnmatch.fnmatch(basename, pattern):
                return True

    return False


def _is_system_path(path: str) -> bool:
    """Check if a path is inside a protected system directory."""
    expanded = os.path.expanduser(path)
    try:
        resolved = str(PurePosixPath(expanded))
    except Exception:
        resolved = expanded

    for sys_dir in SYSTEM_DIRS:
        if resolved.startswith(sys_dir + "/") or resolved == sys_dir:
            return True
    return False


def classify_command(cmd: str) -> RiskLevel:
    """Classify a bash command's risk level.

    Scans the command against pattern tiers from most dangerous to least.
    Returns the highest matching risk level.

    Args:
        cmd: The shell command string.

    Returns:
        RiskLevel enum value.
    """
    if not cmd or not cmd.strip():
        return RiskLevel.LOW

    # Strip leading whitespace for pattern matching
    cmd_stripped = cmd.strip()

    # Check tiers in order: FORBIDDEN > HIGH > MEDIUM > LOW
    for pattern, desc in FORBIDDEN_PATTERNS:
        if pattern.search(cmd_stripped):
            log.debug("FORBIDDEN: %s matched '%s'", cmd_stripped[:80], desc)
            return RiskLevel.FORBIDDEN

    for pattern, desc in HIGH_PATTERNS:
        if pattern.search(cmd_stripped):
            log.debug("HIGH: %s matched '%s'", cmd_stripped[:80], desc)
            return RiskLevel.HIGH

    for pattern, desc in MEDIUM_PATTERNS:
        if pattern.search(cmd_stripped):
            log.debug("MEDIUM: %s matched '%s'", cmd_stripped[:80], desc)
            return RiskLevel.MEDIUM

    for pattern, desc in LOW_PATTERNS:
        if pattern.search(cmd_stripped):
            return RiskLevel.LOW

    # Default: unrecognized commands get MEDIUM (cautious default)
    return RiskLevel.MEDIUM


def classify_tool(tool_name: str, args: Optional[Dict[str, Any]] = None) -> RiskLevel:
    """Classify a tool call's risk level.

    For bash tools, delegates to classify_command. For file write tools,
    checks if the target is a protected file or system path. For other
    tools, uses the TOOL_RISK_MAP default.

    Args:
        tool_name: Name of the tool being invoked.
        args: Tool arguments dict.

    Returns:
        RiskLevel enum value.
    """
    args = args or {}

    # Bash: classify based on the actual command
    if tool_name in ("bash", "shell", "terminal", "exec"):
        cmd = args.get("command", "")
        return classify_command(cmd)

    # File write tools: check target path
    if tool_name in ("write", "file_write", "edit", "file_edit"):
        file_path = args.get("file_path", "") or args.get("path", "")
        if file_path:
            if is_protected_file(file_path):
                log.warning("HIGH risk: write to protected file %s", file_path)
                return RiskLevel.HIGH
            if _is_system_path(file_path):
                log.warning("HIGH risk: write to system path %s", file_path)
                return RiskLevel.HIGH

    # Fall back to tool-level default
    return TOOL_RISK_MAP.get(tool_name, RiskLevel.MEDIUM)


def classify_command_detailed(cmd: str) -> Tuple[RiskLevel, str]:
    """Like classify_command but also returns the matched pattern description.

    Returns:
        (RiskLevel, description) — description is empty string for LOW/unmatched.
    """
    if not cmd or not cmd.strip():
        return RiskLevel.LOW, ""

    cmd_stripped = cmd.strip()

    for pattern, desc in FORBIDDEN_PATTERNS:
        if pattern.search(cmd_stripped):
            return RiskLevel.FORBIDDEN, desc

    for pattern, desc in HIGH_PATTERNS:
        if pattern.search(cmd_stripped):
            return RiskLevel.HIGH, desc

    for pattern, desc in MEDIUM_PATTERNS:
        if pattern.search(cmd_stripped):
            return RiskLevel.MEDIUM, desc

    for pattern, desc in LOW_PATTERNS:
        if pattern.search(cmd_stripped):
            return RiskLevel.LOW, desc

    return RiskLevel.MEDIUM, "unrecognized command"


# ═══════════════════════════════════════════════════════════════
#  Hook Integration
# ═══════════════════════════════════════════════════════════════

def register_risk_hooks(hook_manager) -> None:
    """Register the risk classifier as a before-hook on the given HookManager.

    - FORBIDDEN commands are blocked outright.
    - HIGH commands targeting protected files are blocked.
    - All calls get a 'risk_level' tag in their context for downstream hooks.

    Args:
        hook_manager: An instance of core.hooks.hooks.HookManager.
    """
    from core.hooks.hooks import HookContext, HookResult

    def risk_classifier_guard(ctx: HookContext) -> Optional[HookResult]:
        """Before-hook: classify risk and block FORBIDDEN/protected-file writes."""
        level = classify_tool(ctx.tool_name, ctx.args)

        # Stash risk level on context for downstream hooks / logging
        ctx.args["_risk_level"] = level.name

        if level == RiskLevel.FORBIDDEN:
            _, desc = classify_command_detailed(ctx.args.get("command", ""))
            reason = f"FORBIDDEN ({desc}): command blocked by risk classifier"
            log.warning("Blocked FORBIDDEN command: %s", ctx.args.get("command", "")[:120])
            return HookResult(block=True, reason=reason)

        # Block writes to protected files regardless of tool
        if ctx.tool_name in ("write", "file_write", "edit", "file_edit"):
            file_path = ctx.args.get("file_path", "") or ctx.args.get("path", "")
            if is_protected_file(file_path):
                reason = f"Protected file: {file_path} — manual approval required"
                log.warning("Blocked write to protected file: %s", file_path)
                return HookResult(block=True, reason=reason)

        return None

    hook_manager.register(
        phase="before",
        pattern="*",
        fn=risk_classifier_guard,
        priority=95,  # Just below dangerous_command_guard (100)
        name="risk_classifier_guard",
    )
    log.info("Risk classifier hook registered (priority=95)")
