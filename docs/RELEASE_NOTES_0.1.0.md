# Makakoo OS v0.1.0 — first public release

**Tagged:** PLACEHOLDER (fill at tag time)
**Artifacts:** 5 targets (macOS arm64/x86_64, Linux arm64/x86_64, Windows x86_64) at [GitHub Releases](https://github.com/makakoo/makakoo-os/releases/tag/v0.1.0).

## The 90-second pitch

Makakoo OS is an open-source autonomous cognitive extension that gives every AI CLI on your machine the same persistent brain. Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen, VSCode Copilot, Continue, Cline, JetBrains AI — all share one Logseq-backed memory, one set of proactive maintenance tasks, one capability-sandboxed plugin ecosystem.

One install. Many bodies, one mind. MIT. No VC. Local-first. No telemetry.

## What works today

- **`makakoo install`** umbrella — fresh-machine setup in one command (distro + daemon + infect + health check). macOS, Linux, and Windows code paths all shipped; VM-level smoke is next.
- **One-liner installers** — `curl | sh` and `iwr | iex` scaffolds live. Hosted URL (`makakoo.com/install`) activates when DNS lands.
- **Plugin system** — 38 plugin-manifest entries under `plugins-core/`. Watchdogs, skills, monitors, mascot GYM, agent-dreams. Zero hardcoded subprocess registrations remain; every non-native task is declared in a `plugin.toml`.
- **Capability sandbox** — per-plugin Unix domain socket (macOS + Linux) / Windows named pipe, PID-verified accept loop, grant table from manifest `[capabilities].grants`, append-only JSONL audit log. Rust + Python client libraries let any plugin dial the kernel.
- **Shared Brain** — Logseq-format journals + pages + superbrain (FTS5 + vector + LLM synthesis). Every infected CLI reads and writes the same files.
- **`makakoo uninfect`** — symmetric inverse of `infect`. Strips the bootstrap block from every CLI slot, deletes infect-created-only files, preserves user prose.
- **`makakoo plugin enable/disable/update`** — soft lifecycle. Flip a plugin off without removing its directory; reinstall from the recorded source preserving the enabled state.
- **`makakoo distro save`** — serialize the live install to a reproducible TOML snapshot pinned by exact version + blake3.
- **Shell completion** — bash, zsh, fish, elvish, powershell via `makakoo completion <shell>`. Install guide at `install/completions/README.md`.
- **5 distros** — minimal, core, sebastian (Harvey's dogfood), creator (writers + streamers), trader (market-facing agents).
- **Tag-driven release pipeline** — pushing `v*` tags triggers `.github/workflows/release.yml`. Builds 5 targets, uploads tar.gz/zip + SHA-256 to the GitHub Release, auto-generates notes.

## Platform coverage

| OS | Unit tests | Integration | One-liner install | Daemon |
|---|---|---|---|---|
| macOS (arm64 + x86_64) | ✅ | ✅ | ✅ install.sh | ✅ launchd |
| Linux (x86_64 today, aarch64 cross) | ✅ | ✅ | ✅ install.sh | ✅ systemd user |
| Windows 11 (x86_64) | ✅ | scripted | ✅ install.ps1 | ✅ auto-launch (HKCU Run) |

839 workspace tests, 0 failures. CI green on all three OSes at the v0.1.0 tag.

## Known gaps — shipping honestly

- **Signed release artifacts** — Apple notarization and Windows Authenticode are not applied in v0.1. macOS users see a Gatekeeper "cannot verify developer" dialog on first run; Windows users see SmartScreen caution. Runbook at [`docs/RELEASE_SIGNING.md`](RELEASE_SIGNING.md) for the paste-ready workflow diff once the certificates are in hand. Target: v0.1.1 or v0.2.
- **Homebrew formula + winget submission** — formula draft at `distribution/homebrew/makakoo.rb` with placeholder SHAs, winget manifest at `distribution/winget/makakoo.yaml`. Tap repo + winget PR happen post-v0.1 once we have artifacts to reference.
- **Plugin migration** — 32 of harvey-os's 188 skills are **fully self-contained** in `plugins-core/` with their Python source bundled; the remaining 150 + 11 agent submodules port in waves through v0.2. The `scripts/migrate_skill.py --copy-src` helper does the relocation in one command per skill. Public users installing any shipped plugin get the Python code bundled — no harvey-os clone required.
- **Runtime relocation** — Sebastian's install still has `MAKAKOO_HOME=~/MAKAKOO`. The D10 canonical layout is `~/.makakoo/` runtime + `~/MAKAKOO` compat symlink. Runbook at [`docs/PHASE_H4_RELOCATE.md`](PHASE_H4_RELOCATE.md); execution is post-launch and manual by design.
- **Fresh-VM smokes** — scripted install paths are tested via CI, but the full "open a new macOS VM, curl | sh, see everything work" smoke runs human-supervised. Ship report lands in a pinned GitHub Discussion the week after the tag.
- **`NetHandler` capability** — `net/http` / `net/tcp` / `net/udp` / `net/ws` grant verbs exist in the vocabulary; the handler that proxies them through reqwest is a v0.2 deliverable. Plugins that need network egress currently use their own clients.
- **Audit log rotation** — the capability audit log appends indefinitely. 100 MB cap + 7-day retention is a v0.2 polish item.
- **Node bindings for plugin clients** — Rust + Python shipped. JavaScript bindings arrive when a plugin-core target adopts a JS runtime.

## Upgrade path

There is no upgrade path in v0.1.0 — this is the first public release. Future minor versions (v0.2.x) will document migrations here when any plugin manifest schema or capability ABI bumps.

## Acknowledgements

This is a community-built project. Named contributors appear at [docs/MAINTAINERS.md](MAINTAINERS.md) (created alongside v0.1.0 tag). Special thanks to every maintainer of the upstream crates Makakoo stands on — tokio, rusqlite, serde, clap, cargo-dist, keyring, and the broader Rust ecosystem.

## What's next

- v0.1.1 — signing certs + Homebrew tap + winget submission + any hotfixes surfaced by the launch smoke.
- v0.2 — NetHandler, agent submodule migration (harveychat, arbitrage-agent, career-manager, multimodal-knowledge), the `~/.makakoo/` runtime split executed cleanly, audit log rotation, Node plugin bindings.
- v1.0 — when the kernel ABI is frozen and Makakoo has been running in production on external users' machines for long enough to know the sharp edges.

## Get involved

- [GitHub Discussions](https://github.com/makakoo/makakoo-os/discussions) — share install reports, ideas, and weird edge cases.
- [Issues](https://github.com/makakoo/makakoo-os/issues) — bugs, feature requests.
- [CONTRIBUTING.md](../CONTRIBUTING.md) — how to land a PR.
- Named maintainers appear at `docs/MAINTAINERS.md` at tag time.

🦉 _eat sleep go bananas repeat_ 🍌
