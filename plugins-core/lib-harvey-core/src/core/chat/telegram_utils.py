"""
telegram_utils.py — Fresh-install helpers for the HarveyChat Telegram channel.

Everything a new user needs to get Telegram working on a clean box lives
here as composable, testable functions:

  normalize_chat_id(raw)        — add the -100 supergroup prefix that the
                                  Bot API requires (Fix 1 from the 5-fixes
                                  log)
  get_bot_info(token)           — call Telegram's getMe to validate a token
                                  and fetch the bot's numeric id + username
  fetch_recent_updates(token)   — call Telegram's getUpdates and distil it
                                  into a list of seen users/chats, used
                                  by the `detect` CLI command
  claude_plugin_access_path()   — path to ~/.claude/channels/telegram/access.json
                                  (the Claude Code Telegram plugin's
                                  allowlist) if the plugin is installed
  sync_claude_plugin_access(...)— idempotently adds a chat id + user ids
                                  to the Claude plugin's access.json
                                  with requireMention=False so both bots
                                  see the same messages (Fix 2 + Fix 5)
  diagnose(cfg)                 — return a structured health report for
                                  the CLI `doctor` command: tilde
                                  expansion, bot token reachability,
                                  allowlist shape, Claude plugin sync,
                                  bot-filter flag

None of these functions depend on `python-telegram-bot` — they use the
stdlib + `requests` (already a HarveyChat dependency). That lets the
test suite run without the full telegram library installed.
"""

from __future__ import annotations

import json
import logging
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

log = logging.getLogger("harveychat.telegram_utils")

TELEGRAM_API_BASE = "https://api.telegram.org"


# ── Chat-id normalization ───────────────────────────────────────────


def normalize_chat_id(raw: Any) -> int:
    """
    Convert any form of Telegram chat id into the form the Bot API expects.

    Rules (distilled from the 5-fixes incident):
      - Supergroups: peer id `3746642416` → Bot API needs `-1003746642416`.
        If the caller gave us a positive 10-digit number, prepend `-100`.
      - Channels: same rule as supergroups. The `-100` prefix is the
        MTProto→Bot API translation.
      - Already-normalized supergroup IDs (start with `-100`) pass through.
      - Regular groups are negative numbers like `-12345` — pass through.
      - User IDs (DMs) are small positive numbers — pass through.

    Heuristic for deciding whether to add the prefix: treat a bare positive
    integer with >= 9 digits as a supergroup peer id. That's the shape
    Telegram clients show when you open group info.
    """
    if raw is None:
        raise ValueError("normalize_chat_id: got None")

    # Accept string digits too
    if isinstance(raw, str):
        stripped = raw.strip()
        if not stripped:
            raise ValueError("normalize_chat_id: empty string")
        try:
            raw = int(stripped)
        except ValueError as e:
            raise ValueError(
                f"normalize_chat_id: not an integer: {stripped!r}"
            ) from e

    if not isinstance(raw, int):
        raise TypeError(f"normalize_chat_id: expected int/str, got {type(raw)}")

    # Already normalized: any negative number is a group/channel/supergroup
    if raw < 0:
        return raw

    # Positive: either a user id (small) or a bare supergroup peer id (big)
    # Supergroup peer ids are typically 9-11 digits. User ids are usually
    # 8-10 digits too, so size alone isn't perfect — we conservatively
    # promote to supergroup only when the positive number is >= 1_000_000_000
    # (10 digits), matching what Telegram shows for modern supergroups.
    if raw >= 1_000_000_000:
        return int(f"-100{raw}")

    # Small positive: user DM id, leave alone
    return raw


def is_group_id(chat_id: int) -> bool:
    """True if the chat id represents a group, supergroup, or channel."""
    return chat_id < 0


def is_supergroup_id(chat_id: int) -> bool:
    """True if the chat id is a supergroup/channel in Bot API form."""
    return chat_id < 0 and str(chat_id).startswith("-100")


# ── Telegram Bot API calls (synchronous, use `requests`) ────────────


