# Changelog

All notable changes to Makakoo OS are tracked here. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are added on every tagged release. The GitHub Release workflow at
`.github/workflows/release.yml` also generates per-tag notes automatically
via `generate_release_notes: true` — this file is the curated long-form
complement, focused on user-visible changes and migration notes.

## [0.3.1] - 2026-08-28

### Fixed

- **`makakoo agent start` now works on macOS and Linux.** v0.3.0 generated a
  LaunchAgent plist (and systemd user unit) whose environment held only
  `MAKAKOO_AGENT_SLOT`. Because launchd and systemd-user start services with a
  deliberately minimal environment, the supervisor could not see
  `MAKAKOO_HOME` and exited with `slot '<name>' not found`; once that was
  supplied it failed again with `Node.js 22.9+ is required`, because
  `PATH=/usr/bin:/bin:/usr/sbin:/sbin` excludes every nvm and Homebrew node.
  Background start was unusable — only the `MAKAKOO_AGENT_SUPERVISOR=foreground`
  escape hatch worked. Both backends now export `MAKAKOO_HOME`, prepend the
  directory of the `node` that passed the version gate to the service `PATH`,
  and load `~/.env` for LLM credentials. `MAKAKOO_HOME` is pinned *after* the
  `~/.env` load, so a stale entry there cannot redirect a supervisor at a home
  that does not contain the slot.
- **A failed agent turn no longer reports success with an empty answer.** When
  the model loop ended in an error — an upstream 4xx/5xx, or exhausted
  retries — the generated runtime returned `200 {"response": ""}`. The failure
  was invisible to `makakoo agent prompt`, indistinguishable from a model that
  simply had nothing to say, and recoverable only by decompressing the session
  trace by hand. The runtime now walks back to the terminating `turn/end`,
  and returns `502` with the upstream message, error code, and status.

### Notes

- Not every switchAILocal model can drive a tool-using agent. `ail-compound`
  has no backend advertising `chat_multiturn_tools` and fails with HTTP 422 the
  moment a tool result is fed back; `ail-fast` handles the full loop. With the
  fix above this now surfaces as a clear error instead of an empty response.

## [0.3.0] - 2026-08-27

### Added

- **Supervised DeepSeek Harness agent runtime.** `makakoo agent create`
  now compiles AgentSpec YAML/TOML into a pinned DSH project under
  `$MAKAKOO_HOME/agents-dsh/<slot>/`; DSH owns the model/tool loop while
  AgentSlot remains the Makakoo policy authority.
- **Complete local lifecycle.** `agent start`, `stop`, `restart`, `status`,
  `health`, `prompt`, and transactional `destroy` cover generated DSH slots.
  launchd and systemd-user run one supervisor per slot; an explicit
  `MAKAKOO_AGENT_SUPERVISOR=foreground` escape hatch supports containers and
  debugging.
- **Authenticated prompt API and durable sessions.** Each runtime binds only
  to `127.0.0.1`, rotates a mode-0600 bearer token per start, serializes turns
  within a session, and bounds concurrency, queueing, session count, turns,
  prompt size, and total persistence admission.
- **User discovery surface.** Added the `deepseek-harness-agent-runtime`
  installed skill, a complete DSH walkthrough, an executable local-researcher
  example, CLI/manual coverage, troubleshooting entries, and clear separation
  from plugin-agent MCP scaffolding.

### Changed

- **BREAKING: DSH is the default AgentSpec engine.** Autonomous model calls
  use the fixed switchAILocal OpenAI-compatible endpoint at
  `127.0.0.1:18080/v1`. Explicit non-switchAILocal provider prefixes are
  rejected. Set `MAKAKOO_AGENT_ENGINE=flue` only for the legacy, manually
  operated Flue compatibility renderer.
- `makakoo agent provider-set <provider> [model]` and
  `makakoo agent provider-get` are the authoritative project-default commands.
  Stale `makakoo provider ...` documentation was removed.
- AgentSpec tool lists are strict allowlists. `tools: []` exposes no
  model-facing tools and generated runtimes never inherit a hidden baseline.
- `agent stop` now removes its LaunchAgent/systemd-user definition after the
  process tree exits. A later `start` recreates it; stopped or destroyed slots
  cannot resurrect at login.
- DSH requires Node.js 22.9 or newer. Direct runtime dependencies are exact
  pinned at `0.1.1-rc.2`; the full upstream DSH CLI is not installed.

### Security

- The supervisor holds an exclusive per-slot runtime lock, starts the gateway
  in a dedicated Unix process group, terminates the whole descendant tree, and
  gives the Node gateway a parent-death watchdog. Duplicate foreground/service
  starts fail closed.
- DSH native shell and filesystem tools are not mounted. `makakoo-mcp` is the
  only generated tool source and receives `MAKAKOO_AGENT_SLOT` for server-side
  discovery and call authorization.
- Signed HTTP adapter calls made inside an agent slot now propagate
  `X-Makakoo-Agent-Id` in the signed digest. Calls without an agent binding are
  explicit trusted-peer administrator calls; attribution cannot be stripped or
  changed without invalidating the signature.
- Destroy remains confirmation-gated, proves shutdown before moving state, and
  archives managed runtime/data/TOML transactionally. Secret revocation remains
  separately explicit.
- The MCP scope boundary now counts only `http`/`https`/`data:` sources as
  remote (Windows drive-letter paths can no longer bypass `allowed_paths`),
  channel tools reject a `slot_id` that is not the calling agent, and
  server-side knowledge ingest refuses loopback/private/link-local URLs.

### Fixed

- `agent stop`/`destroy` on platforms without launchd/systemd now prove the
  slot is offline before reporting success and refuse while a foreground
  supervisor holds the runtime lock.
