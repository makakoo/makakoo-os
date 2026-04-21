"""
Canonical Python logic for the three runtime user-grant tools.

Every conversational surface that grants / revokes / lists write
access — HarveyChat, Telegram, and any future Python-native agent —
calls through this module. The Rust MCP handlers at
`makakoo-mcp/src/handlers/tier_b/perms.rs` implement the same
behavior against `makakoo_core::capability::UserGrants` and share a
drift-gate test fixture at `tests/fixtures/grant_tool_vectors.json`.

**Why this file exists (LD#13):**

HarveyChat's agent loop never traverses MCP, so MCP-only registration
would leave Telegram + the local bridge unable to grant. `perms_core`
is the one-stop Python entry point so guardrails cannot drift between
the two conversational registrations.

**Behavior contracts that must match the Rust handler 1:1:**

- Scope refusal: `/`, `~`, `~/`, `$HOME`, empty, `*`, `**`, `.`, `./`,
  `$HOME/`, `~/**`, `$HOME/**` → refused with `scope too broad`.
- Duration grammar: `30m | 1h | 24h | 7d | permanent`. Default 1h.
  Anything else → refused with the valid list.
- Permanent outside `$MAKAKOO_HOME` requires `confirm="yes-really"`.
- Rate limit — global 20 active + 50 creates/hour (LD#14 / lope F7).
- Reply strings are quotable — the LLM quotes them back verbatim.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Optional

from core.capability import (
    RateLimitExceeded,
    UserGrantsFile,
    default_grants_path,
    escape_audit_field,
    log_audit,
    new_grant_id,
)
from core.capability import rate_limit
from core.capability.user_grants import Grant

# Shared with Rust `makakoo_core::capability::rate_limit` via file-on-disk
# contract. Values mirrored in `rate_limit.py` constants.

# ═══════════════════════════════════════════════════════════════
#  Exception taxonomy
# ═══════════════════════════════════════════════════════════════


class PermsError(Exception):
    """Tool-level refusal. Message is the reply the LLM will quote."""

    def __init__(self, message: str):
        super().__init__(message)
        self.message = message


# ═══════════════════════════════════════════════════════════════
#  Conversational channels (v0.3.1 Phase C)
# ═══════════════════════════════════════════════════════════════

#: Plugin identifiers that represent a live human conversational turn.
#: Grants from these surfaces MUST carry a non-empty ``origin_turn_id``
#: binding the grant to the turn that issued the human "yes". An empty
#: ``origin_turn_id`` from one of these plugins is treated as a
#: prompt-injection signature: a legitimate tool-call would carry the
#: host-supplied turn id; a fabricated one wouldn't.
#:
#: Values match the slugs documented in ``spec/USER_GRANTS.md §11.1``
#: (and the shell-rc exports in global_bootstrap v12+).
#:
#: **Additive only.** Do not remove existing entries — the drift-gate
#: test in ``test_user_grants_enforcement.py`` expects every known
#: conversational host to appear here. New chat surfaces must be
#: appended.
CONVERSATIONAL_CHANNELS: frozenset[str] = frozenset({
    "claude-code",
    "gemini-cli",
    "codex",
    "opencode",
    "vibe",
    "cursor",
    "qwen",
    "pi",
    "harveychat",
    "harveychat-telegram",
    "harveychat-web",
})


# ═══════════════════════════════════════════════════════════════
#  Guardrails — duration + scope
# ═══════════════════════════════════════════════════════════════

_BARE_BROAD_SCOPES = {
    "/",
    "~",
    "~/",
    "$HOME",
    "",
    "*",
    "**",
    ".",
    "./",
    "$HOME/",
    "~/**",
    "$HOME/**",
}


def parse_duration(s: str) -> Optional[timedelta]:
    """Strict grammar — `30m|1h|24h|7d|permanent`. None = permanent.

    Mirrors Rust `commands::perms::parse_duration`. Natural-language
    phrases are rejected per LD#15 / lope F12; this is the shared
    attack-surface reduction.
    """
    raw = (s or "").strip()
    if not raw:
        raise PermsError(
            "empty duration; use 30m | 1h | 24h | 7d | permanent"
        )
    if raw.lower() == "permanent":
        return None
    m = re.fullmatch(r"(\d+)([mhd])", raw)
    if m is None:
        raise PermsError(
            f"unsupported duration {raw!r}; use 30m | 1h | 24h | 7d | permanent"
        )
    n = int(m.group(1))
    if n <= 0:
        raise PermsError(
            f"non-positive duration {raw!r}; use 30m | 1h | 24h | 7d | permanent"
        )
    unit = m.group(2)
    td = {
        "m": timedelta(minutes=n),
        "h": timedelta(hours=n),
        "d": timedelta(days=n),
    }[unit]
    if td > timedelta(days=365):
        raise PermsError(
            f"duration {raw!r} exceeds 365 days — shorten or split"
        )
    return td


def validate_and_expand_scope(raw: str) -> str:
    """Reject too-broad scopes; expand `~`/`$MAKAKOO_HOME`/`$HOME`.

    Returns the absolute path string. Relative paths resolve against
    the current cwd at grant-time (spec/USER_GRANTS.md §3.1).
    """
    if raw is None:
        raise PermsError("empty scope — grant a specific directory")
    trimmed = raw.strip()
    if trimmed in _BARE_BROAD_SCOPES:
        raise PermsError(
            f"scope {trimmed!r} is too broad — grant a specific subdirectory"
        )
    # Explicit HOME-catch-all guard.
    if trimmed in ("$HOME/", "~/**", "$HOME/**"):
        raise PermsError(
            f"scope {trimmed!r} covers the entire home directory — grant a "
            "specific subdirectory"
        )

    home = os.environ.get("HOME") or os.path.expanduser("~")
    makakoo_home = os.environ.get("MAKAKOO_HOME") or os.environ.get(
        "HARVEY_HOME"
    ) or os.path.expanduser("~/MAKAKOO")

    expanded = trimmed
    if expanded.startswith("~/"):
        expanded = os.path.join(home, expanded[2:])
    elif expanded == "~":
        expanded = home
    expanded = expanded.replace("$MAKAKOO_HOME", makakoo_home)
    expanded = expanded.replace("$HOME", home)

    if not os.path.isabs(expanded):
        expanded = os.path.join(os.getcwd(), expanded)

    if expanded == "/" or expanded == "/**":
        raise PermsError(
            "expanded scope resolves to root — refuse to grant "
            "filesystem-wide write"
        )
    return expanded


def _build_stored_scope(abs_path: str) -> str:
    """Turn a canonical absolute path into an `fs/write:<glob>` string.

    Bare directories get a `/**` suffix so descendants are covered
    (matches baseline root semantics). Explicit globs pass through.
    """
    if "*" in abs_path:
        return f"fs/write:{abs_path}"
    if abs_path.endswith("/"):
        return f"fs/write:{abs_path}**"
    return f"fs/write:{abs_path}/**"


# ═══════════════════════════════════════════════════════════════
#  Reply templates (LD#13 — shared Python/Rust wording via fixture)
# ═══════════════════════════════════════════════════════════════


def grant_success_msg(grant: Grant) -> str:
    if grant.expires_at is None:
        expires = "permanent"
    else:
        local = grant.expires_at.astimezone()
        expires = f"until {local.strftime('%H:%M %Z')}"
    scope_glob = grant.scope[len("fs/write:"):] if grant.scope.startswith(
        "fs/write:"
    ) else grant.scope
    return (
        f"Granted. {scope_glob} writable {expires}. "
        f"Revoke: makakoo perms revoke {grant.id}"
    )


def revoke_success_msg(grant_id: str, scope_glob: str) -> str:
    return (
        f"Revoked {grant_id}. {scope_glob} no longer writable."
    )


def list_summary_msg(
    baseline_roots: list[str],
    active: list[Grant],
    expired_count: int,
) -> str:
    """Conversational short summary for `list_write_grants`.

    Designed to be quotable verbatim: the LLM should NOT rewrite this.
    """
    baseline_str = ", ".join(baseline_roots) if baseline_roots else "(none)"
    if not active:
        return (
            f"Baseline: {baseline_str}. No active grants. "
            f"{expired_count} expired today."
        )
    active_lines = []
    for g in active:
        scope = g.scope[len("fs/write:"):] if g.scope.startswith("fs/write:") else g.scope
        if g.expires_at is None:
            when = "permanent"
        else:
            local = g.expires_at.astimezone()
            when = f"until {local.strftime('%H:%M %Z')}"
        active_lines.append(f"{scope} {when}")
    return (
        f"Baseline: {baseline_str}. {len(active)} active grant"
        f"{'s' if len(active) != 1 else ''}: {', '.join(active_lines)}. "
        f"{expired_count} expired today."
    )


# ═══════════════════════════════════════════════════════════════
#  Core handlers (called by HARVEY_TOOLS wrappers)
# ═══════════════════════════════════════════════════════════════


@dataclass
class GrantArgs:
    path: str
    duration: str = "1h"
    label: str = ""
    plugin: str = "harveychat"
    origin_turn_id: str = ""
    confirm: Optional[str] = None  # "yes-really" enables permanent-outside-home


def _audit_grant_denial(args: "GrantArgs", correlation_id: str) -> None:
    """Emit one `perms/grant` audit entry with `result="denied"` and a
    taxonomy tag on `correlation_id`. v0.3.1 Phase B — rejections must
    leave a forensic trace so intrusion detection on the grant system
    is possible after the fact.
    """
    log_audit(
        verb="perms/grant",
        scope_requested=args.path or "",
        scope_granted=None,
        result="denied",
        plugin=args.plugin,
        correlation_id=correlation_id,
    )


# v0.3.3 — callers allowed to revoke any grant regardless of owner.
# CLI is the admin escape hatch (Sebastian running `makakoo perms
# revoke` directly); sancho-native is the background purge path.
_ADMIN_REVOKE_BYPASS: frozenset[str] = frozenset({"cli", "sancho-native"})


def _audit_revoke_denial(
    args: "RevokeArgs", target_id: str, correlation_id: str
) -> None:
    """v0.3.3 Phase A — emit an audit entry when a revoke is refused
    by the ownership gate. Mirrors `_audit_grant_denial` shape."""
    log_audit(
        verb="perms/revoke",
        scope_requested=target_id,
        scope_granted=None,
        result="denied",
        plugin=args.plugin,
        correlation_id=correlation_id,
    )


def do_grant(args: GrantArgs, *, now: Optional[datetime] = None) -> str:
    """Core grant flow. Returns the quotable reply string."""
    now = now or datetime.now(tz=timezone.utc)

    # v0.3.1 Phase C — conversational channels require a non-empty
    # origin_turn_id. The check runs before scope/duration so that
    # prompt-injected calls fail fast with a provenance signal, not
    # a guardrail one. CLI + sancho-native are intentionally out of
    # the set — they don't carry a human turn.
    if args.plugin in CONVERSATIONAL_CHANNELS and not args.origin_turn_id:
        _audit_grant_denial(args, "reason:missing_origin_turn_id")
        raise PermsError(
            f"origin_turn_id required on conversational channels "
            f"(plugin={args.plugin}); this grant call appears to be "
            "agent-initiated without a human turn binding"
        )

    try:
        abs_path = validate_and_expand_scope(args.path)
    except PermsError:
        _audit_grant_denial(args, "reason:too_broad")
        raise
    try:
        dur = parse_duration(args.duration)
    except PermsError:
        _audit_grant_denial(args, "reason:bad_duration")
        raise
    expires_at = None if dur is None else now + dur

    # Permanent-outside-HOME confirmation gate.
    if expires_at is None:
        makakoo_home = (
            os.environ.get("MAKAKOO_HOME")
            or os.environ.get("HARVEY_HOME")
            or os.path.expanduser("~/MAKAKOO")
        )
        home_real = os.path.realpath(makakoo_home)
        path_real = os.path.realpath(abs_path)
        if not path_real.startswith(home_real.rstrip(os.sep) + os.sep) and path_real != home_real:
            if args.confirm != "yes-really":
                _audit_grant_denial(
                    args, "reason:permanent_outside_home_unconfirmed"
                )
                raise PermsError(
                    f"permanent grant outside $MAKAKOO_HOME ({abs_path}) "
                    "requires confirm='yes-really'"
                )

    # Load store, rate-limit, append.
    grants_file = UserGrantsFile.load(default_grants_path())
    try:
        new_grant_obj = grants_file.add(
            scope=_build_stored_scope(abs_path),
            expires_at=expires_at,
            label=args.label,
            plugin=args.plugin,
            origin_turn_id=args.origin_turn_id,
            granted_by="sebastian",
            now=now,
        )
    except RateLimitExceeded as e:
        reason = (
            "reason:rate_limit_active"
            if e.creates_in_window == 0
            else "reason:rate_limit_hourly"
        )
        _audit_grant_denial(args, reason)
        raise PermsError(e.reason) from e

    # Audit emit — perms/grant verb, scope_granted = new id.
    log_audit(
        verb="perms/grant",
        scope_requested=new_grant_obj.scope,
        scope_granted=new_grant_obj.id,
        result="allowed",
        plugin=args.plugin,
    )

    return grant_success_msg(new_grant_obj)


@dataclass
class RevokeArgs:
    grant_id: Optional[str] = None
    path: Optional[str] = None
    plugin: str = "harveychat"


def do_revoke(args: RevokeArgs, *, now: Optional[datetime] = None) -> str:
    """Revoke by id OR by scope path. `path="last"`/`"latest"` resolves
    to the most recently-created active grant (E.7)."""
    now = now or datetime.now(tz=timezone.utc)
    grants_file = UserGrantsFile.load(default_grants_path())

    target: Optional[Grant] = None

    if args.grant_id:
        target = grants_file.get(args.grant_id)
        if target is None:
            raise PermsError(
                f"no grant with id {args.grant_id}; run "
                "`makakoo perms list` to see active grants"
            )
    elif args.path:
        path_lower = args.path.strip().lower()
        active = grants_file.active_grants(now)
        if path_lower in ("last", "latest"):
            if not active:
                raise PermsError("no active grants to revoke")
            # `created_at` is the tiebreaker; most recent wins.
            target = max(active, key=lambda g: g.created_at)
        else:
            abs_path = validate_and_expand_scope(args.path)
            prefix = f"fs/write:{abs_path}"
            candidates = [
                g
                for g in active
                if g.scope == prefix
                or g.scope == f"{prefix}/**"
                or g.scope == f"{prefix}**"
            ]
            if not candidates:
                raise PermsError(f"no grant matches path {abs_path}")
            if len(candidates) > 1:
                ids = ", ".join(g.id for g in candidates)
                raise PermsError(
                    f"ambiguous path — {len(candidates)} grants match: {ids}"
                )
            target = candidates[0]
    else:
        raise PermsError("provide either grant_id or path")

    scope_glob = target.scope[len("fs/write:"):] if target.scope.startswith(
        "fs/write:"
    ) else target.scope

    # v0.3.3 Phase A — ownership gate. A caller can only revoke grants
    # it owns unless it's an admin escape hatch (`cli`, `sancho-native`).
    # Prevents compromised-skill-A from wiping skill-B's active grants.
    if (
        args.plugin not in _ADMIN_REVOKE_BYPASS
        and target.owner != args.plugin
    ):
        _audit_revoke_denial(args, target.id, "reason:not_owner")
        raise PermsError(
            f"revoke refused: grant {target.id} is owned by "
            f"{target.owner!r}, not {args.plugin!r}. Only the creating "
            "plugin or an admin caller (cli) may revoke."
        )

    removed = grants_file.remove(target.id)
    if not removed:
        raise PermsError(
            f"race: grant {target.id} already removed; re-check `list`"
        )

    # Release one slot on the per-hour create bucket — revoke is an
    # explicit user intent to undo (purge is deliberately NOT a
    # decrement path; that would let slow-drip grants defeat the cap).
    rate_limit.decrement(now=now)

    log_audit(
        verb="perms/revoke",
        scope_requested=target.id,
        scope_granted=target.id,
        result="allowed",
        plugin=args.plugin,
    )
    return revoke_success_msg(target.id, scope_glob)


@dataclass
class ListArgs:
    include_expired: bool = False


def do_list_grants(
    args: ListArgs, *, now: Optional[datetime] = None
) -> str:
    """Return a short human-readable summary. Quotable verbatim.

    Callers that want structured data should inspect the store
    directly via `UserGrantsFile.load()`.
    """
    now = now or datetime.now(tz=timezone.utc)
    grants_file = UserGrantsFile.load(default_grants_path())

    # Baseline roots — pulled lazily to stay in sync with
    # effective_write_file_roots() in harvey_agent.py.
    try:
        from core.agent.harvey_agent import _baseline_write_file_roots
        baseline_roots = list(_baseline_write_file_roots())
    except Exception:
        baseline_roots = []

    active = grants_file.active_grants(now)
    today_midnight_utc = datetime(
        now.year, now.month, now.day, tzinfo=timezone.utc
    )
    expired_today = [
        g
        for g in grants_file.grants
        if g.expires_at is not None
        and today_midnight_utc <= g.expires_at < now
    ]

    if args.include_expired:
        # Include today's expired in the list — keeps the semantics
        # simple: still a summary, just wider window.
        all_shown = active + expired_today
        return list_summary_msg(baseline_roots, all_shown, 0)
    return list_summary_msg(baseline_roots, active, len(expired_today))
