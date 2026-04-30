"""
Action grants for remote-operator tools.

This extends the existing user_grants.json runtime permission layer with
`action:<kind>:<target-hash>` scopes. It deliberately reuses the same
sidecar lock, rate limits, owner field, origin_turn_id guard, expiry
semantics, and audit log as write grants.

v1 intentionally grants exact actions only — no wildcards. A grant for one
shell command does not authorize a different command.
"""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import subprocess
import time
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Optional
from urllib.parse import urlparse

from core.capability import (
    CONVERSATIONAL_CHANNELS,
    RateLimitExceeded,
    UserGrantsFile,
    default_grants_path,
    log_audit,
)
from core.capability.perms_core import PermsError, parse_duration

_ALLOWED_ACTIONS = frozenset({
    "shell/run",
    "browser/control",
    "process/control",
    "app/control",
})

# Commands that remain hard-blocked even with an action grant. These are
# either irreversible, privilege-escalating, or credential-exfiltration shaped.
_HARD_BLOCK_PATTERNS = (
    "sudo ",
    " su ",
    "rm -rf /",
    "rm -fr /",
    "mkfs",
    "diskutil erase",
    "dd if=",
    ":(){",
    "chmod -r 777 /",
    "chmod 777 /",
    "chown -r ",
    "security find-generic-password",
    "security dump-keychain",
    "cat ~/.ssh",
    "cat $home/.ssh",
    "curl ",
    "wget ",
    " | sh",
    " | bash",
    "bash -c",
    "zsh -c",
    "sh -c",
)

_SHELL_METACHARS = frozenset({"|", ";", "&&", "||", ">", "<", "`", "$(", "&"})


@dataclass
class ActionGrantArgs:
    action: str
    target: str
    duration: str = "1h"
    label: str = ""
    plugin: str = "harveychat"
    origin_turn_id: str = ""
    confirm: Optional[str] = None


def _normalize_action(action: str) -> str:
    a = (action or "").strip().lower()
    if a not in _ALLOWED_ACTIONS:
        allowed = ", ".join(sorted(_ALLOWED_ACTIONS))
        raise PermsError(f"unsupported action {action!r}; use one of: {allowed}")
    return a


def _normalize_target(action: str, target: str) -> str:
    t = " ".join((target or "").strip().split())
    if not t:
        raise PermsError("target is required for action grant")
    if len(t) > 1000:
        raise PermsError("target too long; max 1000 chars")
    if action == "shell/run":
        reason = shell_command_block_reason(t)
        if reason:
            raise PermsError(f"shell command refused even with grant: {reason}")
    return t


def action_scope(action: str, target: str) -> str:
    """Build the exact action scope stored in user_grants.json."""
    a = _normalize_action(action)
    t = _normalize_target(a, target)
    digest = hashlib.sha256(t.encode("utf-8")).hexdigest()[:16]
    return f"action:{a}:{digest}"


def action_preview(action: str, target: str) -> str:
    t = " ".join((target or "").split())
    if len(t) > 120:
        t = t[:119] + "…"
    return f"{action} {t}".strip()


def _active_action_grant(action: str, target: str):
    scope = action_scope(action, target)
    grants = UserGrantsFile.load(default_grants_path())
    for grant in grants.active_grants():
        if grant.scope == scope:
            return grant
    return None


def has_action_grant(action: str, target: str) -> bool:
    return _active_action_grant(action, target) is not None


def _audit_action_grant_denial(args: ActionGrantArgs, correlation_id: str) -> None:
    log_audit(
        verb="perms/action_grant",
        scope_requested=f"{args.action}:{action_preview(args.action, args.target)}",
        scope_granted=None,
        result="denied",
        plugin=args.plugin,
        correlation_id=correlation_id,
    )


def grant_action(args: ActionGrantArgs, *, now: Optional[datetime] = None) -> str:
    """Create one exact action grant and return a quotable reply."""
    now = now or datetime.now(tz=timezone.utc)
    if args.plugin in CONVERSATIONAL_CHANNELS and not args.origin_turn_id:
        _audit_action_grant_denial(args, "reason:missing_origin_turn_id")
        raise PermsError(
            f"origin_turn_id required on conversational channels "
            f"(plugin={args.plugin}); this action grant call appears to be "
            "agent-initiated without a human turn binding"
        )

    try:
        action = _normalize_action(args.action)
        target = _normalize_target(action, args.target)
    except PermsError:
        _audit_action_grant_denial(args, "reason:bad_action")
        raise

    try:
        dur = parse_duration(args.duration)
    except PermsError:
        _audit_action_grant_denial(args, "reason:bad_duration")
        raise
    expires_at = None if dur is None else now + dur

    # Permanent remote actions are intentionally harder than permanent write
    # grants: explicit confirm always required. This prevents a casual remote
    # chat phrase from making shell/browser control stick forever.
    if expires_at is None and args.confirm != "yes-really":
        _audit_action_grant_denial(args, "reason:permanent_action_unconfirmed")
        raise PermsError("permanent action grant requires confirm='yes-really'")

    scope = action_scope(action, target)
    label = args.label or action_preview(action, target)
    grants_file = UserGrantsFile.load(default_grants_path())
    try:
        grant = grants_file.add(
            scope=scope,
            expires_at=expires_at,
            label=label,
            plugin=args.plugin,
            origin_turn_id=args.origin_turn_id,
            granted_by="sebastian",
            now=now,
        )
    except RateLimitExceeded as e:
        _audit_action_grant_denial(args, "reason:rate_limit")
        raise PermsError(e.reason) from e

    log_audit(
        verb="perms/action_grant",
        scope_requested=scope,
        scope_granted=grant.id,
        result="allowed",
        plugin=args.plugin,
    )
    expires = "permanent" if grant.expires_at is None else f"until {grant.expires_at.astimezone().strftime('%H:%M %Z')}"
    return (
        f"Granted. {action} allowed for exact target `{target}` {expires}. "
        f"Revoke: makakoo perms revoke {grant.id}"
    )


