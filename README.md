# Makakoo OS

[![CI](https://github.com/makakoo/makakoo-os/actions/workflows/ci.yml/badge.svg)](https://github.com/makakoo/makakoo-os/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-blue)
![Status](https://img.shields.io/badge/status-v0.1.0%20launch-orange)

> **Many bodies. One mind.**
> Open-source autonomous cognitive extension that gives every AI CLI on your machine the same persistent brain.

---

## Why this exists

AI agents don't remember. And they definitely don't share memory with each other. Every Claude session starts from zero. Every Gemini conversation is a goldfish. Every CLI has its own fishbowl.

Makakoo OS fixes this. One install, one set of tools, one cognitive model across every AI CLI on your machine — Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen — plus IDE assistants (VSCode Copilot, Continue, Cline, JetBrains AI). Persistent Logseq-backed memory, capability-sandboxed plugin architecture, proactive maintenance tasks that run while you sleep.

Your notes, your decisions, your arguments with yourself from three months ago — all retrievable by the next assistant you open, in the same terminal or a different one, without a context-reset ceremony.

## What's in v0.1

| Capability | Status |
|---|---|
| Infect 7 AI CLIs with a shared bootstrap block | ✅ macOS + Linux + Windows |
| 4 IDE-assistant hosts (Copilot / Continue / Cline / JetBrains) | ✅ detection + writers |
| Persistent Brain (Logseq journals + pages) | ✅ |
| Superbrain search — FTS5 + vector + LLM synthesis | ✅ |
| Capability-sandboxed plugin system (`plugin.toml` manifests) | ✅ 38 plugins in `plugins-core/`, 32 fully self-contained (Python bundled) |
| SANCHO proactive task engine (8 native + plugin tasks) | ✅ |
| Unix domain socket + Windows named-pipe IPC for plugins | ✅ |
| 5 distros published (minimal, core, sebastian, creator, trader) | ✅ |
| Shell completion (bash, zsh, fish, elvish, powershell) | ✅ |
| Signed release artifacts (Apple notarization + Authenticode) | 🟡 runbook ready, certs pending |
| Homebrew tap / winget submission | 🟡 manifests drafted, submission post-launch |
| Fresh-VM smokes on all three OSes | 🟡 scripted, human-supervised runs pending |

## Install

**macOS / Linux** — one-liner (works after v0.1.0 release tag lands):

```sh
curl -fsSL https://makakoo.com/install | sh
```

**Windows** — PowerShell one-liner (Developer Mode must be on):

```powershell
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
```

**From source** — works today:

```sh
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo install --path makakoo
cargo install --path makakoo-mcp
makakoo install    # distro + daemon + infect + health
```

**Uninstall** — symmetric inverse:

```sh
makakoo uninfect         # strip bootstrap from every CLI slot
makakoo daemon uninstall # remove the auto-launch agent
rm -rf ~/.makakoo ~/MAKAKOO
```

## Quickstart

```sh
# Ask your Brain a question — FTS retrieval fused with LLM synthesis
makakoo query "what did I decide about the database migration?"

# Search the Brain full-text
makakoo search "polymarket"

# Install a plugin from the shipped core set
makakoo plugin install skill-research-arxiv --core

# See what's registered (native Rust tasks + manifest-driven plugins)
makakoo sancho status

# Preview what infect would do, then commit
makakoo infect --global --dry-run
makakoo infect --global
```

## Layout

| Path | Role |
|---|---|
| `makakoo-core/` | Engine library — platform, config, LLM client, superbrain (FTS5 + vectors + graph), SANCHO, capability socket + grant resolver + audit log |
| `makakoo-mcp/` | MCP stdio server — NDJSON JSON-RPC, 40+ tools, drop-in for any MCP client |
| `makakoo/` | CLI binary — search, query, sancho, plugin, distro, daemon, infect, uninfect, skill, secret, mcp, completion |
| `makakoo-platform/` | Per-OS adapter — launchd (macOS), systemd (Linux), auto-launch (Windows), POSIX symlinks + Windows Dev Mode symlinks |
| `makakoo-client/` + `makakoo-client-py/` | Plugin client libraries (Rust + Python) over the capability socket |
| `plugins-core/` | 38 shipped plugin manifests — skills, watchdogs, monitors, mascot GYM, agent-dreams |
| `distros/` | 5 distro bundles — minimal, core, sebastian, creator, trader |
| `install/` | `install.sh` / `install.ps1` + shell completion guide |
| `distribution/` | Packaging metadata — Homebrew formula, winget manifest, cargo-dist config |
| `spec/` | Frozen v0.1 architecture + ABI contracts |
| `docs/` | Release signing runbook, relocate runbook, roadmap |

## Environment variables

- `MAKAKOO_HOME` — root for Brain, config, logs, plugins, state. Defaults to `~/MAKAKOO` today; moves to `~/.makakoo/` per [the relocate runbook](docs/PHASE_H4_RELOCATE.md) post-launch. `HARVEY_HOME` is a legacy alias that resolves to the same dir.
- `AIL_BASE_URL` — LLM gateway base URL. Default `http://localhost:18080/v1` (switchAILocal).
- `AIL_API_KEY` — API key for the LLM gateway. Store via `makakoo secret set AIL_API_KEY` — writes to the OS keyring (Keychain / Secret Service / Credential Manager).
- `MAKAKOO_SOCKET_PATH` — plugin-side canonical env var for the capability socket. Set automatically by `makakoo skill <name>` when spawning a plugin.

## Build from source

```sh
cargo build --release --workspace
cargo test --workspace
cargo clippy -p makakoo-platform --all-targets -- -D warnings
```

Release profile: `lto=true`, `strip=true`, `opt-level="z"`, `codegen-units=1`, `panic="abort"`. Binary sizes at v0.1.0: `makakoo` ~5 MB, `makakoo-mcp` ~4.7 MB.

## Contributing

This is a community-built open-source project. Contributions welcome across code, docs, mascots, design, translation, testing, ideas, and sponsorship — every category is a first-class citizen. See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow.

- MIT licensed. Forever.
- Local-first. Your data lives on your machine. No telemetry.
- No VC. No acquisition path.
- Every mascot has a named maintainer. Every contributor is recognised.

## Documentation

- [CHANGELOG](CHANGELOG.md) — user-visible changes per release
- [docs/RELEASE_SIGNING.md](docs/RELEASE_SIGNING.md) — Apple notarization + Windows Authenticode runbook
- [docs/PHASE_H4_RELOCATE.md](docs/PHASE_H4_RELOCATE.md) — `~/MAKAKOO → ~/.makakoo/` runtime-relocation runbook
- [install/completions/README.md](install/completions/README.md) — shell completion install + dynamic plugin-name wrappers
- [spec/](spec/) — frozen v0.1 architecture + ABI contracts

## License

MIT — see [LICENSE](LICENSE).

---

*🦉 Makakoo OS · an open project · MIT · no VC · no telemetry · eat sleep go bananas repeat 🍌*
