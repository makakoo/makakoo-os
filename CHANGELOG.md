# Changelog

All notable changes to Makakoo OS are tracked here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are added on every tagged release. The GitHub Release workflow at
`.github/workflows/release.yml` also generates per-tag notes automatically
via `generate_release_notes: true` — this file is the curated long-form
complement, focused on user-visible changes and migration notes.

## [Unreleased]

### Fixed — v0.3.1 User-Grants Hardening (`MAKAKOO-OS-V0.3.1-PERMS-HARDENING`, 2026-04-21)
- **Rate-limit self-DoS closed.** `creates_in_window` now decrements
  on revoke (symmetric with increment-on-grant). Without this a single
  CLI session could cycle 50 grant/revoke pairs and lock itself out
  of the grant system for an hour even with zero active grants. Fix
  spans both Python (`core.capability.rate_limit.decrement`) and Rust
  (`makakoo_core::capability::rate_limit::decrement`), wired into
  `perms_core.do_revoke()` and `makakoo perms revoke`. Shared drift
  fixture at `plugins-core/lib-harvey-core/tests/fixtures/rate_limit_decrement_vectors.json`.
  Closes pi R1, opencode #1.
- **Grant denials now audited.** Every refusal from `do_grant()`
  (`too_broad`, `bad_duration`, `permanent_outside_home_unconfirmed`,
  `rate_limit_active`, `rate_limit_hourly`) emits one
  `logs/audit.jsonl` entry with `result="denied"` and a
  `correlation_id="reason:<kind>"` taxonomy tag. Makes post-incident
  intrusion detection on the grant subsystem possible. Closes
  opencode #2, minimax #2.
- **`origin_turn_id` now enforced on conversational channels.** New
  module constant `CONVERSATIONAL_CHANNELS` (11 slugs). When `plugin`
  is in the set and `origin_turn_id` is empty, `do_grant()` refuses
  with `origin_turn_id required on conversational channels (...)`
  before scope/duration gates. Closes the prompt-injection path where
  a fabricated `grant_write_access(user_turn_id=null)` call landed
  indistinguishably from a legit human-turn grant. Closes gemini #1,
  minimax #3, opencode §3, pi R3 (related). `cli` and `sancho-native`
  remain unaffected (no human turn). Python-only this sprint; Rust
  MCP handler enforcement deferred to v0.3.2.

### Added — v0.3 User Grants (`MAKAKOO-OS-V0.3-USER-GRANTS`, 2026-04-21)
- Three-layer additive write-permission model (baseline → manifest →
  user grants). Agents can now write outside the hardcoded baseline
  when the user grants access — without editing code or restarting.
  See `spec/CAPABILITIES.md §1.11` for the precedence diagram +
  worked example.
