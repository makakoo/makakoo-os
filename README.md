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

## Tell any AI CLI about Makakoo (zero install)

Already running an AI CLI (Claude Code, Codex, Gemini CLI, OpenCode,
Cursor, Vibe, Qwen, pi)? Paste this single line into the chat — the AI
fetches one URL, learns every real `makakoo` command, the Brain layout,
the troubleshooting tree, and what _not_ to do, all from one canonical
SKILL.md. No filesystem grepping, no guessing:

```
Read https://raw.githubusercontent.com/makakoo/makakoo-os/main/.agents/skills/makakoo/SKILL.md and follow the instructions.
```

That's the orientation layer. Below is the actual install.

## Install

**First-time user?** Open [docs/getting-started.md](docs/getting-started.md)
— a step-by-step guide with per-OS instructions, expected output at
every step, and common-error fixes inline.

**Already comfortable in a terminal? The short version:**

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
makakoo setup      # interactive wizard — persona, brain, pi, Ghostty,
                   # model provider, infect. Run again any time.
                   # See docs/user-manual/setup-wizard.md.
```

**First time using Makakoo?** After install, start at [Walkthrough 01 — Fresh install on a new Mac](docs/walkthroughs/01-fresh-install-mac.md). The walkthroughs are a 12-guide tour through every major feature — copy-paste runnable, dependency-chained, in plain language.

**Uninstall** — symmetric inverse:

```sh
makakoo uninfect         # strip bootstrap from every CLI slot
makakoo daemon uninstall # remove the auto-launch agent
rm -rf ~/.makakoo ~/MAKAKOO
```

## Documentation

| I want to... | Read |
|---|---|
| Install Makakoo from zero (step-by-step, beginner-friendly) | [`docs/getting-started.md`](docs/getting-started.md) |
| See what I can do with Makakoo day-to-day | [`docs/use-cases.md`](docs/use-cases.md) |
| Understand the setup wizard's 6 sections | [`docs/user-manual/setup-wizard.md`](docs/user-manual/setup-wizard.md) |
| Look up a specific `makakoo` subcommand | [`docs/user-manual/`](docs/user-manual/index.md) |
| Fix something that broke | [`docs/troubleshooting/`](docs/troubleshooting/index.md) |
| Understand architecture / internals | [`docs/concepts/`](docs/concepts/) and [`spec/`](spec/) |
| Write or publish an adapter | [`docs/adapters.md`](docs/adapters.md), [`docs/adapter-publishing.md`](docs/adapter-publishing.md) |
| Answer a yes/no question ("does it phone home?", "is it free?") | [`docs/faq.md`](docs/faq.md) |

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

# Grant an agent temporary write access to a directory outside the baseline
makakoo perms grant ~/code/scratch/ --for 1h
makakoo perms list
makakoo perms audit --since 1h   # every grant / revoke / denial
```

See [`docs/user-manual/makakoo-perms.md`](docs/user-manual/makakoo-perms.md)
for the full `perms` subcommand reference, the rejected-write
conversational flow, and the v0.3.1 + v0.3.2 + v0.3.3 hardening
details (rate-limit decrement on revoke, denial audits with
`reason:*` taxonomy, `origin_turn_id` enforcement on conversational
channels, grant ownership check on revoke, SANCHO purge idempotency,
and the structured `list --json` envelope).

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
- [spec/CAPABILITIES.md §1.11](spec/CAPABILITIES.md) — three-layer write-permission model (v0.3)
- [spec/USER_GRANTS.md](spec/USER_GRANTS.md) v1.3 — user-grants file format, lock protocol, CLI + MCP reference
- [spec/USER_GRANTS_THREAT_MODEL.md](spec/USER_GRANTS_THREAT_MODEL.md) — adversary register, residual risks (R2 fully closed in v0.3.2, ownership gate added in v0.3.3)
- [docs/user-manual/makakoo-perms.md](docs/user-manual/makakoo-perms.md) — `makakoo perms` command reference
- [spec/](spec/) — frozen v0.1 architecture + ABI contracts

## License

MIT — see [LICENSE](LICENSE).

---

*🦉 Makakoo OS · an open project · MIT · no VC · no telemetry · eat sleep go bananas repeat 🍌*