- `agent status`/`stop` keep matching a supervisor started from a pre-upgrade
  binary (status.json records the supervisor's own executable path), tolerate
  install paths containing spaces, and the gateway cleanup never signals a pid
  that was recycled inside the TERM→KILL window.
- The generated DSH runner honors the per-slot `[llm.override].model` (the
  compiled spec model is the fallback), out-of-range `max_tokens` fails fast at
  start instead of crash-looping, and the runtime finds a sibling
  `makakoo-mcp.exe` on Windows.
- The legacy gateway's parent watchdog no longer terminates its supervisor on
  Windows, where `os.kill(pid, 0)` maps to `TerminateProcess`.

### Known limitations

- DSH V1 preserves channel and trigger declarations but does not start
  Telegram, Slack, Discord, WhatsApp, email, voice, webhook, or cron adapters.
  Use `agent prompt` or a trusted local adapter; the CLI warns when declarations
  are not executable.
- DSH `0.1.1-rc.2` is an upstream release candidate. Package upgrades remain a
  deliberate Makakoo integration decision.
- Background service registration ships for macOS and Linux. Other platforms
  require explicit foreground mode.

## [0.2.3] - 2026-08-02

### Changed

- **Friendlier setup wizard.** A full copy pass over `makakoo setup`:
  every section now explains in plain language what it does, what a yes
  answer will run, and that Enter always picks the safe default. Internal
  jargon (SANCHO, "blessed", "bootstrap block", "validator ensemble") is
  gone from user-facing text — e.g. the infect step now says "Connect
  Makakoo to your installed AI CLIs", and the updates step states that
  updates never touch your settings or data. The wizard opens with
  "nothing changes without your confirmation."

## [0.2.2] - 2026-08-02

### Changed

- **Clearer post-update setup prompt.** The `Review setup defaults / new
  sections now?` question after `makakoo update` read as if answering `n`
  might discard existing configuration. The prompt now states explicitly
  that settings are kept either way and that `y` only opens a review of
  sections added or changed in the new version.

## [0.2.1] - 2026-08-02

### Changed

- **`makakoo update` now restarts the daemon automatically.** When the
  update actually changed the installed version and a daemon service
  exists, the updater invokes `makakoo daemon restart` through the newly
  installed binary — no manual step left behind. Installs without a
  daemon are untouched (restart would have installed one). Opt out with
  `--no-daemon-restart` or `MAKAKOO_UPDATE_NO_DAEMON_RESTART=1`; on
  opt-out or restart failure the familiar manual hint is printed.

## [0.2.0] - 2026-08-02

### Added

**DSPy-compiled intent router prompt (promoted on held-out evidence).**
The `lib-harvey-core` intelligent router's LLM classifier now ships a
MIPROv2-compiled prompt baked as dependency-free string constants
(`router_compiled_prompt.py`) — DSPy runs offline only, the runtime imports
no new packages. Held-out test (n=25, paired): 80% accuracy vs 36% for the
keyword table, delta +0.44, bootstrap 95% CI [+0.20, +0.68], McNemar
p=0.007, decision `97b4c1d5…`. Golden tests pin the baked renderer to
DSPy's own rendered messages. Rollback: `MAKAKOO_ROUTER_COMPILED_PROMPT=off`.

- Router LLM path fixes: dead `timeout=0.3` → `MAKAKOO_ROUTER_LLM_TIMEOUT`
  (default 10s); model selection is provider-agnostic
  (`MAKAKOO_ROUTER_LLM_MODEL` → `LLM_MODEL` → `auto`, no vendor default);
  missing confidence in compiled replies defaults to measured accuracy
  instead of 0.0 (which silently flunked `is_confident()` gates).
- `skill-ai-ml-dspy` v2.0.0: rewritten against dspy 3.2.1 and the verified
  compile-offline-ship-strings workflow; stale 1.x references deleted.

### Added (spec sprint)

**Sprint: declarative agent spec format + LLM provider auto-detection**
(Phase 1-6 of SPRINT-FLUE-DEFAULT-AGENT-SPECS). `makakoo agent create --specs <PATH>`
now scaffolds a runnable Flue (TypeScript) project from a YAML or TOML spec
in one command, with zero manual `app.ts` editing.

- **Spec format** (Phase 1-5). `AgentSpec` / `ChannelSpec` / `TriggerSpec` /
  `ScopeSpec` in `makakoo-core/src/agents/spec/`. YAML + TOML parsing, validation,
  conversion to slot TOML. `makakoo agent create --specs <PATH>` accepts a file,
  directory, or `.` to scan the current folder. `makakoo agent validate-spec <PATH>`
  for dry-run validation.
- **6 channel kinds** (Phase 2-4). `telegram`, `slack`, `discord`, `webhook` in V1;
  `email`, `voice` deferred to V2. Each scaffoldered as `src/channels/<kind>-<n>.ts`.
- **2 trigger kinds** (Phase 2-4). `cron` (5-field, via `node-cron`),
  `webhook` (standalone Hono server on port 8809).
- **13-file Flue scaffolder** (Phase 4). Was a single `flue_scaffold.rs`; now a
  directory with `app.rs`, `assistant.rs`, `context.rs`, `package_json.rs`,
  `mcp_proxy.rs`, `env_example.rs`, `readme.rs`, `gitignore.rs`, plus
  `channels/{telegram,slack,discord,webhook,email,voice}.rs` and
  `triggers/{cron,webhook}.rs`.
- **LLM provider auto-detection** (Phase 6).
  `makakoo-core::agents::llm_provider::discover_providers()` concurrently probes
  `http://localhost:18080/v1/models` (switchailocal),
  `http://localhost:11434/api/tags` (Ollama), and `ANTHROPIC_API_KEY` /
  `OPENAI_API_KEY` env vars. Sorts local-first, concurrent 2s timeout.
- **`src/app.ts` always scaffolded** (Phase 6) with the right
  `registerProvider` call for the chosen provider. Local providers (switchailocal,
  ollama) get `api` + `baseUrl` + the lope team fix. Cloud providers (anthropic,
  openai) get only `apiKey` (catalog provides the rest). No more manual
  `app.ts` editing after scaffolding.
- **lope team fix baked in** (Phase 6, critical). Flue v1.0.0-beta.9 silently
  defaults `contextWindow: 0` and `maxTokens: 0` for non-catalog providers,
  which limits the LLM output to **one token** and looks like a "hang".
  The scaffolder **always sets both to safe values** (`128_000` and `8_192`) for
  local providers.
- **4 example specs** in `examples/agents/`: `weather-bot.yaml`, `slack-assistant.yaml`,
  `ollama-local.yaml`, `personal-day-brief.yaml`.
- **Interactive agent creation** (Phase 6.1).
  - `makakoo provider set <provider> [model]` / `makakoo provider get` — project-level
    LLM default at `$MAKAKOO_HOME/config/llm-default`.
  - `makakoo agent init-spec <PATH>` — interactive TTY starter. Asks the right
    questions, discovers providers, respects the project default, writes a correct
    spec. `--minimal` flag emits a 10-line "hello world" spec.
- **3 new `AgentCmd` variants**: `InitSpec`, `ProviderSet`, `ProviderGet`.
- **End-to-end proven** (Phase 6). `makakoo agent create --specs ./weather-bot.yaml`
  → Flue dev server → switchailocal (model `ail-compound`) → `brain_search` tool
  via Makakoo MCP → Telegram bot reply (screenshot-verified by user).

### Changed

- **BREAKING** (Phase 4, predates 0.2.0): the original single-file
  `makakoo/src/commands/flue_scaffold.rs` was split into a 13-file directory at
  `makakoo/src/commands/flue_scaffold/`. The `scaffold_flue_project` function
  signature gained a 4th parameter `llm_provider: Option<&DiscoveredProvider>`.
- Spec discovery now accepts a third argument `inline_secrets: &InlineSecrets`
  (was a hardcoded empty map in the spec path).

### Known limitations

- **Flue v1.0.0-beta.9 LLM dispatch bug for some Ollama `:cloud` models** — `flue
  dev` accepts the webhook, starts the agent session, but the background worker
  never fires the LLM call. The LLM itself is fine (direct `curl` to the provider
  works in <1s). The bug is in the Flue runtime's background worker. Workaround:
  use switchailocal (proven end-to-end) or wait for an upstream Flue fix.
- **Provider detection is best-effort** — 2s probe timeout means a slow LLM gateway
  (cold start, network latency) might be missed. Set `AGENT_MODEL` in `.env` or set
  the spec's `model` explicitly if the auto-detection misses your provider.



### Added
- **Agent spec format** — declarative YAML/TOML agent definitions (name, model, instructions, tools, channels, triggers, scope). `makakoo agent create --specs <PATH>` accepts a file, directory, or `.` to scan the current folder. `makakoo agent validate-spec <PATH>` validates without creating. See `docs/agents/spec.md` for the full schema.
- **6 channel kinds** — telegram, slack, discord, webhook (V1); email, voice (V1 deferred, see Limitations).
- **2 trigger kinds** — cron (standard 5-field), webhook (HMAC-SHA256, standalone Hono server on port 8809).
- **Flue project scaffolder** — `flue_scaffold` module renders a complete runnable Flue (TypeScript) project from a spec: `package.json` (deps driven by the spec's channels/triggers), `src/agents/assistant.ts` (wires model + instructions + tool whitelist + channels), `src/channels/<kind>-<n>.ts` (one per channel), `src/triggers/<kind>-<n>.ts` (one per trigger), `mcp-proxy.mjs` (unchanged stdio→StreamableHTTP bridge to `makakoo-mcp`), `instructions.txt`, `.env.example`, `README.md`, `spec.yaml` (verbatim copy for reproducibility).

### Changed
- **BREAKING**: `--runtime` flag removed from `makakoo agent create`. Flue is the only creation engine.
- **BREAKING**: `--from-toml` flag removed. Use `--specs` instead. See `docs/agents/spec-migration.md`.
- **BREAKING**: `<SLOT>` positional argument is now optional when `--specs` is used (the spec's `name` becomes the slot id).
- Slot TOML schema unchanged. Existing native slots keep working.

### Limitations (tracked for V2)
- **Email & voice channels** — no first-party `@flue/*` adapter on npm. `agent create` with these channels writes the slot TOML but errors at scaffold time with a clear message. Workaround: use a `webhook` channel + a custom `defineTool` for SMTP/IMAP/Twilio.
- **Slack/Discord outbound** — the Flue channel only handles inbound. Outbound requires an operator-supplied `defineTool` calling the platform's Web/REST API. The generated template includes a `post_slack_message` starter.
- **Telegram `allowedUsers`** — not a channel config option in `@flue/telegram`. Enforced inside the webhook handler before `dispatch()`.
- **Cron** — uses `node-cron` directly. `@flue/runtime` has no `defineTrigger` export. Standard 5-field cron only.
- **Webhook trigger** — standalone Hono server on port 8809 (convention). Triggers are loaded as side-effecting imports.
- **Scope overlap detection** — exact-string match only. Proper glob overlap requires `globset` and is deferred to V2.
- **Orphan slot TOML** — if a spec with a deferred channel (email/voice) is scaffolded, the slot TOML is written first, then the scaffold errors. The slot is harmless without a Flue project; cleanup via `makakoo agent destroy <slot>` if desired.



## [0.1.41] - 2026-07-17

### Added
- `makakoo cli add|list|remove` — a runtime registry of custom CLI hosts at `$MAKAKOO_HOME/config/cli_hosts.json`, merged into every `makakoo infect` run. New AI CLIs can now be onboarded without recompiling the binary. `makakoo cli add <name>` autodetects the bootstrap file (`AGENTS.md`/`CLAUDE.md`/`GEMINI.md`) and MCP config (`config.toml`/`mcp.json`/`settings.json`) under `~/.<name>/`, sniffs the MCP format, and supports `--from <known-host>`, explicit `--config-dir`/`--bootstrap-file`/`--mcp-file`/`--mcp-format` overrides, and `--no-mcp`. The nine built-in hosts are unchanged; a custom host whose name collides with a built-in is ignored.
- New MCP format primitive `toml-simple` — a plain `[mcp_servers.<name>]` inline table with `enabled`/`env` and none of Codex's `env_vars`/`model_instructions_file` extras. This is the schema the Grok CLI uses, and the generic primitive for any future TOML-mcp custom host.

### Notes
- `makakoo infect` (write/refresh) now covers registered custom hosts on both surfaces (bootstrap + MCP). `makakoo infect --verify` (drift audit) still iterates the built-in host list only — custom-host drift coverage is a follow-up.

## [0.1.40] - 2026-07-16

### Fixed
- `makakoo update` on curl-pipe installs now refreshes the binary and bundled assets only (`MAKAKOO_NO_AUTORUN=1`); it no longer re-runs the full `makakoo install` umbrella — distro reconcile plus interactive wizard — on a machine that is already set up.
- Distro install no longer fails the whole distro (and with it `makakoo install`/`makakoo update`) when a plugin's directory exists on disk without a lock entry, e.g. after an out-of-band auto-update. Such plugins are skipped with a warning and a reconcile hint.
- `makakoo plugin update` for path-sourced plugins backs up the installed tree before the uninstall+reinstall round-trip and restores it (tree + lock entry) when the reinstall is refused — a security-gate refusal no longer strands the user without the plugin.

### Changed
- The setup wizard's brain picker now always shows the real, expanded default Brain folder (e.g. `/Users/you/MAKAKOO/data/Brain`) in every user-facing line — header, Obsidian vault question, and closing summary — instead of the `$MAKAKOO_HOME/data/Brain` literal, so the path can be pasted straight into Obsidian or a file manager. The symbolic form remains in the stored config and is labeled as such.
- Brain picker copy: consistent Makakoo naming in write-permission questions, an explicit `(Enter = skip)` on the optional plain-folder prompt, and closing hints that point at `makakoo brain list|add|remove` and `makakoo setup brain`.

## [0.1.39] - 2026-07-16

### Added
- Added `makakoo brain ingest <files|folders> --out <bundle>` — build a portable OKF v0.1 bundle from arbitrary local Markdown (loose files, folders, or a mix). The output round-trips through `makakoo brain validate` by construction; ingest never registers or indexes on its own.
- Added optional `aspect_ratio` to the `harvey_generate_image` MCP tool.

### Fixed
- `harvey_generate_image` now works through Codex and switchAILocal without persisting secret values in CLI configuration: the Codex adapter forwards selected environment variable *names* only, and `LlmClient` accepts the `SWITCHAI_KEY` / `LLM_API_KEY` / `LLM_BASE_URL` aliases.
- Image generation handles the MiniMax URL/JPEG response shape (`data.image_urls[0]`) alongside the existing OpenAI base64 shape, and reports the detected `mime_type` honestly (`png_bytes_b64` alias kept for actual PNG payloads only).

## [0.1.38] - 2026-07-15

### Added
- Added native `makakoo brain list`, `add`, `remove`, `export`, and `validate` commands for managing enrichment sources and exchanging local [Open Knowledge Format](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md) v0.1 bundles.
- Added read-only OKF enrichment to Superbrain. `makakoo sync` indexes valid concepts, source-qualified Markdown relationships, and `type` metadata without moving or rewriting the imported bundle.
- Added local OKF export with journal opt-in, JSON reports, generated progressive-disclosure indexes, and a public allowlist that refuses credential-shaped content.

### Changed
- Hardened Brain source registry writes and forced export replacement with cross-process locks, owned recovery markers, verified backups, overlap checks, and fail-closed collision handling.
- Updated the public docs, Brain guide, troubleshooting index, skills, and command manual so users and AI agents route through the native OKF workflow.

### Fixed
- Fixed OKF file durability on Windows by syncing staged files through writable handles, and made `$HOME` source paths resolve through the platform home directory when Windows does not expose a `HOME` environment variable.

## [0.1.37] - 2026-06-28

### Added
- Added `skill-dev-lazy-build`, a Makakoo-native YAGNI/minimality discipline adapted from the useful Ponytail prompt pattern. It gives Harvey a reusable ladder for building the smallest safe change, reviewing over-engineering, and fixing shared root causes without dropping security, validation, accessibility, error handling, or explicit requirements.
- Bundled `skill-dev-lazy-build` in the Sebastian distro for dogfood before wider default promotion.

## [0.1.36] - 2026-06-23

### Fixed
- Fixed `agent-browser-harness` against upstream `browser-harness` v0.1.3 by supporting the packaged `src/browser_harness/` layout, restoring compatibility shims for older MCP children, and documenting the required `--core` install form.
- Restored the documented `harvey_browse` helper aliases (`goto`, `read`, `click`, `fill`, `screenshot`) via the plugin agent workspace so existing Makakoo snippets keep working after the upstream helper rename.

## [0.1.35] - 2026-06-23

### Fixed
- Removed the confusing unpinned-blake3 warning from trusted bundled core plugin installs while keeping user-installed local/git/tar plugins noisy unless they are explicitly pinned.
- Clarified the remaining unpinned-plugin warning so user-installed plugins are told to pass `--blake3` or pin distro/source metadata instead of editing a self-referential manifest hash.

## [0.1.34] - 2026-06-23

### Fixed
- Fixed plugin install security scans so install-time SkillSpector checks always scan the current staged plugin bytes instead of reusing a same-day cached report. This prevents stale findings from blocking fixed plugins and prevents stale safe reports from approving changed plugin code.

## [0.1.33] - 2026-06-23

### Added
- Added an optional Brain enrichment layer for separate Obsidian, Logseq, and plain-Markdown sources. The canonical Brain remains `$MAKAKOO_HOME/data/Brain`; enrichment sources are indexed with source labels and normal journal writes stay canonical.
- Added Obsidian metadata extraction for tags, aliases, and Canvas graph hints so Superbrain search can surface richer context without treating external vaults as the source of truth.

### Changed
- Improved `makakoo setup brain` defaults: the canonical Makakoo Brain folder (`$MAKAKOO_HOME/data/Brain`) now counts as a completed setup, Obsidian app detection/install runs before separate-vault prompts, and declining Obsidian install cleanly skips Obsidian setup instead of asking for a vault path.
- Made remaining setup picker prompts explicit about defaults: empty persona-name input selects suggestion 1, and empty model-provider input skips instead of erroring.
- `makakoo update` / `makakoo upgrade` now offer an interactive, default-No setup review after successful updates so new defaults can be checked without forcing the full wizard on routine upgrades.

### Fixed
- Fixed `agent-browser-harness` SkillSpector preflight noise by keeping its Chrome doctor probe fixed to the documented local CDP endpoint instead of accepting an environment-provided URL.

## [0.1.32] - 2026-06-21

### Added
- Added `makakoo update` as the primary self-update command, keeping `makakoo upgrade` as a legacy alias.
- Added `makakoo setup updates` plus the bundled `sancho-task-makakoo-update` task so fresh Core setup can default to automatic 24h Makakoo OS updates while existing installs stay idle until `config/updates.toml` exists.
- Added `makakoo setup lope`, an explicit Lope installer/pitch that clones or updates `~/.lope` and registers Lope skills/commands into detected AI CLI hosts after consent.

### Changed
- Improved the Brain setup picker so missing Obsidian now offers an app install when a supported package manager is available, instead of only printing manual instructions.
- Refreshed the setup/manual/skill docs for `makakoo agent create`, `makakoo sync`, `makakoo memory`, Headroom-by-default, and the distinction between infected AI hosts and chat transports.

## [0.1.31] - 2026-06-20

### Added
- Added `makakoo agent create <slot> --runtime flue` — scaffolds a runnable [Flue](https://flueframework.com) (TypeScript) channel agent next to the native slot. Makakoo keeps the control plane (identity, scope, secrets, registry) while Flue runs the data plane (agent loop + Telegram webhook); a generated `mcp-proxy.mjs` bridges the local `makakoo-mcp` stdio server to StreamableHTTP so the agent consumes every Makakoo tool as `mcp__harvey__*`. New `--out` flag sets the scaffold dir (default `$MAKAKOO_HOME/agents-flue/<slot>`). See `docs/walkthroughs/flue-telegram-bot.md`.

### Changed
- `makakoo agent validate` and the create-time credential check are now **config-only** (parse + resolve secret references); they no longer make a live network probe to the transport API.
- Retired the never-assembled in-process Rust transport runtime (router/outbound/pairing/whatsapp/web/voice_twilio/email modules) that only ever ran under `#[cfg(test)]`; the live message loop runs in the Python harveychat gateway. The locked wire schema (`transport/frame.rs` + `ipc/`) and the telegram/slack/discord adapters are unchanged.

## [0.1.30] - 2026-06-20

### Added
- Install scripts (`install.sh` / `install.ps1`) now verify the sha256 of each artifact before unpacking and **fail closed** on a missing or mismatched sidecar.
- The release workflow publishes a combined `SHA256SUMS` manifest alongside the per-artifact `.sha256` sidecars.
- Governance: added `CODEOWNERS`, `docs/MAINTAINERS.md`, and routed security reports through `SECURITY.md`.

### Changed
- Hardened CI: SHA-pinned all third-party GitHub Actions, and pinned rustfmt + clippy (`-D warnings`) to rustc 1.95.0.
- Added a gitleaks secret-scanning gate with an audited allowlist (0 live secrets).

## [0.1.29] - 2026-06-14

### Added
- Added `agent-harveychat` to the federation distro so remote Makakoo nodes can run Telegram/HarveyChat bodies like Donna once tokens and allowlists are configured.
- Added an install bootstrap for `agent-harveychat` that creates a plugin-local Python venv and installs pinned Telegram/chat dependencies on fresh installs.

### Changed
- Promoted HarveyChat to a persona-aware gateway: channel personas such as Donna now override the global Harvey bootstrap, status/start/access messages use the configured persona, and Cortex Memory can inject a larger bounded memory block for long-context chats.
- Kept experimental chat workflows opt-in via `HARVEYCHAT_WORKFLOWS=1` so Telegram answers direct questions with Brain/tools instead of false-positive “working on it” workflow acknowledgements.
- Plugin installs now skip common non-runtime artifact directories such as `tests/`, `node_modules/`, and `target/` before SkillSpector scans and promotion.

### Fixed
- Fixed HarveyChat release installs on Python 3.12 Linux hosts by removing the stale `python3.11` entrypoint assumption and adding installed-layout imports for `lib-harvey-core`/`lib-hte`.
- Fixed HTTP client logging so Telegram Bot API URLs containing bot tokens are not emitted at normal gateway INFO level.
- Fixed Superbrain FTS queries for hyphenated terms like `makakoo-vps`; hyphens are now treated as separators so prefix matches no longer trigger FTS5 `no such column` errors.

## [0.1.28] - 2026-06-14

### Changed
- Improved Brain Network discoverability in the top-level docs and getting-started/use-case guides.
- Taught the Brain Network skill and user manual to route plain-language requests like “connect my Mac Brain with my VPS Brain” into safe `makakoo network` flows.

## [0.1.27] - 2026-06-14

### Fixed
- Fixed fresh Brain Network activation so the legacy `harvey-listen.js` sidecar stays dormant unless explicitly enabled, while the signed MCP HTTP shim starts and reports healthy for remote Brain reads.

## [0.1.26] - 2026-06-14

### Added
- Added the opt-in `federation` distro so users can install Brain Network on any Makakoo node with `makakoo distro install federation`.
- Added `makakoo network` as the safe Brain federation control plane for activating Octopus peers, registering endpoints, exchanging trust, and running origin-tagged remote Brain searches.
- Added Brain Network user-manual and walkthrough docs for Harvey laptop, Donna VPS, Tytus pods, and future Makakoo-to-Makakoo setups.

### Changed
- Made `agent-octopus-peer` default to loopback instead of public bind, and made Brain Network activation write persistent listener env before restarting the agent.

### Fixed
- Fixed manual plugin installs when SkillSpector writes a valid high-risk report but exits nonzero: Makakoo now parses the report and shows the policy block instead of a scanner infrastructure error.
- Fixed `agent-octopus-peer` release installs by resolving `makakoo-mcp` from `~/.local/bin` as well as cargo/Homebrew paths and passing that path into launchd/systemd.
- Fixed `agent-octopus-peer` lifecycle entrypoints so `makakoo agent start|stop|health agent-octopus-peer` runs from installed plugin directories.
- Fixed fresh Brain Network installs by bootstrapping Octopus' `cryptography` dependency into `lib-harvey-core`'s venv and routing Octopus/Brain Network commands through that venv.

## [0.1.25] - 2026-06-13

### Fixed
- Fixed Linux `switchailocal_watchdog` SANCHO ticks by removing unconditional macOS `launchctl` calls and using platform-specific service/process diagnostics.
- Fixed the pi auto-update task to track the installed `@earendil-works/pi-coding-agent` npm package instead of the stale `@mariozechner/pi-coding-agent` name.

## [0.1.24] - 2026-06-13

### Fixed
- Improved `makakoo setup brain` Obsidian handling: detects whether the Obsidian app is installed, explains install steps when missing, treats `n/no/skip` at the vault path prompt as cancellation, and no longer registers nonexistent vault paths unless explicitly confirmed.

## [0.1.23] - 2026-06-13

### Fixed
- Added the missing troubleshooting index entry for the fresh-install `switchailocal` adapter bootstrap error so docs verification stays green.

## [0.1.22] - 2026-06-13

### Fixed
- Added `skill-brain-multi-source` to the core distro so fresh installs provide the Brain picker that `makakoo setup brain` launches.
- Fixed bundled adapter discovery from release installs by checking `<prefix>/share/makakoo/plugins-core/adapters`, not only checkout paths.
- Made `makakoo setup model-provider` bootstrap the bundled `switchailocal` adapter and set it as primary on fresh installs instead of ending with a failed setup section.
- Allowed `makakoo adapter install <name> --bundled` to install release-bundled adapter manifests without requiring an extra `--allow-unsigned` flag.

## [0.1.21] - 2026-06-13

### Fixed
- Made `makakoo upgrade --reinfect` perform a real `makakoo infect --global` refresh followed by `makakoo infect --verify`, so releases that change CLI bootstrap fragments actually update every infected host.
- Updated upgrade docs and user manuals with the exact VPS/server upgrade sequence: `makakoo upgrade --method curl-pipe --reinfect`, `makakoo daemon restart`, then restart AI CLI sessions.
- Fixed the curl-pipe upgrade path to invoke the Bash installer with Bash instead of Ubuntu `/bin/sh`, preventing `set: Illegal option -o pipefail` on servers.

## [0.1.20] - 2026-06-13

### Fixed
- Fixed bootstrap fragment rendering so non-`bootstrap-fragment` plugins with `[infect.fragments]`, including `tool-headroom`, actually inject their instructions into every infected CLI.
- Made `makakoo infect` re-render the bootstrap from the live plugin registry instead of reusing a stale `config/bootstrap-cache.md`, so newly installed fragments activate immediately on existing machines.

## [0.1.19] - 2026-06-13

### Fixed
- Expanded `tool-headroom` MCP registration beyond upstream Claude-only install support. Fresh Makakoo installs now register the Headroom MCP server for detected Claude, Gemini, Codex, OpenCode, Vibe, Qwen, and Cursor hosts using each host's native config shape.

## [0.1.18] - 2026-06-13

### Fixed
- Prevented `makakoo secret get` from opening OS keychain GUI prompts in non-interactive/background agent shells. The command now reads a same-named env var first, fails fast when a prompt would be unsafe, and only allows automation prompts through explicit `MAKAKOO_SECRET_ALLOW_KEYCHAIN_PROMPT=1`.

## [0.1.17] - 2026-06-13

### Fixed
- Added Docker-native fallback to `tool-headroom` so fresh installs can still install Headroom on hosts where the Python package cannot build native dependencies.
- Added a Docker-native Claude MCP stdio shim for Headroom so Claude can launch the MCP server from Docker-backed installs.

## [0.1.16] - 2026-06-13

### Added
- Bundled `tool-headroom` in the core distro so fresh installs get Headroom MCP context compression for large tool outputs.
- Added a Headroom bootstrap fragment and skill guidance for safe compress/retrieve behavior across infected CLIs.

### Fixed
- Included `skill-meta-caveman-voice` in the core distro plugin set so the `voice = "caveman"` default is backed by an installed plugin on fresh systems.

## [0.1.15] - 2026-06-12

### Fixed

- Fixed `agent-switchailocal` install on servers where npm no longer supports `npm bin -g` and SSH/non-login shells do not include npm's global prefix bin directory on `PATH`.


## [0.1.14] - 2026-06-12

### Fixed

- Fixed fresh `makakoo install` on clean machines by skipping SkillSpector scans for bundled distro plugins that already ship inside the release archive. Manual plugin installs and remote plugin sources still use the security gate.


## [0.1.13] - 2026-06-12

### Added

- Added identity capture to the core setup flow so new installs can persist both the assistant persona name and the human user name globally across infected CLIs.
- Added the persona capture and registry bootstrap plugins to the core distro so names chosen during setup become part of Makakoo OS identity state instead of one CLI session state.

### Fixed

- Fixed infected Codex/OpenCode/Vibe MCP configuration to prefer the installed `makakoo-mcp` next to the active `makakoo` binary or on `PATH`, avoiding stale `$HOME/.cargo/bin/makakoo-mcp` paths on VPS installs.
- Fixed setup and plugin-install tests so CI no longer shells out to host package managers or real SkillSpector bootstrap paths while validating setup status and risk-gate behavior.


## [0.1.12] - 2026-06-08

### Added

- Added a SkillSpector security gate for plugin installs. `makakoo plugin install` now scans staged plugin source before promotion, blocks high/critical findings by default, writes per-plugin risk metadata, and supports explicit reviewed overrides with `--allow-risk --risk-ack`.
- Added `makakoo skill audit <target>` for manual SkillSpector scans with terminal, JSON, SARIF, and audit-log output.
- Added `makakoo skill audit --all [--limit N]` for fleet audits across installed plugins and local skill roots, with dated JSON/Markdown summary reports.
- Added documentation for macOS SkillSpector prerequisites, report locations, false-positive handling, override policy, and experimental `--llm` semantic triage.

## [0.1.11] - 2026-06-07

### Fixed

- Disabled background Brain vector embedding by default in native SANCHO and legacy Python handlers. This stops daemon boot from spawning Ollama `qwen3-embedding:0.6b` / `llama-server` and saturating local CPU.
- Kept Brain writes immediately searchable through FTS/entity sync while making embedding refresh opt-in via `MAKAKOO_ENABLE_BACKGROUND_EMBED_SYNC=1`.

## [0.1.10] - 2026-05-20

### Added

- Added Agent Sessions v1: `makakoo agent-session open/list/status/eval/read/gate/gates/close` for durable child-agent work records without flooding parent context.
- Added `makakoo handle read` for bounded reads from `agent-artifact://...` handles with summary, head, tail, section, and small JSONPath projections.
- Added persistent `$MAKAKOO_HOME/data/agent_sessions.db` storage for sessions, events, artifacts, and verification gates.
- Added Agent Sessions documentation in the agents catalog and user manual.

### Changed

- Session names are validated as labels, active duplicate names are rejected by SQLite, and verification gates persist full stdout/stderr behind handles.

## [0.1.9] - 2026-05-17

### Added

- Added the first public Mascot GYM autoresearch loop: fixed eval cache, simplicity scoring, Polar Express constants, Muon optimizer port, universal artifact handlers, and SANCHO maintenance tasks.
- Added release-readiness smoke tests for lazy Muon imports, GYM task registration, Makakoo-home-aware eval cache writes, optional PyYAML behavior, and `gym_cascade` subprocess dispatch.

### Fixed

- Registered the new GYM SANCHO tasks in the in-process scheduler and added the missing `gym_cascade` handler so plugin task declarations no longer point at dead handlers.
- Made GYM imports safe on fresh user machines by keeping torch and PyYAML optional until the specific runtime path needs them.
- Made new GYM paths honor `MAKAKOO_HOME` / `HARVEY_HOME` instead of writing only under `~/MAKAKOO`.
- Fixed the GYM snapshot handler's UTC timestamp crash and removed a TOML round-trip dependency on non-existent `tomli.dumps`.

## [0.1.8] - 2026-05-16

### Fixed

- Made `makakoo upgrade` treat an already-current package-manager no-op as a successful up-to-date result instead of exiting with a scary warning.
- Made `makakoo upgrade --method brew` reuse the detected Homebrew prefix when the current install is already Homebrew, so forced dry-runs report `/usr/local` vs `/opt/homebrew` correctly.

## [0.1.7] - 2026-05-16

### Added

- Added `makakoo daemon restart` as a first-class command. It re-registers the daemon service descriptor before starting it so Homebrew/curl-pipe upgrades do not leave launchd/systemd pointing at an old binary path.

### Fixed

- Fixed `makakoo upgrade` daemon restart guidance to print `makakoo daemon restart` instead of stale platform-specific commands or documentation-only commands.
- Updated daemon/upgrade documentation so public beta users get one working restart command after upgrades.

## [0.1.6] - 2026-05-15

### Fixed

- Made the public installer fail hard on aborted or failed distro/plugin installs instead of printing a false-success completion.
- Installed core distro plugins in dependency order so bundled plugins can rely on `lib-harvey-core`.
- Made `agent-browser-harness` degrade cleanly when Python 3.11+ is missing, with a re-run command after Python is installed.
- Hardened macOS, Linux, and Windows smoke workflows so public install tests assert a real successful distro summary.
- Clarified public platform support: macOS arm64/x64, Linux arm64/x64, Windows x64; Windows ARM64 is explicitly unsupported until assets ship.

### Changed

- Narrowed executable documentation verification to public quickstart/getting-started docs while long walkthroughs wait for hermetic fixtures.
- Updated makakoo.com install copy and GitHub links for the public beta.

## [0.1.5] - 2026-05-08

### Changed

- **Installer hands off to the setup wizard automatically.** Both
  `install.sh` (Linux/macOS) and `install.ps1` (Windows) now exec
  `makakoo install` at the end of the install — which itself runs
  the core distro install, registers the daemon, infects every
  detected AI CLI, and lands the user in the interactive `makakoo
  setup` wizard on success. Re-attaches `/dev/tty` so `curl … | sh`
  pipes still get an interactive wizard rather than a silent
  non-TTY skip.
  - Opt out: set `MAKAKOO_NO_AUTORUN=1` (sh) or
    `$env:MAKAKOO_NO_AUTORUN = "1"` (ps1) before invoking. Used by
    `smoke.yml` / unattended CI.
  - Removed the old "next steps: `makakoo infect --global` /
    `makakoo daemon install`" copy from the installer's tail —
    `makakoo install` already covers both.
  - Files: `distribution/install.sh:93`, `install/install.ps1:170`.

## [0.1.4] - 2026-05-02

### Fixed

- **`makakoo upgrade` Homebrew detection on real installs.** Live smoke
  on a Homebrew install (`brew install traylinx/tap/makakoo`) showed the
  detector classified the binary as `Unknown` and refused to upgrade.
  Root cause: `canonicalize()` resolves `/usr/local/bin/makakoo` to
  `/usr/local/Cellar/makakoo/<ver>/bin/makakoo`, and the matcher only
  checked `<prefix>/bin/`. Widened to also recognise `<prefix>/Cellar/`
  paths (`makakoo-core/src/upgrade/detect.rs:82`). +2 regression tests
  covering both Intel + Apple Silicon canonical Cellar paths. v0.1.3 was
  the first release containing the upgrade verb at all, so brew users
  hit this on every invocation.

## [0.1.3] - 2026-05-02

Cuts a release for the three sprints that landed after `v0.1.2` was tagged
on 2026-05-01: `SPRINT-MAKAKOO-UPGRADE-VERB`, `SPRINT-KIMI-ADAPTER`, and the
`winget` manifest version bump. No new functionality beyond what is captured
in those sections — this release just gets the bits onto user machines.

### Added — Kimi adapter (`SPRINT-KIMI-ADAPTER`, 2026-05-01)

Kimi (`@moonshotai/kimi-cli`) joins the `makakoo infect` roster as the
9th supported AI CLI. Previously infected manually via a hand-written
`agent.yaml`; now `makakoo infect --target kimi` (or `makakoo infect`
to hit all slots) writes/upgrades the file automatically alongside
the other 8 hosts.

- **`SlotFormat::KimiYaml`** in `makakoo/src/infect/slots.rs`. New
  variant alongside the existing `Markdown` + `OpencodeJson` formats.
  The Kimi slot lives at `~/.kimi/agents/makakoo/agent.yaml` and
  follows Kimi's "named-agent" pattern (one directory per agent
  with a single `agent.yaml`). The bootstrap occupies
  `agent.system_prompt_args.ROLE_ADDITIONAL` — wrapped in the same
  `<!-- harvey:infect-global START v12 --> ... END -->` markers
  used by markdown slots, so versioned in-place upgrades work
  identically.
- **`write_kimi_yaml` + `remove_kimi_yaml`** in
  `makakoo/src/infect/writer.rs`. Round-trip the YAML via `serde_yml`,
  ensure scaffolding (`version: 1`, `agent.name: "Harvey"`,
  `agent.extend: default`, `agent.system_prompt_args: {}`) on first
  install, and crucially **preserve any other `agent.*` keys the
  user has set** (`model`, `when_to_use`, custom names) on
  rewrites. Re-uses the existing `upsert_markdown_block` /
  `find_prior_version` machinery so the YAML path benefits from
  the same in-place upgrade logic the markdown slots use.
- **9-slot SLOTS table.** Kimi is the 9th entry. The order matches
  `plugins-core/lib-harvey-core/src/core/orchestration/infect_global.py`
  exactly (Python mirror also updated — `HostType.KIMI` added to
  the host-detector enum + a ppid-based detection signal at 0.85
  confidence).
- **`detect.rs` BINARIES table** gained `("kimi", "kimi")` and
  `("pi", "pi")` (closes a long-standing gap where `pi` was in
  SLOTS but not in BINARIES — `binary_for("pi")` silently returned
  `None`). The `probe_covers_canonical_slots` test now requires
  `kimi` alongside the other 8.

Test counts: workspace test suite 1776 → +9 net new for the kimi
adapter (5 in `writer.rs::tests::*kimi*`, 4 updated in
`infect::tests` + `slots::tests` + `detect::tests`). All 1785+
pass.

Live smoke against `~/.kimi/agents/makakoo/agent.yaml` confirmed:
the manually-installed bootstrap survives in place above the
marker-bracketed block, and re-running `makakoo infect` flips
status to `unchanged`. Future runs will replace only the
marker-bracketed region — the manual leftover above it is
harmless and the user can delete it by hand if they want a tidy
file.

### Added — `makakoo upgrade` self-update verb (`SPRINT-MAKAKOO-UPGRADE-VERB`, 2026-05-01)

Self-upgrade for the `makakoo` + `makakoo-mcp` binaries. Detects the
install method by inspecting the running binary's path, dispatches to
the matching update command, and prints the version delta plus a
platform-specific daemon-restart hint. Closes the gap surfaced after
SPRINT-PATTERN-SUBSTRATE-V1 shipped — users now have a single command
that picks the right upgrade path instead of memorizing
`cargo install --path` vs `brew upgrade` vs `curl-pipe`.

- **`makakoo upgrade` CLI verb** with flags:
  - `--dry-run` — print the upgrade plan without spawning anything.
  - `--reinfect` — after a successful upgrade, run
    `makakoo infect --verify --repair` to refresh bootstrap fragments
    in every infected CLI / IDE host.
  - `--method <cargo|brew|curl-pipe>` — override the detector (rare).
  - `--source <path>` — for Cargo upgrades, point at a local source
    checkout (overrides `MAKAKOO_SOURCE_PATH` env). Defaults to
    `cargo install --git https://github.com/makakoo/makakoo-os`.
  - `--install-script-url <url>` — for curl-pipe upgrades, override
    the default `https://makakoo.com/install.sh`. Refuses non-HTTPS.
  - `--only-kernel` / `--only-mcp` — upgrade just one of the two
    binaries (mutually exclusive). Default: both.
- **Install-method detector** at `makakoo-core/src/upgrade/detect.rs`.
  Resolves symlinks via `canonicalize`, then maps the running binary's
  path to `Cargo` (`~/.cargo/bin/`), `Homebrew`
  (`/opt/homebrew/`, `/usr/local/`, `/home/linuxbrew/.linuxbrew/`),
  `CurlPipe` (`$MAKAKOO_PREFIX/bin/`, default `$HOME/.local/bin/`), or
  `Unknown`. Dev builds running from `target/debug/` or
  `target/release/` are explicitly classified as `Unknown` with a
  message pointing the user at `cargo install --path`.
- **Per-method upgrade dispatchers** at `makakoo-core/src/upgrade/dispatch.rs`.
  Plans actions purely (so `--dry-run` shares the same code path), then
  spawns subprocesses sequentially. First failure aborts the chain.
- **Version delta** captured by re-running `makakoo version` before and
  after; if unchanged, the verb exits 1 with a warning. Uses text
  parsing of the existing `version` command output — no new `--json`
  flag in v1.
- **Daemon restart hint** at `makakoo-core/src/upgrade/verify.rs` —
  initial v0.1.3 behavior printed platform-specific commands after a
  successful upgrade. This was superseded in v0.1.7 by the first-class
  `makakoo daemon restart` command.

Test counts: 1893 passed / 0 failed / 5 ignored (workspace), +35 net
new tests across `upgrade::detect`, `upgrade::dispatch`,
`upgrade::verify`, plus the renamed `probe_covers_canonical_slots`
test (was `probe_covers_all_eight_slots` — pinned a hard "8" count
that tripped over recently-added Kimi support; rewritten to assert
required slots are present without pinning a count).

Out of v1 scope (queued for follow-up sprints): auto `daemon restart`,
upgrade rollback, beta-channel selection, scheduled auto-upgrade,
`makakoo version --json` flag.

## [0.1.2] - 2026-05-01

> **Note on the v0.1.1 tag.** A release named `v0.1.1` was published
> 2026-04-27 pointing at the docs-mcp Phases A–F commit
> ([`9d905bf`](https://github.com/makakoo/makakoo-os/commit/9d905bf)).
> That release shipped before the workspace `Cargo.toml` `version`
> field was bumped, so binaries from that release self-report as
> `makakoo 0.1.0`. v0.1.2 is the first release where
> `makakoo --version` reports the same string as the git tag — and
> the first to roll up the post-Phase-F polish (Pattern substrate
> v1, setup wizard, security lockdown).

### Added — Docs MCP server (`SPRINT-MAKAKOO-DOCS-MCP`, 2026-05-01)

Makakoo OS docs are now queryable from any AI CLI in real time, with
citations linking back to the source markdown. Modeled on Microsoft's
`azure-docs` pattern + Google's Firebase MCP. Bundled into the main
`makakoo` binary — users add one MCP entry, no separate install.

- **`makakoo docs-mcp --stdio` subcommand.** New stdio JSON-RPC MCP
  server exposing four tools that any AI CLI (Claude Code, Gemini,
  OpenCode, Cursor, Codex, Qwen, Vibe) can call:
  - `makakoo_docs_search(query, limit?)` — BM25 full-text search over
    the indexed corpus, returns `[{path, title, snippet, score}]`.
  - `makakoo_docs_read(path)` — full markdown content for a path
    surfaced by a prior search/list call.
  - `makakoo_docs_list(prefix?)` — directory-style listing with size
    + title per entry.
  - `makakoo_docs_topic(name)` — resolves a topic keyword (e.g.
    `agent`, `infect`, `brain`) to its canonical doc plus breadcrumb
    + sibling related docs.
- **117 markdown files baked into the binary** (~822 KB) at build
  time via `build.rs` → SQLite FTS5 with `porter unicode61`
  tokenizer. `include_bytes!` embeds the index file so cold queries
  work offline with zero setup.
- **`makakoo docs update [--from-github] [--from-branch <branch>]`.**
  Pulls the latest `docs/` + `spec/` from
  `github.com/makakoo/makakoo-os` (default `main`, override per
  flag), rebuilds the FTS5 index, and writes
  `~/.makakoo/docs-cache/index.db`. The MCP server prefers the
  cache when present and falls back to the baked-in corpus —
  build-pinned lower bound, user-refreshable upper bound.
- **Standalone `makakoo-docs-mcp` binary** (workspace member, same
  source) for users who prefer to wire the MCP server directly
  without going through `makakoo`. Both invocation paths are
  byte-identical.
- **Setup doc at `docs/docs-mcp-setup.md`** with config snippets
  for all 7 supported AI CLIs (Claude Code, Gemini, OpenCode,
  Cursor, Qwen, Vibe, Codex), `--update` workflow, citation
  format, and troubleshooting.
- **13 user-manual stubs deepened** to ~60 lines each (target locked
  by lope verdict Q1, 2026-04-27): `makakoo-{adapter,completion,
  daemon,distro,infect,mcp,plugin,query,sancho,search,secret,status,
  uninfect}.md`. Search snippets land on real prose now, not 19-line
  `--help` shims.

Test counts: 6/6 passing in `makakoo-docs-mcp` (`search` /
`read` / `list` / `topic` round-trip + cache-prefer fallback).

Out of v1 scope (queued): Tytus docs vendoring (Q2 verdict locked
to "index existing 3 files" — wiring deferred pending build-time
network policy decision); on-disk index versioning beyond
`built_for_version`; per-tool rate limiting.

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