- `$MAKAKOO_HOME/config/user_grants.json` — machine-local, gitignored
  grant store with sidecar-lock protocol (LD#9), atomic temp-rename,
  corrupt-file tolerance. Full schema + lock contract at
  `spec/USER_GRANTS.md` v1.0.
- `makakoo perms {list,grant,revoke,purge,audit,show}` — dedicated
  CLI for scripted + CI workflows. Strict duration grammar
  (`30m|1h|24h|7d|permanent`); broad scopes (`/`, `~`, `**`, `*`)
  refused with `too broad`; `permanent` outside `$MAKAKOO_HOME`
  requires `--confirm yes-really`.
- `grant_write_access` / `revoke_write_access` / `list_write_grants`
  — conversational MCP + HARVEY_TOOLS handlers. Every infected CLI
  can issue + list + revoke grants from chat. Canonical replies
  quoted verbatim by the agent; shared scenario fixture at
  `tests/fixtures/grant_tool_vectors.json` locks Python ↔ Rust
  drift.
- `perms_purge_tick` — SANCHO native handler #10. Runs every 900s,
  drops expired grants, emits one `perms/revoke` audit per removed
  grant with `correlation_id="reason:expired"` and
  `plugin="sancho-native"`.
- `perms/grant` + `perms/revoke` audit verbs. Both land in
  `logs/audit.jsonl` under the existing schema with
  `plugin="cli"`, `plugin="sancho-native"`, or any
  `HARVEY_PLUGIN` env value from a conversational surface.
- Rate-limit guardrail (LD#14): max 20 active grants, max 50
  create-ops per rolling hour. Counter state in
  `state/perms_rate_limit.json` so a corrupt counter can't poison
  grants.
- Telegram allowlist gate — `HARVEY_PLUGIN=harveychat-telegram` +
  `HARVEY_TELEGRAM_CHAT_ID` routed through the existing
  `data/chat/config.json` allowlist. Non-allowlisted chats get an
  `authz:` refusal and an audit entry with `result=denied`.
- Write-access-grants section in every infected CLI bootstrap
  (claude / gemini / codex / opencode / vibe / cursor / qwen / pi).
  Carries the rejection-path flow + verbatim-quote rule. Re-run
  `makakoo infect --global` to propagate.
- Threat-model doc at `spec/USER_GRANTS_THREAT_MODEL.md`: 6-asset
  register, 4 adversary types (T1–T4), 10-row per-surface authN
  matrix, STRIDE pass, R1–R4 residual-risk register.

### Changed — v0.3
- `WRITE_FILE_ROOTS` (hardcoded tuple) → three-layer resolver
  `_resolve_write_path()`. Baseline resolution is now env-aware
  (reads `$MAKAKOO_HOME` at call time instead of at import).
- Write-file rejection string now suggests the exact
  `makakoo perms grant '<path>' --for 1h` command to run.
- `HARVEY_SYSTEM_PROMPT` gains an `{allowed_paths}` placeholder
  rendered per-call with the active baseline + grants. Agents see
  their current writable surface in every turn.
- `HARVEY_PLUGIN` env var now propagates from chat bridge → every
  audit entry. Audit log shows which CLI made each perms call.
- `NATIVE_TASK_COUNT: 9 → 10`, `NATIVE_TASK_NAMES` appends
  `"perms_purge_tick"`. Gated by `native_task_names_match_registry`.

### Added
- `makakoo uninfect` — symmetric inverse of `makakoo infect --global`.
  Strips the bootstrap block from every global CLI slot (or the
  `--target <csv>` subset), deletes infect-created-only files, preserves
  user prose around the block. `--dry-run` previews without writing.
- Shell completion via `makakoo completion <bash|zsh|fish|elvish|powershell>`
  + install guide at `install/completions/README.md`.
- `makakoo plugin enable/disable/update` — soft lifecycle verbs.
- `makakoo distro save` — serialize the live install into a reproducible
  distro TOML pinned by exact version + blake3 per plugin.
- Two new distros: `creator.toml` (writers/streamers/artists) and
  `trader.toml` (market-facing autonomous agents). DoD #8 now 5/5.
- Windows added to the CI test matrix — `windows-latest` joins
  macOS + Ubuntu so `#[cfg(windows)]` code paths get exercised per push.
- Plugin install rejects sancho-task name collisions with native kernel
  handlers. New `InstallError::NativeTaskCollision`.
- `makakoo sancho status` now prints `N registered task(s) (X native +
  Y manifest)` so the split is visible at a glance.

### Changed
- Capability socket env var is `MAKAKOO_SOCKET_PATH` across the whole
  stack (kernel spawn + Rust client + Python client + ABI docs). Prior
  drafts used `MAKAKOO_PLUGIN_SOCKET` in the spawn path, which silently
  broke plugins dialing the socket. Regression test locks the name.
- Release pipeline: cargo-dist `ci = ["github"]`, target set includes
  Linux aarch64 + Windows x86_64. Actual release builds + publishing run
  from `.github/workflows/release.yml` on tag push.

### Changed (pre-0.1.0 tag)
- Plugin subprocess CWD is now the plugin's install root, not
  `$MAKAKOO_HOME`. Relative paths in `[entrypoint].run` (e.g.
  `python3 -u src/run.py`) now resolve inside the plugin's own
  bundled source tree. `$MAKAKOO_HOME` stays exported in env so
  plugins can still reach shared state via absolute paths.
- 32 plugins-core entries migrated to the self-contained shape —
  Python source bundled under `plugins-core/<name>/src/`. Public
  users installing any shipped plugin get the code bundled; no
  harvey-os clone required. Helper shipped at
  `scripts/migrate_skill.py --copy-src`.
- `$MAKAKOO_PLUGIN_ROOT` now exported to every spawned skill
  subprocess so ad-hoc shell one-liners can reach their own
  bundled files even after a `cd` elsewhere.

### Deferred to a later release
- Apple notarization + Windows Authenticode signing — awaits signing
  cert acquisition. Runbook in `docs/RELEASE_SIGNING.md`.
- Audit log rotation (100 MB / 7-day retention) — Phase G log-management
  story.
- NetHandler for `net/http|tcp|udp|ws` capability verbs — plugins that
  want kernel-enforced network egress wait for Phase H.4.
- winget submission — `distribution/winget/makakoo.yaml` manifest is
  drafted; PR into `microsoft/winget-pkgs` happens post-v0.1.

## [0.1.0] - YYYY-MM-DD

Placeholder entry. Populated at tag time.
