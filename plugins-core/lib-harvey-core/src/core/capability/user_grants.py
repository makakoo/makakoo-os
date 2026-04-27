"""
UserGrantsFile — Python writer + reader for user_grants.json.

Pairs with Rust `makakoo-core::capability::user_grants::UserGrants`.
Both implementations MUST stay schema-compatible — see
`spec/USER_GRANTS.md §3` for the locked field set and §5 for the
sidecar-lock protocol.

Key design decisions (frozen in SPRINT.md §3):

* LD#9 — sidecar lock at `user_grants.json.lock`, NEVER on the
  data fd; released AFTER `os.replace()` completes.
* LD#4 — file is machine-local, gitignored, never synced.
* Lope F4 — no `use_count` / `last_used_at` on the schema and no
  `record_use()` method anywhere. The audit log answers "was this
  grant used".
* Lope F6 — `origin_turn_id` is stored but not enforcement-bound
  until v0.3.1.
* Lope F7 — rate-limit counter lives in a separate file
  (`state/perms_rate_limit.json`) so a corrupt counter can't poison
  the grants.

Windows is a non-target OS for v0.3 (fcntl is POSIX-only). On
Windows this module degrades to read-only.
"""

from __future__ import annotations

import dataclasses
import fcntl
import json
import logging
import os
import re
import secrets as _secrets
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable

from core.capability import rate_limit

log = logging.getLogger("core.capability.user_grants")

SCHEMA_VERSION = 1


# ═══════════════════════════════════════════════════════════════
#  Grant model
# ═══════════════════════════════════════════════════════════════


@dataclass
class Grant:
    """One runtime user grant. Field set matches spec/USER_GRANTS.md §3.1."""

    id: str
    scope: str  # e.g. "fs/write:/Users/sebastian/code/**"
    created_at: datetime
    expires_at: datetime | None
    label: str
    granted_by: str = "sebastian"
    plugin: str = "cli"
    origin_turn_id: str = ""
    # v0.3.3 — `owner` captures the plugin string that created the
    # grant. `do_revoke` refuses unless the caller's plugin matches
    # OR the caller is `cli` / `sancho-native` (admin escape hatches).
    # Defaults to "cli" on the dataclass; `add()` backfills from
    # `plugin` when omitted; `from_dict` falls back to `plugin` for
    # backward-compatible reads of pre-v0.3.3 records.
    owner: str = "cli"

    # ── deserialize ────────────────────────────────────────────
    @classmethod
    def from_dict(cls, raw: dict) -> Grant:
        created = _parse_iso8601(raw["created_at"])
        expires_raw = raw.get("expires_at")
        expires = _parse_iso8601(expires_raw) if expires_raw else None
        plugin = str(raw.get("plugin", "cli"))
        return cls(
            id=str(raw["id"]),
            scope=str(raw["scope"]),
            created_at=created,
            expires_at=expires,
            label=str(raw.get("label", "")),
            granted_by=str(raw.get("granted_by", "sebastian")),
            plugin=plugin,
            origin_turn_id=str(raw.get("origin_turn_id", "")),
            # Backward compat: pre-v0.3.3 records have no `owner` field
            # — attribute ownership to whatever `plugin` claimed.
            owner=str(raw.get("owner", plugin)),
        )

    # ── serialize ──────────────────────────────────────────────
    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "scope": self.scope,
            "created_at": _iso8601(self.created_at),
            "expires_at": _iso8601(self.expires_at) if self.expires_at else None,
            "label": self.label,
            "granted_by": self.granted_by,
            "plugin": self.plugin,
            "origin_turn_id": self.origin_turn_id,
            "owner": self.owner,
        }

    # ── predicates ─────────────────────────────────────────────
    def is_expired(self, now: datetime | None = None) -> bool:
        if self.expires_at is None:
            return False
        return self.expires_at <= (now or _now_utc())

    def matches_path(self, abs_path: str) -> bool:
        """Match `abs_path` against this grant's scope glob.

        Only `fs/write:<glob>` scopes are checked; other verbs return
        False because v0.3 only gates write.
        """
        if not self.scope.startswith("fs/write:"):
            return False
        glob = self.scope[len("fs/write:") :]
        return _glob_match(glob, abs_path)