def get_bot_info(token: str, timeout: float = 10.0) -> Tuple[bool, Dict[str, Any]]:
    """
    Call getMe. Returns (ok, info_dict). `info_dict` has keys:
      id, first_name, username — when ok is True, else "error" key.

    Never raises on network issues; returns (False, {"error": "..."}).
    """
    try:
        import requests
    except ImportError:
        return False, {"error": "requests library not installed"}

    if not token or ":" not in token:
        return False, {"error": "invalid token format"}

    try:
        r = requests.get(
            f"{TELEGRAM_API_BASE}/bot{token}/getMe",
            timeout=timeout,
        )
    except Exception as e:
        return False, {"error": f"network: {e}"}

    if r.status_code != 200:
        return False, {"error": f"HTTP {r.status_code}"}

    data = r.json() if r.content else {}
    if not data.get("ok"):
        return False, {"error": data.get("description", "unknown")}

    result = data.get("result") or {}
    return True, {
        "id": result.get("id"),
        "first_name": result.get("first_name", ""),
        "username": result.get("username", ""),
        "can_join_groups": result.get("can_join_groups", False),
        "can_read_all_group_messages": result.get("can_read_all_group_messages", False),
    }


def fetch_recent_updates(
    token: str,
    limit: int = 20,
    timeout: float = 10.0,
) -> List[Dict[str, Any]]:
    """
    Call getUpdates, distil into a flat list of seen chats/users.

    Returns a list of dicts:
      {
        "update_id": int,
        "chat_id": int,         (already normalized to Bot API form)
        "chat_type": "private" | "group" | "supergroup" | "channel",
        "chat_title": str,
        "user_id": int,
        "username": str,
        "text": str,
        "is_bot": bool,
      }

    Used by the `detect` CLI command: user sends a message, we pull
    recent updates and show every unique (user, chat) pair so they can
    copy the IDs into `add-chat`. Never raises; returns [] on error.
    """
    try:
        import requests
    except ImportError:
        return []

    if not token:
        return []

    try:
        r = requests.get(
            f"{TELEGRAM_API_BASE}/bot{token}/getUpdates",
            params={"limit": limit, "timeout": 0},
            timeout=timeout,
        )
    except Exception as e:
        log.warning(f"getUpdates failed: {e}")
        return []

    if r.status_code != 200:
        return []

    data = r.json() if r.content else {}
    if not data.get("ok"):
        return []

    out: List[Dict[str, Any]] = []
    for update in data.get("result", []):
        msg = update.get("message") or update.get("edited_message") or {}
        chat = msg.get("chat") or {}
        user = msg.get("from") or {}
        if not chat or not user:
            continue
        # Telegram already returns normalized chat_id for supergroups
        # (prefixed with -100), so we just pass it through.
        out.append({
            "update_id": update.get("update_id"),
            "chat_id": chat.get("id"),
            "chat_type": chat.get("type", ""),
            "chat_title": chat.get("title", "") or chat.get("first_name", ""),
            "user_id": user.get("id"),
            "username": user.get("username", "") or user.get("first_name", ""),
            "text": (msg.get("text") or "")[:200],
            "is_bot": bool(user.get("is_bot", False)),
        })
    return out


# ── Claude Code Telegram plugin integration ────────────────────────


def claude_plugin_access_path() -> Path:
    """Path to the Claude Code Telegram plugin's access.json allowlist."""
    return Path.home() / ".claude" / "channels" / "telegram" / "access.json"


def claude_plugin_installed() -> bool:
    """Is the Claude Code Telegram plugin present on this machine?"""
    return claude_plugin_access_path().parent.exists()


def load_claude_plugin_access() -> Dict[str, Any]:
    """
    Load the Claude plugin's access.json if it exists, otherwise return a
    fresh template. Never raises.
    """
    path = claude_plugin_access_path()
    if path.exists():
        try:
            return json.loads(path.read_text())
        except Exception as e:
            log.warning(f"could not parse {path}: {e}")
    return {
        "dmPolicy": "pairing",
        "allowFrom": [],
        "groups": {},
        "pending": {},
    }


def save_claude_plugin_access(data: Dict[str, Any]) -> Path:
    """Write the Claude plugin's access.json (creates parent dir if needed)."""
    path = claude_plugin_access_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2))
    return path