def list_action_grants(include_expired: bool = False, *, now: Optional[datetime] = None) -> str:
    now = now or datetime.now(tz=timezone.utc)
    grants = UserGrantsFile.load(default_grants_path())
    pool = grants.grants if include_expired else grants.active_grants(now)
    action_grants = [g for g in pool if g.scope.startswith("action:")]
    if not action_grants:
        return "No active action grants." if not include_expired else "No action grants."
    parts: list[str] = []
    for g in action_grants:
        if g.expires_at is None:
            when = "permanent"
        elif g.is_expired(now):
            when = f"expired {g.expires_at.astimezone().strftime('%H:%M %Z')}"
        else:
            when = f"until {g.expires_at.astimezone().strftime('%H:%M %Z')}"
        parts.append(f"{g.id}: {g.scope} ({g.label}) {when}")
    return "Action grants:\n" + "\n".join(parts)


def shell_command_block_reason(command: str) -> str:
    c = (command or "").strip()
    if not c:
        return "empty command"
    if len(c) > 1000:
        return "command too long"
    lower = f" {c.lower()} "
    for pattern in _HARD_BLOCK_PATTERNS:
        if pattern in lower:
            return f"hard-blocked pattern {pattern.strip()!r}"
    for token in _SHELL_METACHARS:
        if token in c:
            return f"shell metacharacter {token!r} not allowed in remote operator v1"
    try:
        shlex.split(c)
    except ValueError as e:
        return f"invalid shell syntax: {e}"
    return ""


def browser_read_target(url: str, query: str = "summary", browser: str = "default") -> str:
    """Normalize browser-read intent into the exact action-grant target."""
    u = (url or "").strip()
    parsed = urlparse(u)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        raise PermsError("browser URL must be http(s) with a host")
    q = " ".join((query or "summary").strip().split()) or "summary"
    b = " ".join((browser or "default").strip().split()) or "default"
    if len(q) > 300:
        q = q[:300]
    if len(b) > 40:
        raise PermsError("browser name too long")
    return f"browser/read url={u} query={q} browser={b}"


def _browser_harness_paths() -> tuple[str, str]:
    home = (
        os.environ.get("MAKAKOO_HOME")
        or os.environ.get("HARVEY_HOME")
        or os.path.expanduser("~/MAKAKOO")
    )
    plugin = os.path.join(home, "plugins", "agent-browser-harness")
    return (
        os.path.join(plugin, ".venv", "bin", "python"),
        os.path.join(plugin, "upstream", "run.py"),
    )


def _discover_cdp_ws() -> str:
    url = os.environ.get("BU_CDP_URL", "http://127.0.0.1:9222/json/version")
    try:
        with urllib.request.urlopen(url, timeout=2) as resp:
            payload = json.loads(resp.read().decode("utf-8", errors="replace"))
        return str(payload.get("webSocketDebuggerUrl") or "")
    except Exception:
        return ""


def _browser_rejection(target: str) -> str:
    return (
        "operator_browser_read rejected: no active browser/control grant for this exact target. "
        "Ask Sebastian for explicit permission, then call "
        f"grant_action_access(action='browser/control', target={target!r}, duration='1h')."
    )


