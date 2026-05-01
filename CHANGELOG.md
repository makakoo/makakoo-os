# Changelog

All notable changes to Makakoo OS are tracked here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are added on every tagged release. The GitHub Release workflow at
`.github/workflows/release.yml` also generates per-tag notes automatically
via `generate_release_notes: true` — this file is the curated long-form
complement, focused on user-visible changes and migration notes.

## [Unreleased]

### Added — Pattern substrate v1 (`SPRINT-PATTERN-SUBSTRATE-V1`, 2026-05-01)

A subagent dispatch substrate inspired by Daniel Miessler's Fabric, reframed
for Makakoo's parasite-OS model. Patterns are markdown system-prompt units
callable identically from CLI, MCP, and any future surface — letting Harvey
shell out one-shot LLM dispatch without burning host CLI context tokens.

- **`kind = "pattern"` plugin kind.** Patterns are markdown + TOML, no Python
  entrypoint, no daemon. New `[pattern]` table declares `model`, `vendor`,
  `strategy_default`, `mascot_default`, `tags`, and a `[[pattern.variables]]`
  list. Sibling `system.md` carries the prompt body. Loader graceful-skips
  pattern dirs missing `system.md`.
- **`makakoo run pattern=<name>` CLI verb.** Composes
  `strategy ⊕ mascot ⊕ pattern → system message`, fires `switchAILocal`,
  returns text or JSON. Flags: `--input`/`--var`/`--mascot`/`--strategy`/
  `--model`/`--vendor`/`--dry-run`/`--json`. Stdin (`-`), file (`@path`),
  or literal input all supported.
- **Five strategy files** baked in via `include_str!`:
  `cot, tot, react, harvey-rigor, caveman`. User overrides at
  `$MAKAKOO_HOME/data/strategies/<name>.md` win when present. The caveman
  strategy ports lope's `CAVEMAN_VALIDATOR_DIRECTIVE` plus a HARD-GATE
  BYPASS preamble that skips compression for any external-writing context.
- **Per-pattern model + vendor pinning.** Resolution precedence (highest
  first): pattern.toml → flag → `FABRIC_MODEL_<NAME>` env → kernel default.
  Same shape for vendor sans env. Hyphens in pattern names normalize to
  underscores in the env-var key.
- **Mascot persona externalization.** Olibia's `SYSTEM_PROMPT_FRAGMENT`
  promoted to `plugins-core/mascot-olibia/persona.md`; Pixel/Cinder/Ziggy
  ship as placeholder slots ready for voice authoring. Python `mascot.py`
  lazy-loads from disk with the embedded constant as fallback.
- **MCP auto-expose at boot.** `makakoo-mcp` walks
  `<makakoo_home>/plugins/pattern-*/` and registers one `pattern_<name>`
  tool per discovered pattern. JSON Schema is generated mechanically from
  `[pattern].variables`. Five routing controls (`_strategy`, `_mascot`,
  `_model`, `_vendor`, `_json`) are added to every tool's schema.
  Every infected CLI sees new patterns as `mcp__harvey__pattern_<name>`
  on next session — no per-CLI code, no manual registration.
- **MCP caveman default with tag bypass (Locked Decision 11).** Patterns
  invoked via MCP default to the `caveman` strategy when no
  `strategy_default` is declared and the pattern's `tags` does not include
  `external` or `polished`. The `_strategy` argument always overrides.
  CLI invocations stay neutral — the host CLI already governs voice.
- **Two seed patterns shipped:** `pattern-summarize` (5-bullet summary,
  `gemini-2.5-flash-lite`) and `pattern-extract-wisdom` (insights extraction
  with `harvey-rigor` strategy default, `gemini-2.5-pro`).

Test counts: 1858 passed / 0 failed / 5 ignored (workspace), +74 net new
tests across `manifest`, `registry`, `run::*`, `commands::run`,
`tests/run_pattern.rs`, and `handlers::patterns`.

Out of v1 scope (queued for v2 sprints): Brain-aware templating namespaces
(`{{brain:...}}`, `{{garage:...}}`, `{{persona:...}}`), session resumption
with vendor-message conversion, git-sourced pattern marketplace,
pattern-driven file-changes apply, custom-pattern shadow directories.

### Added — `makakoo setup` interactive wizard (`MAKAKOO-SETUP-WIZARD`, 2026-04-23)

- **Section dispatcher** — the one-shot `makakoo setup` persona picker is
  now the first section of a re-runnable wizard. Bare `makakoo setup`
  walks every section; `makakoo setup <section>` runs one; `--only`
  and `--skip` scope the list. Existing `--force` still applies to the
  persona section.