def sync_claude_plugin_access(
    chat_id: int,
    user_ids: Optional[List[int]] = None,
    require_mention: bool = False,
    dm_user_ids: Optional[List[int]] = None,
) -> Tuple[bool, str]:
    """
    Idempotently add a chat id + user ids to the Claude Code Telegram
    plugin's access.json.

    Encodes every lesson from the 5-fixes log:
      - chat_id is normalized via `normalize_chat_id()` before write,
        so callers can pass either the bare peer id or the -100 form
      - `require_mention=False` by default so ClaudeSeba sees every
        group message, not just @-mentions (Fix 5)
      - `dm_user_ids` are added to the top-level `allowFrom` list so
        direct messages from those users are accepted too

    Returns (ok, message) where `message` is a human-readable status line.
    No-ops gracefully if the plugin isn't installed.
    """
    if not claude_plugin_installed():
        return False, (
            f"Claude plugin not installed at "
            f"{claude_plugin_access_path().parent} — skipped"
        )

    try:
        normalized = normalize_chat_id(chat_id)
    except Exception as e:
        return False, f"could not normalize chat_id: {e}"

    data = load_claude_plugin_access()
    data.setdefault("groups", {})
    data.setdefault("allowFrom", [])

    key = str(normalized)
    existing = data["groups"].get(key) or {}
    allow_from = list({
        str(u) for u in (existing.get("allowFrom") or []) + (user_ids or [])
    })
    data["groups"][key] = {
        "requireMention": bool(require_mention),
        "allowFrom": allow_from,
    }

    if dm_user_ids:
        dm_set = {str(u) for u in (data["allowFrom"] or [])}
        dm_set.update(str(u) for u in dm_user_ids)
        data["allowFrom"] = sorted(dm_set)

    path = save_claude_plugin_access(data)
    return True, (
        f"synced {normalized} → {path} "
        f"(requireMention={require_mention}, "
        f"allowFrom={len(allow_from)}, dm_allowFrom={len(data['allowFrom'])})"
    )


# ── Doctor / diagnostics ────────────────────────────────────────────


@dataclass
class DiagnosticResult:
    name: str
    ok: bool
    detail: str = ""


@dataclass
class DiagnosticReport:
    results: List[DiagnosticResult] = field(default_factory=list)

    def add(self, name: str, ok: bool, detail: str = "") -> None:
        self.results.append(DiagnosticResult(name=name, ok=ok, detail=detail))

    def all_ok(self) -> bool:
        return all(r.ok for r in self.results)

    def failed(self) -> List[DiagnosticResult]:
        return [r for r in self.results if not r.ok]

    def to_dict(self) -> Dict[str, Any]:
        return {
            "all_ok": self.all_ok(),
            "pass_count": sum(1 for r in self.results if r.ok),
            "fail_count": sum(1 for r in self.results if not r.ok),
            "results": [
                {"name": r.name, "ok": r.ok, "detail": r.detail}
                for r in self.results
            ],
        }