# ═══════════════════════════════════════════════════════════════
#  Glob matcher (mirrors Rust semantics in capability/verb.rs)
# ═══════════════════════════════════════════════════════════════


def _glob_to_regex(pattern: str) -> re.Pattern[str]:
    """Translate glob grammar to an anchored regex.

    `**` — match any run of characters INCLUDING `/` (descending)
    `*`  — match any run of characters EXCEPT `/` (single-segment)
    Other regex metacharacters are escaped literally.
    """
    out = ["^"]
    i = 0
    n = len(pattern)
    while i < n:
        c = pattern[i]
        if c == "*":
            # look ahead for second star
            if i + 1 < n and pattern[i + 1] == "*":
                out.append(".*")
                i += 2
            else:
                out.append("[^/]*")
                i += 1
        elif c in r".+()|^$\{}[]?":
            out.append("\\" + c)
            i += 1
        else:
            out.append(c)
            i += 1
    out.append("$")
    return re.compile("".join(out))


def _glob_match(pattern: str, path: str) -> bool:
    if not path:
        return False
    try:
        return bool(_glob_to_regex(pattern).match(path))
    except re.error:
        log.warning("invalid glob pattern %r — treating as no-match", pattern)
        return False


# ═══════════════════════════════════════════════════════════════
#  File paths + time helpers
# ═══════════════════════════════════════════════════════════════


def default_grants_path(home: Path | None = None) -> Path:
    """Canonical path: $MAKAKOO_HOME/config/user_grants.json."""
    if home is not None:
        return Path(home) / "config" / "user_grants.json"
    base = (
        os.environ.get("MAKAKOO_HOME")
        or os.environ.get("HARVEY_HOME")
        or os.path.expanduser("~/MAKAKOO")
    )
    return Path(base) / "config" / "user_grants.json"


def _now_utc() -> datetime:
    return datetime.now(tz=timezone.utc)


def _parse_iso8601(s: str | None) -> datetime | None:
    if s is None:
        return None
    # Python < 3.11 doesn't parse trailing "Z" — replace with +00:00
    s2 = s.replace("Z", "+00:00") if isinstance(s, str) else s
    dt = datetime.fromisoformat(s2)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt


def _iso8601(dt: datetime) -> str:
    # Always emit UTC with trailing Z for cross-language stability.
    dt_utc = dt.astimezone(timezone.utc)
    return dt_utc.isoformat().replace("+00:00", "Z")


def new_grant_id(now: datetime | None = None) -> str:
    """`g_<yyyymmdd>_<8hex>` — see spec/USER_GRANTS.md §3.2."""
    if now is None:
        now = _now_utc()
    date = now.astimezone(timezone.utc).strftime("%Y%m%d")
    rand = _secrets.token_hex(4)
    return f"g_{date}_{rand}"


def escape_audit_field(s: str | None, max_len: int = 80) -> str:
    """Strip control chars / ANSI / newlines; truncate to max_len.

    Used by both the audit client and `makakoo perms list` rendering
    per LD#16. Rust mirror lives at
    `makakoo-core/src/capability/audit_escape.rs`; the shared
    test-vector fixture is at
    `tests/fixtures/audit_escape_vectors.json` (Phase B.5).
    """
    if not s:
        return ""
    # Drop C0 controls, DEL, and C1 controls; allow printable + space.
    # Whitespace-ish C0 chars (\n \r \t \v \f) become a literal space so
    # later split/join collapses wordbreaks — otherwise "hello\nworld"
    # would fuse into "helloworld".
    cleaned_chars = []
    for ch in s:
        cp = ord(ch)
        if cp == 0x7F:
            continue
        if cp < 0x20:
            if ch in "\n\r\t\v\f":
                cleaned_chars.append(" ")
            # Other C0 controls silently dropped.
            continue
        if 0x80 <= cp <= 0x9F:
            continue
        cleaned_chars.append(ch)
    cleaned = "".join(cleaned_chars)
    # Collapse whitespace runs to single spaces.
    cleaned = " ".join(cleaned.split())
    if len(cleaned) > max_len:
        cleaned = cleaned[: max_len - 1] + "…"
    return cleaned


