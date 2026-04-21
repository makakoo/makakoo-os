# User Grants — File Format, Lock Protocol, and API

**Version:** 1.0-draft (stub landed in Phase B.1 of
`MAKAKOO-OS-V0.3-USER-GRANTS`; final draft lands at Gate G.2).

This document is the authoritative schema contract for
`$MAKAKOO_HOME/config/user_grants.json`. Python (`lib-harvey-core`)
and Rust (`makakoo-core`) both read AND write this file; the two
implementations MUST stay byte-for-byte compatible. Round-trip tests
at `tests/fixtures/grant_glob_vectors.json` lock glob semantics;
drift fails CI.

For the *why* of the user-grant layer, see
`spec/CAPABILITIES.md §1.11`. For the threat model, see
`spec/USER_GRANTS_THREAT_MODEL.md`. For the CLI-chat UX, see
§CLI-chat UX (populated in Phase E.10).

## 1. Purpose

User grants are the runtime-mutable Layer-3 of the three-layer
additive capability model. Sebastian manages them via
`makakoo perms {list,grant,revoke,purge,audit,show}` (CLI, Phase D)
or via the conversational MCP + `HARVEY_TOOLS` tools (Phase E).

A grant is **always additive** — Layer-1 baseline and Layer-2
manifest grants remain in effect. A grant never *removes* access.

## 2. File location

```
$MAKAKOO_HOME/config/user_grants.json         # data file
$MAKAKOO_HOME/config/user_grants.json.lock    # sidecar lock target (LD#9)
$MAKAKOO_HOME/state/perms_rate_limit.json     # separate counter (lope F7)
```

**Permissions:** `0o600` on both the data file and the rate-limit
counter. Lock file created on-demand, `0o600`.

**Gitignore:** grants are machine-local per LD#4 — never synced.

## 3. Schema v1

```json
{
  "version": 1,
  "grants": [
    {
      "id": "g_20260421_abcd1234",
      "scope": "fs/write:~/MAKAKOO/development/sprints/**",
      "created_at": "2026-04-21T09:30:00Z",
      "expires_at": "2026-04-21T10:30:00Z",
      "label": "sprint doc edits",
      "granted_by": "sebastian",
      "plugin": "harveychat-telegram",
      "origin_turn_id": "tg-msg-4823"
    }
  ]
}
```

### 3.1 Field-by-field