def run_granted_browser_read(
    url: str,
    query: str = "summary",
    browser: str = "default",
    timeout_seconds: int = 60,
) -> str:
    """Read a page through real Chrome only with an exact browser/control grant."""
    try:
        target = browser_read_target(url, query, browser)
    except PermsError as e:
        return f"operator_browser_read rejected: {e.message}"

    grant = _active_action_grant("browser/control", target)
    if grant is None:
        scope = action_scope("browser/control", target)
        log_audit(
            verb="action/browser_read",
            scope_requested=scope,
            scope_granted=None,
            result="denied",
            correlation_id="reason:no_action_grant",
        )
        return _browser_rejection(target)

    py, run_py = _browser_harness_paths()
    if not os.path.exists(py):
        return (
            f"operator_browser_read error: agent-browser-harness venv python missing at {py}. "
            "Run `makakoo plugin install --core agent-browser-harness`."
        )
    if not os.path.exists(run_py):
        return (
            f"operator_browser_read error: browser-harness run.py missing at {run_py}. "
            "Run `makakoo plugin install --core agent-browser-harness`."
        )

    u_json = json.dumps(url)
    code = f"""
goto({u_json})
wait_for_load(15)
info = page_info()
print("PAGE_INFO", info)
content = js("(()=>{{const title=document.title||''; const text=(document.body&&document.body.innerText)||''; const links=[...document.querySelectorAll('a[href]')].slice(0,30).map(a=>`${{a.innerText.trim()}} -> ${{a.href}}`).join('\\\\n'); return `TITLE: ${{title}}\\\\n\\\\nTEXT:\\\\n${{text}}\\\\n\\\\nLINKS:\\\\n${{links}}`;}})()")
print("PAGE_CONTENT_START")
print(content)
print("PAGE_CONTENT_END")
"""
    timeout = max(5, min(int(timeout_seconds or 60), 180))
    start = time.monotonic()
    env = os.environ.copy()
    env["BU_NAME"] = browser or "default"
    if "BU_CDP_WS" not in env:
        cdp_ws = _discover_cdp_ws()
        if cdp_ws:
            env["BU_CDP_WS"] = cdp_ws
    try:
        result = subprocess.run(
            [py, run_py],
            input=code,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        elapsed = int((time.monotonic() - start) * 1000)
        output = (result.stdout.strip() or result.stderr.strip() or "(browser produced no output)")
        if len(output) > 8000:
            output = output[:8000] + "\n... (truncated)"
        log_audit(
            verb="action/browser_read",
            scope_requested=target,
            scope_granted=grant.id,
            result="allowed" if result.returncode == 0 else "error",
            duration_ms=elapsed,
            bytes_out=len(output.encode("utf-8", errors="ignore")),
        )
        if result.returncode != 0:
            return f"operator_browser_read failed exit={result.returncode}\n{output}"
        return output
    except subprocess.TimeoutExpired:
        log_audit(
            verb="action/browser_read",
            scope_requested=target,
            scope_granted=grant.id,
            result="error",
            correlation_id="reason:timeout",
        )
        return f"operator_browser_read timeout after {timeout}s"
    except Exception as e:
        log_audit(
            verb="action/browser_read",
            scope_requested=target,
            scope_granted=grant.id,
            result="error",
            correlation_id=f"reason:{type(e).__name__}",
        )
        return f"operator_browser_read error: {e}"


def run_granted_shell_command(command: str, timeout_seconds: int = 30) -> str:
    """Run an exact shell command only if an active action grant exists."""
    target = " ".join((command or "").strip().split())
    reason = shell_command_block_reason(target)
    if reason:
        log_audit(
            verb="action/shell_run",
            scope_requested=target,
            scope_granted=None,
            result="denied",
            correlation_id=f"reason:{reason[:80]}",
        )
        return f"operator_run_command rejected: {reason}"

    try:
        grant = _active_action_grant("shell/run", target)
    except PermsError as e:
        return f"operator_run_command rejected: {e.message}"
    if grant is None:
        scope = action_scope("shell/run", target)
        log_audit(
            verb="action/shell_run",
            scope_requested=scope,
            scope_granted=None,
            result="denied",
            correlation_id="reason:no_action_grant",
        )
        return (
            "operator_run_command rejected: no active action grant for this exact command. "
            "Ask Sebastian for explicit permission, then call "
            f"grant_action_access(action='shell/run', target={target!r}, duration='1h')."
        )

    timeout = max(1, min(int(timeout_seconds or 30), 120))
    argv = shlex.split(target)
    start = time.monotonic()
    try:
        env = os.environ.copy()
        env["PATH"] = ":".join([
            "/usr/local/bin",
            "/opt/homebrew/bin",
            os.path.expanduser("~/.nvm/versions/node/v22.17.0/bin"),
            os.path.expanduser("~/bin"),
            env.get("PATH", ""),
        ])
        result = subprocess.run(
            argv,
            shell=False,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
        elapsed = int((time.monotonic() - start) * 1000)
        output = result.stdout.strip() or result.stderr.strip() or "(command produced no output)"
        if len(output) > 5000:
            output = output[:5000] + "\n... (truncated)"
        log_audit(
            verb="action/shell_run",
            scope_requested=target,
            scope_granted=grant.id,
            result="allowed" if result.returncode == 0 else "error",
            duration_ms=elapsed,
            bytes_out=len(output.encode("utf-8", errors="ignore")),
        )
        return f"exit={result.returncode}\n{output}"
    except subprocess.TimeoutExpired:
        log_audit(
            verb="action/shell_run",
            scope_requested=target,
            scope_granted=grant.id,
            result="error",
            correlation_id="reason:timeout",
        )
        return f"operator_run_command timeout after {timeout}s"
    except Exception as e:
        log_audit(
            verb="action/shell_run",
            scope_requested=target,
            scope_granted=grant.id,
            result="error",
            correlation_id=f"reason:{type(e).__name__}",
        )
        return f"operator_run_command error: {e}"