- **New sections:**
  - `brain` — shells to the existing `skill-brain-multi-source` picker
    to register Logseq / Obsidian / plain-markdown vaults.
  - `cli-agent` — Y/n/s prompt + `npm install -g @mariozechner/pi-coding-agent`.
  - `terminal` — macOS-only Y/n/s prompt + `brew install --cask ghostty`.
  - `model-provider` — introduces `~/.makakoo/primary_adapter.toml`, a
    single-field TOML pointing at the default routing adapter.
  - `infect` — thin wrapper over `makakoo infect --verify` + `makakoo infect`.
- **State file** at `$MAKAKOO_HOME/state/makakoo-setup/completed.json`
  records per-section status with atomic writes + schema-versioned
  forward-compat loader.
- **Install hand-off** — `makakoo install` now offers to run the wizard
  at the end. `--no-setup` flag skips the prompt; non-TTY installs
  never prompt.
- **New primitive in makakoo-core:** `adapter::registry::{primary_adapter_path, load_primary_adapter, write_primary_adapter}` — atomic, registry-validated, wizard-driven.
- Docs: `docs/setup-wizard.md`.
- Tests: 9 new primary-adapter unit tests in `makakoo-core`,
  ~54 setup unit tests + 13 setup integration tests in `makakoo`.

### Fixed — v0.3.3 Security Lockdown (`MAKAKOO-OS-V0.3.3-SECURITY-LOCKDOWN`, 2026-04-21)
- **Grant ownership check on revoke** (closes pi N3). New `owner`
  field on every grant captures the caller's plugin at create time;
  `do_revoke` / `RevokeWriteAccessHandler::call` refuse unless the
  caller's plugin matches OR the caller is an admin bypass
  (`cli`, `sancho-native`). Without this, a compromised skill with
  knowledge of another agent's grant_id could silently revoke it.
  Denial emits `correlation_id="reason:not_owner"` audit entry.
  Backward-compatible: pre-v0.3.3 records with no `owner` field
  fall back to their `plugin` attribution on load.
- **SANCHO `perms_purge_tick` idempotency key** (closes pi R2).
  New `makakoo_core::capability::purge_idempotency` module. When
  the 900s tick fires twice within 60s (daemon restart, clock skew),
  the second run now returns `skipped (within Ns cooldown since
  last tick)` without touching the grant store — no more double
  audit entries for the same revocations. CLI `makakoo perms purge`
  deliberately skips the gate (admin bypass).
- **`makakoo perms list --json` structured envelope** (closes the
  gemini nit). Pre-v0.3.3 the flag emitted an undocumented flat
  array; now it emits `{schema_version, baseline, active,
  expired_today_count, all}` matching the MCP `list_write_grants`
  response shape. CI / IDS / dashboards use one parser across CLI
  and MCP surfaces.
- New shared drift fixture
  `plugins-core/lib-harvey-core/tests/fixtures/grant_ownership_vectors.json`
  (6 cases) loaded by both Python and Rust test suites. Sixth
  Python↔Rust drift gate.

### Fixed — v0.3.2 Rust MCP Phase B/C parity (`MAKAKOO-OS-V0.3.2-MCP-PARITY`, 2026-04-21)
- **Rust MCP `grant_write_access` now enforces `origin_turn_id` on
  conversational channels.** v0.3.1 closed the gap for the Python
  conversational path (HarveyChat, Telegram, infected-CLI
  HARVEY_TOOLS dispatch). The Rust MCP handler at
  `makakoo-mcp/src/handlers/tier_b/perms.rs` — which is what Claude
  Code, Cursor, Vibe, and every other MCP-native CLI actually calls —
  did not. Now it does. Closes R2's residual T1 for the Rust direct
  path in `spec/USER_GRANTS_THREAT_MODEL.md`.
- **Every Rust MCP grant refusal now writes a
  `result="denied"` audit entry** with the same
  `correlation_id="reason:<kind>"` taxonomy as Python: `too_broad`,
  `bad_duration`, `permanent_outside_home_unconfirmed`,
  `rate_limit_active`, `rate_limit_hourly`,
  `missing_origin_turn_id`. Python and Rust now emit identical
  denial signals — IDS / forensic tooling no longer has to special-
  case which runtime emitted the refusal.
- **Shared drift-gate fixture** at
  `plugins-core/lib-harvey-core/tests/fixtures/conversational_channels.json`
  is loaded by both Python and Rust tests. Both sides assert their
  own `CONVERSATIONAL_CHANNELS` set equals the fixture — adding a
  plugin slug on one side without the other fails both suites.
- New `makakoo_core::capability::CONVERSATIONAL_CHANNELS` + 
  `is_conversational_channel(plugin)` exported for downstream
  consumers.

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