| Field | Type | Required | Semantics |
|---|---|---|---|
| `version` | int | yes | schema version; currently `1` |
| `grants` | array | yes | zero or more grant objects; order is insertion-order |
| `grants[].id` | string | yes | `g_<yyyymmdd>_<8hex>` — human-greppable + case-sensitive |
| `grants[].scope` | string | yes | `fs/write:<glob>` — §4 glob grammar; `~` and `$MAKAKOO_HOME` expanded at grant-time |
| `grants[].created_at` | ISO-8601 UTC | yes | grant creation instant; immutable after creation |
| `grants[].expires_at` | ISO-8601 UTC or `null` | yes | expiry instant; `null` = permanent |
| `grants[].label` | string | yes | free text; escaped + truncated to 80 chars before audit emit (LD#16) |
| `grants[].granted_by` | string | yes | literal `"sebastian"` in single-user installs; field reserved for multi-user future |
| `grants[].plugin` | string | yes | caller surface — one of `cli`, `claude-code`, `gemini-cli`, `codex`, `opencode`, `vibe`, `cursor`, `qwen`, `pi`, `harveychat`, `harveychat-telegram`, `sancho-native` |
| `grants[].origin_turn_id` | string | yes | host-provided turn identifier; enforcement-binding deferred to v0.3.1 per lope F6 |

**Explicitly NOT in the schema (lope F4):** `use_count`, `last_used_at`.
The audit log is the sole record of grant usage. `makakoo perms audit
--grant <id>` answers "was this grant used" without a per-write lock
storm.

### 3.2 ID scheme

`g_<yyyymmdd>_<8hex>` where:
- `g_` is the literal prefix
- `<yyyymmdd>` is UTC date at creation (not local time — avoids DST collisions)
- `<8hex>` is 4 random bytes, lowercase hex

ULID-ish but greppable (`grep g_20260421 logs/audit.jsonl` finds all
grants issued on that day). Collision window is cosmetic
(4-byte random × 1 day = 2^32 slots; grant-creation rate ≤ 50/hr).

## 4. Glob grammar

Matches `spec/CAPABILITIES.md §1.5` `fs/write:<glob>` exactly.

```
*         match any run of characters EXCEPT `/`   (single-segment wildcard)
**        match any run of characters INCLUDING `/` (descending wildcard)
exact     match the literal path
```

**Expansion:** `~` and `$MAKAKOO_HOME` expand at grant-time
(`makakoo perms grant`, `grant_write_access`), NOT at match-time.
This means a grant scoped as `fs/write:~/code/**` at 9:30am is
stored as `fs/write:/Users/sebastian/code/**` and remains that
literal string through lock-protocol writes, audit emits, and
later revokes — regardless of `HOME` or `$MAKAKOO_HOME` changes
between grant and revoke.

**Matching:** at check-time, the target write path is resolved
via `os.path.realpath()` (Python) / `fs::canonicalize()` (Rust),
then tested against each active grant's scope-glob.

**Test vectors:** `tests/fixtures/grant_glob_vectors.json` —
`(scope, test_path, expected_match)` triples exercised by both
Python and Rust loaders. Drift fails CI.

Examples:
| scope | test_path | match? |
|---|---|---|
| `fs/write:/Users/sebastian/code/**` | `/Users/sebastian/code/foo/bar.md` | ✅ |
| `fs/write:/Users/sebastian/code/**` | `/Users/sebastian/code` | ❌ (no segment after `/`) |
| `fs/write:/Users/sebastian/*` | `/Users/sebastian/file.txt` | ✅ |
| `fs/write:/Users/sebastian/*` | `/Users/sebastian/sub/file.txt` | ❌ (single-star stops at `/`) |
| `fs/write:/tmp/*.md` | `/tmp/foo.md` | ✅ |
| `fs/write:/tmp/*.md` | `/tmp/foo.txt` | ❌ |

## 5. Lock protocol (LD#9 — non-negotiable)

All writes to `user_grants.json` MUST acquire an exclusive lock on
the **sidecar** file `user_grants.json.lock`, NEVER on the data
file itself.

### 5.1 Why sidecar

Holding a lock on the data-file fd across `os.replace()` /
`fs::rename()` is a race hazard: the rename unlinks the original
inode, the new inode has no lock, and a concurrent reader sees the
new file before the lock semantics catch up. Separating the lock
target from the data target makes the rename atomic under the lock.

### 5.2 Protocol (Python)

```python
import fcntl
lock_path = grants_path.with_suffix(".json.lock")
with open(lock_path, "w") as lock_fd:
    fcntl.flock(lock_fd, fcntl.LOCK_EX)   # blocking exclusive
    # 1. read current data (may race if reader is also mid-parse; tolerated)
    # 2. mutate in memory
    # 3. write to tmp path
    tmp = grants_path.with_suffix(".json.tmp")
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=2)
        f.flush()
        os.fsync(f.fileno())
    # 4. atomic rename (POSIX guarantees inode-swap is atomic)
    os.replace(tmp, grants_path)
    # 5. lock released when lock_fd is closed (context-manager exit)
```

### 5.3 Protocol (Rust)

```rust
use fs2::FileExt;
let lock_path = grants_path.with_extension("json.lock");
let lock_fd = std::fs::OpenOptions::new()
    .create(true).write(true).open(&lock_path)?;
lock_fd.lock_exclusive()?;          // blocking
// 1. read, 2. mutate, 3. write tmp, 4. rename, 5. unlock (Drop)
let tmp = grants_path.with_extension("json.tmp");
std::fs::write(&tmp, serde_json::to_vec_pretty(&obj)?)?;
std::fs::rename(&tmp, &grants_path)?;
drop(lock_fd);                       // explicit or Drop
```

### 5.4 Reads are lock-free

Readers `fs::read_to_string()` the data file directly. Stale reads
between a purge and the next load are tolerated — the SANCHO
`perms_purge_tick` interval (15 min, Phase F) bounds staleness.

### 5.5 Platform support

Lock protocol is Linux + macOS. Windows is a non-target OS for v0.3
— the file will load read-only there (no grants can be created from
Windows). Reason: `fcntl.flock` is absent on Windows, and `fs2`'s
Windows path uses `LockFile` which has different semantics around
process exit.

## 6. Corrupt-file tolerance

The loader MUST tolerate:

- **Missing file** — returns empty grants, does NOT auto-create.
  Startup log: `tracing::info!("loaded 0 user grants (no file)")`.
- **Invalid JSON** — returns empty grants, logs
  `tracing::warn!("corrupt user_grants.json; falling back to empty")`.
  Does NOT auto-repair the file. User runs
  `rm $MAKAKOO_HOME/config/user_grants.json` to reset.
- **Unknown `version`** — logs warning, attempts best-effort parse.
  A future v2 loader SHOULD be able to read v1 files.
- **Missing required field** — drops the offending entry, keeps the
  rest, logs a warning naming the index.

## 7. Rate limits (LD#14 · lope F7)

| Limit | Value | Behavior on exceed |
|---|---|---|
| Active grants (system-wide) | `20` | `add()` returns `RateLimitExceeded("N active grants; revoke some or wait")` |
| Grant creates per rolling 60-min | `50` | Same error shape; rolling window starts at creation time of the 51st create |

**Counter file:** `$MAKAKOO_HOME/state/perms_rate_limit.json` —
separate from the grant store so a corrupt counter doesn't poison the
grants (lope F7). Schema:

```json
{
  "window_start": "2026-04-21T09:30:00Z",
  "creates_in_window": 12
}
```

When `creates_in_window < 50` AND `active_grant_count() < 20`, the
counter increments. When `Utc::now() - window_start > 60min`, the
window resets.

## 8. Audit events

Every grant creation, grant deletion, SANCHO purge expiry, and
enforcement decision writes one line to
`$MAKAKOO_HOME/logs/audit.jsonl` matching the `AuditEntry` schema
in `spec/CAPABILITIES.md §3`. Verbs used:

| Verb | Emitted when |
|---|---|
| `perms/grant` | Sebastian creates a grant (CLI or conversational) |
| `perms/revoke` | Sebastian revokes a grant OR SANCHO purges an expired one (`reason="expired"`) |
| `fs/write` | `_resolve_write_path` makes a decision; `scope_granted` = matched grant id or `null` |

## 9. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| Write rejected despite active grant | Path resolved via realpath differs from the stored scope (e.g., grant scope expanded from `~` at grant-time, path expanded from a different `HOME` at check-time) | Re-grant with absolute path |
| Grant created but doesn't match any write | Glob semantics — single-star stops at `/`, use `**` for descending | See §4 glob grammar |
| All grants silently disappear | `user_grants.json` was corrupted and loader fell back to empty | Restore from git? No — LD#4: file is never git-synced. Check for a stale `.tmp` sibling and rename it back, or re-create grants via `makakoo perms grant` |
| Rate-limit error at unexpected times | Counter file corrupt or stuck at a stale window | `rm $MAKAKOO_HOME/state/perms_rate_limit.json` — starts fresh |

## 10. CLI reference

*Stub — populated in Phase G.2 as Phase D lands the Rust CLI implementation.*

## 11. MCP + HARVEY_TOOLS reference

*Stub — populated in Phase G.2 as Phase E lands the conversational tools.*

## 12. CLI-chat UX

*Stub — populated in Phase E.10 (canonical conversational flows from SPRINT.md §12).*

## 13. Writers

Two writers with no reader/writer asymmetry (LD#9):

1. **Python** — `UserGrantsFile` in
   `plugins-core/lib-harvey-core/src/core/capability/user_grants.py`.
   Callers: HarveyChat agent loop via `HARVEY_TOOLS`, the agent
   enforcement path in `harvey_agent.py::_resolve_write_path`.
2. **Rust** — `UserGrants` in
   `makakoo-core/src/capability/user_grants.rs`.
   Callers: `makakoo perms` CLI handlers, `makakoo-mcp` tier-b
   permission handlers, `perms_purge_tick` SANCHO native task.

Both MUST use the same sidecar-lock protocol (§5). The shared fixture
at `tests/fixtures/grant_glob_vectors.json` is exercised by both
implementations to catch glob-semantic drift.

## 14. Revision log

- **v0.1 (stub)** — 2026-04-21. Landed in Phase B.1 of
  `MAKAKOO-OS-V0.3-USER-GRANTS`. Schema locked, lock protocol
  locked, glob grammar locked. CLI/MCP/UX sections stubbed for
  later phases.
