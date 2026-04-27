# Plugin Update Workflow

v0.4 added a real upgrade loop for git-sourced plugins — dry-run probe, diff the manifest, prompt on capability drift, apply. Plus an opt-in nightly cron that reminds Sebastian when anything has drifted.

## Three commands, one mental model

| Command | Disk state mutated | Network | Interactive |
|---|---|---|---|
| `makakoo plugin outdated [--json]` | no | yes | no |
| `makakoo plugin update <name> [--yes]` | yes if drift + accepted | yes | yes (unless `--yes`) |
| `makakoo plugin update --all [--yes]` | yes per-plugin | yes | yes (unless `--yes`) |

## `plugin outdated` — pure dry-run

Walks `plugins.lock`, re-fetches each git/tar entry, diffs against the locked SHA, prints a table:

```
┌────────────────────────┬──────────┬──────────┬─────────┐
│ name                   │ current  │ upstream │ drift   │
├────────────────────────┼──────────┼──────────┼─────────┤
│ agent-browser-harness  │ abc1234  │ def5678  │ content │
│ sancho-my-task         │ 1111111  │ 2222222  │ manifest│
│ agent-stable-one       │ abcdef0  │ abcdef0  │ no      │
└────────────────────────┴──────────┴──────────┴─────────┘
```

Drift types:
- `no` — `resolved_sha` unchanged. Up to date.
- `content` — `resolved_sha` differs, `manifest_hash` unchanged. Safe silent upgrade.
- `manifest` — both differ. User re-trust required (capabilities, sandbox, install script may have changed).

Pipe-friendly: `makakoo plugin outdated --json` emits one object per plugin.

## `plugin update <name>` — one plugin

```bash
makakoo plugin update agent-browser-harness
```

Flow:
1. Load lock entry.
2. Probe upstream.
3. If up-to-date → exit 0 with "up to date (sha abc1234)".
4. If content-only drift → reinstall silently, exit 0.
5. If manifest drift → print the new sha + manifest_hash, prompt `y/N`. Decline → nothing changes. Accept → uninstall + reinstall, preserve `enabled` flag.

Pass `--yes` to skip the manifest-drift prompt. Use only when you trust upstream unconditionally.

## `plugin update --all` — batch

```bash
makakoo plugin update --all --yes
# → 3 up-to-date, 2 updated, 0 failed
```

Per-plugin error isolation — one failing plugin never blocks the rest. Tarball-sourced plugins are skipped with a hint, since they'd need a fresh `--sha256` before we can safely promote a new archive.

## SANCHO nightly update-check

There's an opt-in SANCHO task at `plugins-core/sancho-task-plugin-update-check/` that runs every 24h. Report-only: appends a Brain journal line per drifted plugin, never auto-installs.

Next-morning journal:

```markdown
- [[Harvey]] plugin update available: agent-browser-harness @ abc1234 → def5678 (manifest). Run `makakoo plugin update agent-browser-harness`.
- [[Harvey]] plugin update available: sancho-my-task @ 1111111 → 2222222 (content). Run `makakoo plugin update sancho-my-task`.
```

Install it once:

```bash
makakoo plugin install --core sancho-task-plugin-update-check
# SANCHO picks it up on next boot; no extra config.
```

Disable temporarily via `makakoo plugin disable sancho-task-plugin-update-check` — nightly check skips without uninstalling.

## Why re-trust on manifest change

If a plugin update adds a new capability grant, expands its sandbox, or changes its install script, you want to see that BEFORE the new code runs on your machine. The prompt is a security contract, not a UX choice — same pattern as the v0.3 adapter re-trust prompt.

`manifest_hash` is a sha256 of the raw `plugin.toml` bytes, captured at install time. Any whitespace-only change also triggers the prompt; we'd rather over-prompt than miss a capability-expansion hidden behind formatting.

## Rollback

Makakoo doesn't ship a first-class "rollback" command. The workflow:

1. You declined the update (or it broke in production).
2. Your `plugins.lock` still holds the old `resolved_sha`.
3. `makakoo plugin update <name>` re-prompts; accept when you're ready.
4. If you accepted a bad update, reinstall from a known-good tag:

```bash
makakoo plugin uninstall agent-browser-harness
makakoo plugin install git+https://github.com/browser-use/browser-harness@<good-sha>
```

## Internals

- **Primitives**: `makakoo-core::plugin::{probe_upstream, apply_update, drop_probe, list_updatable}` (Phase C).
- **Drift classification**: `ProbeDrift::{UpToDate, ContentOnly, ManifestChange}`.
- **Lock file**: `$MAKAKOO_HOME/config/plugins.lock` is authoritative; hand-edits are discouraged but supported (malformed entries surface as `InvalidLockSource`).
- **SANCHO task format**: locked in `plugins-core/sancho-task-plugin-update-check/src/tick.py` with Python tests in `tests/test_tick.py` that pin every journal line shape.