# ═══════════════════════════════════════════════════════════════
#  UserGrantsFile
# ═══════════════════════════════════════════════════════════════


@dataclass
class UserGrantsFile:
    """Typed handle to `$MAKAKOO_HOME/config/user_grants.json`.

    Reads are lock-free (§5.4). Writes MUST go through `_with_lock()`
    — a helper that opens the sidecar lock file, takes
    `fcntl.LOCK_EX`, runs the mutation, atomic-renames the data
    file, and releases the lock.
    """

    path: Path
    version: int = SCHEMA_VERSION
    grants: list[Grant] = field(default_factory=list)

    # ── public constructors ────────────────────────────────────
    @classmethod
    def load(cls, path: Path | None = None) -> UserGrantsFile:
        """Load from disk. Missing or corrupt file → empty grants."""
        p = path if path is not None else default_grants_path()
        if not p.exists():
            log.info("loaded 0 user grants (no file at %s)", p)
            return cls(path=p, version=SCHEMA_VERSION, grants=[])

        try:
            raw = json.loads(p.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            log.warning(
                "corrupt user_grants.json at %s; falling back to empty (%s)",
                p,
                e,
            )
            return cls(path=p, version=SCHEMA_VERSION, grants=[])

        if not isinstance(raw, dict):
            log.warning("user_grants.json at %s is not a JSON object; empty", p)
            return cls(path=p, version=SCHEMA_VERSION, grants=[])

        version = int(raw.get("version", SCHEMA_VERSION))
        if version != SCHEMA_VERSION:
            log.warning(
                "user_grants.json version=%s (this loader expects %s); "
                "best-effort parse",
                version,
                SCHEMA_VERSION,
            )

        grants: list[Grant] = []
        for i, entry in enumerate(raw.get("grants", []) or []):
            try:
                grants.append(Grant.from_dict(entry))
            except (KeyError, ValueError, TypeError) as e:
                log.warning(
                    "dropping malformed grant entry at index %d in %s (%s)",
                    i,
                    p,
                    e,
                )
        log.info("loaded %d user grants from %s", len(grants), p)
        return cls(path=p, version=version, grants=grants)

    # ── predicates / accessors ─────────────────────────────────
    def active_grants(self, now: datetime | None = None) -> list[Grant]:
        """Return grants that have not yet expired."""
        n = now or _now_utc()
        return [g for g in self.grants if not g.is_expired(n)]

    def match_write_path(
        self,
        abs_path: str,
        plugin: str | None = None,
        now: datetime | None = None,
    ) -> Grant | None:
        """Return the first active grant whose glob matches, else None.

        `plugin` is accepted for forward-compatibility with v0.4
        per-plugin scoping but is NOT enforced in v0.3.
        """
        for g in self.active_grants(now):
            if g.matches_path(abs_path):
                return g
        return None

    def get(self, grant_id: str) -> Grant | None:
        for g in self.grants:
            if g.id == grant_id:
                return g
        return None

    # ── mutations (sidecar-locked, atomic rename) ──────────────
    def add(
        self,
        scope: str,
        expires_at: datetime | None,
        label: str,
        plugin: str,
        origin_turn_id: str,
        granted_by: str = "sebastian",
        owner: str | None = None,
        now: datetime | None = None,
    ) -> Grant:
        """Append one grant, sidecar-locked + atomic rename.

        Checks the global rate limit BEFORE mutating; on overflow
        raises RateLimitExceeded without writing anything.

        `owner` defaults to `plugin` — whoever created the grant owns
        it (v0.3.3). Admin callers (`cli`, `sancho-native`) get a
        bypass at revoke time, not at create time.
        """
        n = now or _now_utc()
        # Rate-limit check uses the current on-disk active count — NOT
        # cached — so a stale in-memory instance can't bypass the cap.
        active_count = len(self.active_grants(n))
        rate_limit.check_and_increment(active_count, now=n)

        grant = Grant(
            id=new_grant_id(n),
            scope=scope,
            created_at=n,
            expires_at=expires_at,
            label=escape_audit_field(label, max_len=80),
            granted_by=granted_by,
            plugin=plugin,
            origin_turn_id=origin_turn_id,
            owner=owner if owner is not None else plugin,
        )
        with self._lock():
            # Re-read under lock so we don't clobber a concurrent writer.
            latest = UserGrantsFile.load(self.path)
            latest.grants.append(grant)
            self.grants = latest.grants
            self._write_unlocked()
        return grant

    def remove(self, grant_id: str) -> bool:
        """Delete a grant by id. Returns True if removed, False if absent."""
        with self._lock():
            latest = UserGrantsFile.load(self.path)
            before = len(latest.grants)
            latest.grants = [g for g in latest.grants if g.id != grant_id]
            removed = before != len(latest.grants)
            if removed:
                self.grants = latest.grants
                self._write_unlocked()
            else:
                self.grants = latest.grants
        return removed

    def purge_expired(self, now: datetime | None = None) -> list[Grant]:
        """Drop expired grants. Returns the removed list (for audit)."""
        n = now or _now_utc()
        with self._lock():
            latest = UserGrantsFile.load(self.path)
            kept: list[Grant] = []
            removed: list[Grant] = []
            for g in latest.grants:
                if g.is_expired(n):
                    removed.append(g)
                else:
                    kept.append(g)
            if removed:
                latest.grants = kept
                self.grants = kept
                self._write_unlocked()
            else:
                self.grants = latest.grants
        return removed

    # ── lock + write helpers ──────────────────────────────────
    def _lock(self):
        """Context manager: exclusive flock on the sidecar lock file.

        Never locks the data fd (LD#9). Released after context exit;
        caller is expected to rename while the lock is still held.
        """
        lock_path = self.path.with_suffix(".json.lock")
        lock_path.parent.mkdir(parents=True, exist_ok=True)
        return _SidecarLock(lock_path)

    def _write_unlocked(self) -> None:
        """Atomic temp-rename write. MUST be called under self._lock()."""
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".json.tmp")
        payload = {
            "version": self.version,
            "grants": [g.to_dict() for g in self.grants],
        }
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(payload, f, indent=2)
            f.flush()
            os.fsync(f.fileno())
        try:
            os.chmod(tmp, 0o600)
        except OSError as e:
            log.warning("chmod 600 failed on %s: %s", tmp, e)
        os.replace(tmp, self.path)


class _SidecarLock:
    """Context manager for `fcntl.flock(LOCK_EX)` on a sidecar path.

    Opens + keeps the fd for the lock lifetime; releases on exit.
    """

    def __init__(self, lock_path: Path):
        self.lock_path = lock_path
        self._fd: int | None = None

    def __enter__(self) -> _SidecarLock:
        # `w` so the file is created if absent; we don't actually write
        # to it — the lock bit is the semantic payload.
        fd = os.open(
            str(self.lock_path),
            os.O_WRONLY | os.O_CREAT,
            0o600,
        )
        fcntl.flock(fd, fcntl.LOCK_EX)
        self._fd = fd
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self._fd is not None:
            try:
                fcntl.flock(self._fd, fcntl.LOCK_UN)
            finally:
                os.close(self._fd)
                self._fd = None