def diagnose(
    telegram_cfg: Any,
    harvey_home: Optional[str] = None,
    check_network: bool = True,
) -> DiagnosticReport:
    """
    Build a structured diagnostic report for the current Telegram setup.

    Passing `check_network=False` skips the Telegram API calls — useful
    for tests and offline runs.

    Checks:
      1. HARVEY_HOME env var is tilde-expanded (Fix 3)
      2. Telegram bot token is present
      3. Telegram bot token is reachable (getMe succeeds) — if check_network
      4. At least one of allowed_user_ids / allowed_chat_ids is populated
         (otherwise anyone can message the bot)
      5. All allowed_chat_ids are normalized (Fix 1)
      6. ignore_bots is True (Fix 4)
      7. Claude Code plugin allowlist exists and mentions the same chat_ids
         (Fix 2 + Fix 5)
    """
    report = DiagnosticReport()

    # 1. HARVEY_HOME expansion
    raw_home = harvey_home or os.environ.get("HARVEY_HOME", "")
    if raw_home:
        expanded = os.path.expanduser(raw_home)
        if "~" in raw_home and expanded == raw_home:
            report.add(
                "harvey_home_expansion",
                ok=False,
                detail=(
                    f"HARVEY_HOME='{raw_home}' contains ~ but was not "
                    f"expanded by os.path.expanduser — check shell env"
                ),
            )
        else:
            report.add(
                "harvey_home_expansion",
                ok=True,
                detail=f"resolved to {expanded}",
            )
    else:
        report.add(
            "harvey_home_expansion",
            ok=True,
            detail="HARVEY_HOME not set — using default",
        )

    # 2. Bot token present
    token = getattr(telegram_cfg, "bot_token", "") or ""
    report.add(
        "bot_token_present",
        ok=bool(token),
        detail="configured" if token else "missing — run `setup`",
    )

    # 3. Bot token reachable
    if token and check_network:
        ok, info = get_bot_info(token)
        if ok:
            report.add(
                "bot_token_reachable",
                ok=True,
                detail=f"@{info.get('username', '?')} (id={info.get('id')})",
            )
        else:
            report.add(
                "bot_token_reachable",
                ok=False,
                detail=f"getMe failed: {info.get('error', 'unknown')}",
            )

    # 4. Allowlist shape
    user_ids = list(getattr(telegram_cfg, "allowed_user_ids", []) or [])
    chat_ids = list(getattr(telegram_cfg, "allowed_chat_ids", []) or [])
    if not user_ids and not chat_ids:
        report.add(
            "allowlist_populated",
            ok=False,
            detail="both lists empty — anyone can message the bot",
        )
    else:
        report.add(
            "allowlist_populated",
            ok=True,
            detail=f"users={len(user_ids)}, chats={len(chat_ids)}",
        )

    # 5. Chat ids normalized
    unnormalized = [
        c for c in chat_ids
        if isinstance(c, int) and c > 0 and c >= 1_000_000_000
    ]
    report.add(
        "chat_ids_normalized",
        ok=not unnormalized,
        detail=(
            f"bare supergroup peer ids detected: {unnormalized} — "
            f"should be prefixed with -100"
            if unnormalized
            else f"all {len(chat_ids)} chat ids properly formatted"
        ),
    )

    # 6. ignore_bots flag
    ignore_bots = bool(getattr(telegram_cfg, "ignore_bots", True))
    report.add(
        "ignore_bots_enabled",
        ok=ignore_bots,
        detail=(
            "on — bot-to-bot messages dropped silently"
            if ignore_bots
            else "OFF — Olibia may respond to other bots (Fix 4)"
        ),
    )

    # 7. Claude plugin sync
    if claude_plugin_installed():
        plugin = load_claude_plugin_access()
        plugin_groups = set(plugin.get("groups", {}).keys())
        expected = {str(c) for c in chat_ids}
        missing = expected - plugin_groups
        if not chat_ids:
            report.add(
                "claude_plugin_sync",
                ok=True,
                detail="no chats to sync",
            )
        elif missing:
            report.add(
                "claude_plugin_sync",
                ok=False,
                detail=(
                    f"{len(missing)} chat id(s) missing from Claude "
                    f"plugin access.json: {sorted(missing)} — "
                    f"run `add-chat` or `sync-claude`"
                ),
            )
        else:
            # Check requireMention setting
            bad_mention = [
                k for k, v in plugin.get("groups", {}).items()
                if v.get("requireMention") is True
            ]
            report.add(
                "claude_plugin_sync",
                ok=not bad_mention,
                detail=(
                    f"all chats synced; requireMention=true on: "
                    f"{bad_mention} (Fix 5)"
                    if bad_mention
                    else f"all {len(expected)} chats synced"
                ),
            )
    else:
        report.add(
            "claude_plugin_installed",
            ok=True,
            detail="Claude Code Telegram plugin not detected — skipped",
        )

    return report


__all__ = [
    "TELEGRAM_API_BASE",
    "normalize_chat_id",
    "is_group_id",
    "is_supergroup_id",
    "get_bot_info",
    "fetch_recent_updates",
    "claude_plugin_access_path",
    "claude_plugin_installed",
    "load_claude_plugin_access",
    "save_claude_plugin_access",
    "sync_claude_plugin_access",
    "DiagnosticResult",
    "DiagnosticReport",
    "diagnose",
]
