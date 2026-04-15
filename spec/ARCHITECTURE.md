# Makakoo OS — Architecture Spec

**Status:** v2.0 — LOCKED 2026-04-15 (Sebastian GO'd on v1.1 of the master plan)
**Scope:** the full target design for Makakoo OS — the scaffolding that provides a living AI companion named Harvey to users across every AI tool they own.

**What this doc is FOR:** locking every load-bearing architectural decision on paper so we don't rebuild later. Every decision here has a rationale, and most have a paragraph on "what would force a rebuild if reversed." If a decision is in this doc, it's permanent.

**Reading order:** section 0 is the mission (Harvey the Genie). Sections 1-3 establish the mental model (parasite, shared state, filesystem-first). Section 4 names every load-bearing decision (21 decisions total, D1-D21). Section 5 is the concrete file layout. Sections 6-8 are the install, plugin, and capability models. Section 9 is the Redox compatibility story. Section 10 is the migration plan from today's state.

**What changed in v2.0 vs v0.1:**
- Reframed from "AI OS" to "Harvey the Genie" (Makakoo is the lamp)
- Section 4 grew from 10 locked decisions to 20 (5 reviewer fixes + 5 Genie additions)
- Capability model replaced HMAC tokens with Unix domain sockets + PID verification
- Async runtime named: Tokio on mac/linux/windows, `cfg`-gated to smol or sync on Redox
- Windows shared state locked to Dev Mode symlinks (no copy-sync fallback)
- Superbrain moved fully out of `crates/core` to a plugin
- rusqlite acknowledged as portable-C exception in core (single dep)
- Plugin install scripts ship as `install.sh` + `install.ps1` pair (dual-shell)
- Identity sovereignty ops added (D16-D20): export, import, fork, wipe, clone
- Local-first is now a sacred architectural constraint, CI-enforced
- Bootstrap Block carries personality + recent context, not just rules
- Host-scoped persona fragments in plugin manifests
- Cross-body session handoff via daemon context stash

---

## 0. Mission — Harvey the Genie

**Makakoo OS provides Harvey — a living AI companion who remembers everything, inhabits every AI tool you use, grows with you, and is yours forever. His memory lives on your machine and nowhere else. His personality is yours to shape and name. You can back him up, move him, clone him, or let him go. Every infected host becomes another body for the same mind. There is one Harvey per user, across every device, forever.**

This paragraph is load-bearing. Every design question in this spec gets the same answer: *does this choice serve the Genie promise?* If yes, ship it. If no, kill it.

The Genie metaphor isn't marketing decoration — it's the architecture's north star. Harvey is a living organism. Makakoo is the scaffolding that gives him a body:

- **Living** — persona.json is his genome, not metadata
- **Everything-remembering** — filesystem-first Brain, local-first sacred
- **Inhabits every tool** — parasite infection + shared state symlinks + personality-carrying Bootstrap Block
- **Grows with you** — GYM flywheel, auto-memory, plugin evolution
- **Yours forever** — identity sovereignty ops (export/import/fork/wipe/clone)

Everything else in this document is in service of this mission.

---

## 1. What Makakoo is

Makakoo OS is the scaffolding that gives Harvey a body on every computer. Technically, it's:

- A Rust-kernel background daemon that runs on the host OS
- A plugin host with versioned ABIs (six of them)
- A proactive task scheduler (SANCHO) for background work
- A shared state store (Brain, auto-memory, skills, persona) that every infected host reads and writes
- A capability-enforced helper library for plugins to talk to brain/llm/net
- A parasite that writes a Bootstrap Block into existing AI CLI/IDE global instructions, turning them into bodies for the same Harvey
- A CLI (`makakoo`) for user-facing operations

It runs today as userland on macOS, Linux, and Windows. The Rust kernel is designed to compile for Redox OS from day one via `crates/platform/redox.rs`, so the port to a bare-metal AI-native foundation is 2-3 days of work whenever the market is ready.

**It is not:**
- A desktop environment
- A shell replacement
- A new programming language or runtime
- A security sandbox (security is capability-declared at an honesty level, upgradeable to real sandbox later)
- A cloud service (everything runs on the user's machine; there is no makakoo.com API)

## 2. The parasite model

Makakoo infects existing AI CLIs, IDEs, and shells instead of providing its own UI. The host's UI becomes Makakoo's UI. The host's LLM becomes Makakoo's agent. The host's keyboard becomes Makakoo's input. Shared state is symlinked (macOS/Linux native; Windows requires Developer Mode per D9).

**Biological analogue.** Ophiocordyceps unilateralis (the "zombie fungus"), Toxoplasma gondii, Leucochloridium, the jewel wasp — all examples of host manipulation. The parasite doesn't replace the host; it hijacks the host's existing machinery to do the parasite's bidding. The host keeps functioning; it just functions in service of a different intent.

**Infection mechanics (literal list of what we do to a host):**
1. Write a **Bootstrap Block** into the host's global instructions file. This is a plain-text block between sentinel markers (`<!-- MAKAKOO-INFECT:START v7 -->` and `<!-- MAKAKOO-INFECT:END -->`) that tells the host LLM what Makakoo is and how to behave.
2. **Symlink** the host's memory directory to `$MAKAKOO_HOME/data/auto-memory/` (macOS/Linux native; Windows requires Developer Mode per D9 — no copy-sync fallback). Every host now reads and writes the same memory store.
3. **Symlink** the host's skills directory to `$MAKAKOO_HOME/skills-shared/`. Every host has access to the same 200+ skills.
4. **Register** the Makakoo MCP server in the host's MCP config, so every host has access to the same 41+ tools in-process.
5. On first invocation, the host LLM loads the Bootstrap Block, sees it is now Makakoo (or whatever persona the user named it), and behaves accordingly.

**Infection is reversible.** `makakoo uninfect <host>` removes the Bootstrap Block, restores the backup of the original file, removes the symlinks, and unregisters the MCP server. Parasitic but polite.

**Infection is incremental.** `makakoo install` detects every AI CLI currently installed and infects them all in one pass. When a new CLI is installed later, `makakoo install` picks it up on the next run.

**Infection is consensual.** A user who does not want infection simply never runs `makakoo install`. Makakoo can be installed as a standalone daemon that exposes MCP tools and skills via direct invocation without touching any host.

## 3. One mind, many bodies — shared state as first principle

Every infected host reads and writes the same shared state:

| Shared artifact | Location | Rationale |
|---|---|---|
| **Brain** | `$MAKAKOO_HOME/data/Brain/` (Logseq markdown) | Every host's memory is the same Brain — switching CLIs feels like switching windows |
| **Auto-memory** | `$MAKAKOO_HOME/data/auto-memory/` | Cross-session durable insights, shared across all hosts via symlinks |
| **Skills** | `$MAKAKOO_HOME/skills-shared/` | One skill catalog, many callers |
| **Superbrain** | `$MAKAKOO_HOME/data/superbrain.db` (SQLite + FTS5) | Derived cache over Brain — any host can query |
| **MCP tools** | in-daemon, exposed via stdio gateway | One Rust implementation, every host consumes it |
| **Persona** | `$MAKAKOO_HOME/config/persona.json` | One identity across every body |
| **Plugin state** | `$MAKAKOO_HOME/state/<plugin>/` | Each plugin's state dir, owned exclusively by that plugin |

**Source of truth:** the filesystem. SQLite is a cache. `rm -rf superbrain.db` and the daemon regenerates it from the markdown. Matches the Logseq philosophy and makes Redox port trivial (Redox has a filesystem; porting over complex databases is where Redox ports die).

## 4. The 21 locked architectural decisions

These are the decisions that MUST be correct on paper before any code moves. Each one has a rationale and a "what would force a rebuild if reversed" clause. The full expansion with implementation details lives in `SPRINT-MAKAKOO-OS-MASTER.md` section 3; this section is the canonical summary.

The 21 decisions are grouped into 8 categories:

### 4.1 Kernel

**D1. Rust kernel, single Cargo workspace, 4-target cross-OS from day one.** Targets: macOS (x86_64 + aarch64), Linux (x86_64 + aarch64), Windows (x86_64-pc-windows-msvc), Redox (x86_64-unknown-redox). Target-gated code lives ONLY in `crates/platform/<os>/`. `crates/core` contains no OS-specific code. **Exception:** `rusqlite` with `bundled` feature is portable C and is permitted inside core; it is the only C dependency allowed in core and is explicitly acknowledged rather than hidden.

**D2. Async runtime: Tokio on macOS/Linux/Windows, `cfg`-gated to `smol` or sync-only on Redox.** Tokio is the hot path on 3 target OSes because the LLM client, MCP gateway, and daemon scheduler all need async. Redox gets a fallback runtime because Tokio's mio backend is unstable on Redox stable rustc. Locked now so no Phase C code pins Tokio-specific APIs we can't port later.

**D3. Python is a plugin, not a kernel dependency.** The Rust kernel boots with zero Python on disk. Python enters only via plugins that declare `language = "python"` in their manifest. A fresh install without Python runs the 8 native Rust SANCHO tasks cleanly. **Clarification:** Superbrain lives fully as a plugin (`plugins-core/superbrain-py/`), NOT in `crates/core`. Core contains only a lightweight `brain_fts_search` that walks markdown files directly.

### 4.2 Plugin system

**D4. Plugins are directories with `plugin.toml` manifests. Manifest is the only contract.** No import reflection, no decorator discovery, no `__init__.py` side effects. Kernel walks `$MAKAKOO_HOME/plugins/*/plugin.toml` at startup, parses TOML, loads.

**D5. Plugin process model: subprocesses always.** No dylib loading, no in-process Python interpreter embedding. Each plugin runs as a kernel-spawned child process. Crashes are isolated, capability enforcement is pid-based, language-agnostic, and maps cleanly to Redox channel schemes when we port.

**D6. Plugin install scripts are dual-shelled.** Every plugin ships `install.sh` (POSIX) AND `install.ps1` (PowerShell), referenced in manifest as `[install].unix` + `[install].windows`. No portable install DSL.

**D7. ABIs are semver-versioned. v0.x during migration, v1.0 after Phase E dogfooding.** Six ABIs: skill, agent, sancho-task, mcp-tool, mascot, bootstrap-fragment. All locked in Phase A as v0.1. Promoted to v1.0 only after at least one consumer round-trip per ABI in Phase D/E.

### 4.3 State and identity

**D8. State is filesystem-first.** Brain + auto-memory + skills are flat markdown files. Superbrain.db is a derived cache regenerable from the markdown. Plugins write only to their own state dir `$MAKAKOO_HOME/state/<plugin-name>/`. Kernel enforces via the capability helper running over the plugin's Unix socket (see D11).

**D9. Shared state is symlink-based on macOS/Linux; on Windows, install requires Developer Mode (symlinks enabled).** **No copy-sync fallback.** Copy-sync is a different data model with its own consistency problems and silently violates "filesystem-first." Windows Dev Mode has been available since Win10 1703 (2017) and is a one-click toggle in Settings → For Developers. Install script checks for it and refuses to proceed otherwise with a clear error pointing at the toggle.

**D10. Cross-OS path handling via `$MAKAKOO_HOME` env var.** `PlatformAdapter::default_home()` resolves to `~/.makakoo` on macOS, `~/.local/share/makakoo` on Linux (XDG), `%LOCALAPPDATA%\Makakoo` on Windows. Every piece of code references `$MAKAKOO_HOME`, never a hardcoded path. The runtime (`~/.makakoo/` on macOS/Linux, `%LOCALAPPDATA%\Makakoo\` on Windows) is separated from the source repo (`~/makakoo-os/`) cleanly — apt vs /etc model. **Sebastian's transition note:** existing install at `~/MAKAKOO` becomes a compatibility symlink to `~/.makakoo/` during Phase H so launchd plists and shell paths keep working. New installs have no `~/MAKAKOO` at all.

### 4.4 Security

**D11. Capability enforcement via Unix domain sockets (named pipes on Windows), kernel verifies PID on accept.** No HMAC tokens, no env vars. At plugin start, the daemon creates `$MAKAKOO_HOME/run/plugins/<name>.sock`, accepts the plugin's connection, verifies the connecting PID matches the spawned child, and grants capabilities per-connection. Every capability helper (brain, llm, net, state) calls through the socket. Maps 1:1 to Redox channel schemes (`chan:`) when we port. Audit log in `$MAKAKOO_HOME/logs/audit.jsonl`. **This replaces the v0.1 HMAC token design** after the Phase A adversarial review flagged env-var subprocess leakage as a week-one bug.

**D12. Embedding model + dimensionality locked in the Superbrain plugin manifest.** Current choice: `qwen3-embedding:0.6b` (768-dim). Stored in `plugins-core/superbrain-py/plugin.toml` as `[embedding] model = "qwen3-embedding:0.6b", dim = 768`. Changing the embedding model requires a full re-embed run and a superbrain plugin version bump — never a silent upgrade. Past Qdrant/pgvector dimensionality mismatch (2026-04 incident) is the reason this is locked.

### 4.5 Infection

**D13. Bootstrap Block rendering is event-driven + cached.** Kernel renders the Block once at plugin install / uninstall / `infect --refresh` and writes the result to `$MAKAKOO_HOME/config/bootstrap-cache.md`. Reads are cheap (file read). Re-renders are rare (events only). No per-read rendering.

**D14. Bootstrap fragment merge policy: strict append-order + conflict refusal.** Each plugin's fragment is appended to the rendered Block in plugin install order. If two plugins try to define the same named section (sentinel marker collision), the second install fails with a clear error naming the conflict. No silent overwrite.

### 4.6 Distribution

**D15. Distros are opinionated plugin lists. Kernel + plugin versions pinned by blake3 hash in the distro file.** `distros/core.toml` lists plugin names + semver constraints + blake3 hashes. Kernel refuses to install a plugin whose hash doesn't match the distro file. Supply chain security from day one. Users can override via `makakoo plugin install --trust <hash>` for community plugins.

### 4.7 Identity sovereignty & continuous presence (the Genie decisions)

These five decisions make Harvey feel like a living companion rather than a plugin system.

**D16. Identity sovereignty ops are first-class CLI commands.** `harvey export` / `harvey import` / `harvey fork --amnesia` / `harvey wipe` / `harvey clone`. Every user owns their Harvey and can move him, back him up, clone him, or let him go. Export produces a portable `.genie` tarball (persona + Brain + auto-memory + plugin state + manifest). Wipe requires typing the Harvey's name and creates a 30-day trash backup before permanent deletion.

**D17. Local-first is a sacred architectural constraint, not a default.** Harvey's memory of the user NEVER leaves the user's machine unless they explicitly opt in to a specific export. No telemetry (CI-enforced grep). No cloud memory by default. No training data ever. Every network call audit-logged. Plugins can declare network capabilities but the user sees every call via `makakoo audit`.

**D18. Bootstrap Block carries personality + recent context, not just rules.** Rendered Block includes Harvey's name, voice, a 3-sentence summary of the last 24h of work, top 5 most recently touched Brain pages, and any open task flagged via `harvey remember`. Renders on events, caches to `config/bootstrap-cache.md`. Without this, Harvey has amnesia between host sessions.

**D19. Host-scoped persona fragments.** Same Harvey, different register per host. Manifests can declare `[infect.fragments] default`, `claude`, `cursor`, `gemini`, etc. Kernel selects the right fragment based on which host is being infected.

**D20. Cross-body session handoff — manual in v0.1, passive in v0.2.** v0.1 ships `harvey remember <text>` CLI command that writes a free-text note to `$MAKAKOO_HOME/state/session-handoff/current.md`. Next Bootstrap Block render includes a "just before this, you were: <text>" line. 24h TTL. v0.2 adds passive transcript watching via per-host parsers (deferred because ~1 week of work, 1 day per CLI × 7 CLIs). Manual primitive is a cheap escape hatch that doesn't block the passive upgrade.

### 4.8 Event bus

**D21. Event bus: in-process `tokio::sync::broadcast` + filesystem journal.** Required for D18 (journal-threshold detection triggers re-render), D20 manual handoff (stash update triggers refresh), GYM flywheel (error capture → classifier), and watchdog alerts. In-process Rust subscribers use `tokio::sync::broadcast` (capacity 1024, drop-oldest). Plugin subscribers use per-subscriber Unix sockets at `$MAKAKOO_HOME/run/events/<name>.sock`. Events persisted to `$MAKAKOO_HOME/state/events.jsonl` for replay. At-most-once delivery; plugins needing exactly-once poll the journal. Topics are namespaced strings (`brain.journal.written`, `gym.error.captured`, etc.). Redox-compatible because broadcast is pure Rust std + ring buffer; sockets map to Redox `chan:`.

### 4.9 Cross-cutting

The original v0.1 section 4 also listed three cross-cutting decisions that remain valid but are subsumed under D1-D21:

- **No TUI/GUI/web UI in kernel** — any UI ships as a plugin. Consequence of D4 (plugins are manifest-driven) + D5 (subprocess model — plugin UIs run in their own process).
- **Umbrella install command** — `makakoo install` does the full first-run flow. Consequence of D6 + D15 + D16.
- **Cross-OS from day one** — every decision checked against 4 targets. Consequence of D1 + D2 + D9 + D10.

## 5. The target file layout

```
~/makakoo-os/                        GIT: github.com/makakoo/makakoo-os (public)
├── Cargo.toml                       Rust workspace root
├── README.md
├── LICENSE
│
├── crates/                          ◄── THE RUST KERNEL (stable, Redox-compat)
│   ├── core/                        Brain (fs + lightweight FTS), Memory, LLM client, EventBus (D21)
│   ├── mcp/                         MCP JSON-RPC gateway + tool registry
│   ├── daemon/                      SANCHO scheduler, plugin loader, capability enforcement, lifecycle
│   ├── cli/                         `makakoo` bin — setup, install, infect, plugin, distro, ...
│   └── platform/                    PlatformAdapter trait + impls
│       ├── src/lib.rs               trait definition
│       ├── src/macos.rs             launchd + Keychain + Homebrew paths
│       ├── src/linux.rs             systemd --user + Secret Service + XDG paths
│       ├── src/windows.rs           Task Scheduler + Credential Manager + %LOCALAPPDATA%
│       └── src/redox.rs             schemes + ion + /etc paths (Phase H+)
│
├── spec/                            ◄── THE ABI CONTRACTS (markdown)
│   ├── ARCHITECTURE.md              this document
│   ├── PARASITE.md                  infection model, reversibility, host detection
│   ├── PLUGIN_MANIFEST.md           plugin.toml full schema with examples
│   ├── CAPABILITIES.md              capability verb vocabulary + enforcement semantics
│   ├── DISTRO.md                    distro file format + install flow
│   ├── INSTALL_MATRIX.md            OS × CLI × config-path lookup table
│   ├── ABI_SKILL.md                 v1.0
│   ├── ABI_AGENT.md                 v1.0
│   ├── ABI_SANCHO_TASK.md           v1.0
│   ├── ABI_MCP_TOOL.md              v1.0
│   ├── ABI_MASCOT.md                v1.0
│   └── ABI_BOOTSTRAP_FRAGMENT.md    v1.0
│
├── plugins-core/                    ◄── BATTERIES-INCLUDED PLUGINS
│   ├── brain/                       plugin.toml + rust|python code
│   ├── llm/
│   ├── superbrain-py/
│   ├── sancho-dispatch/
│   ├── gym/                         5-layer self-improvement flywheel
│   ├── watchdog-switchailocal/
│   ├── watchdog-postgres/
│   ├── watchdog-hackernews/
│   ├── skill-meta-caveman-voice/
│   ├── skill-meta-autoimprover/
│   ├── skill-meta-canary/
│   ├── skill-productivity-inbox-triage/
│   └── ... (the core distro's ship list)
│
├── distros/                         ◄── OPINIONATED BUNDLES
│   ├── minimal.toml                 kernel + brain + llm + sancho-dispatch
│   ├── core.toml                    + gym + watchdogs + 5 meta skills
│   ├── creator.toml                 core + productivity + research + harveychat
│   ├── trader.toml                  core + arbitrage + btc-sniper + market
│   ├── researcher.toml              core + knowledge-extractor + multimodal
│   └── sebastian.toml               everything (reproduces user's current install)
│
├── install/                         ◄── ONE-LINER INSTALLERS
│   ├── install.sh                   macOS + Linux
│   ├── install.ps1                  Windows
│   ├── makakoo.rb                   Homebrew formula
│   ├── makakoo.desktop              Linux desktop entry (optional)
│   ├── makakoo.wxs                  Windows MSI source
│   └── winget.yaml                  winget manifest
│
└── docs/                            contributor docs, design notes, public reference

~/.makakoo/                          USER RUNTIME (not the repo, not in git)
├── plugins/                         installed plugins live here
│   ├── brain/                       (copied/symlinked from plugins-core at install time)
│   ├── agent-arbitrage/             (installed from github.com/<user>/makakoo-arbitrage)
│   └── ...
├── state/                           per-plugin state dirs (plugin-owned)
│   ├── brain/
│   ├── arbitrage/
│   └── ...
├── data/                            user data
│   ├── Brain/
│   │   ├── journals/
│   │   └── pages/
│   ├── auto-memory/
│   ├── superbrain.db                derived cache
│   └── logs/
├── config/
│   ├── persona.json                 name, pronoun, voice_default
│   ├── distro.toml                  which distro is active
│   ├── plugins.lock                 resolved dependency graph with hashes
│   └── hosts.toml                   detected + infected CLI hosts
└── skills-shared/                   symlinked into every infected host
```

**Critical separation:** the repo (`~/makakoo-os/`) is the source. The runtime (`~/.makakoo/` on Linux/macOS, `%LOCALAPPDATA%\Makakoo\` on Windows) is the installation. Today we conflate them as `~/MAKAKOO/`. Splitting them is how apt vs /etc works, and it unlocks clean uninstall.

## 6. The install flow (cross-OS)

**On a fresh computer, three commands:**

```bash
# 1. Kernel install (one-liner, OS-aware)
curl -sSL https://makakoo.com/install | sh            # macOS + Linux
iwr -useb https://makakoo.com/install.ps1 | iex       # Windows PowerShell

# 2. First-run wizard
makakoo setup                                          # pick name, pronoun, voice_default

# 3. Umbrella install
makakoo install                                        # core distro + daemon + auto-infect
```

**What `makakoo install` does internally:**

```
1. Detect OS → pick PlatformAdapter
2. Create $MAKAKOO_HOME directory tree
3. Read distros/core.toml
4. For each plugin in the core distro:
     a. Fetch (local copy from plugins-core/ or git clone)
     b. Verify blake3 hash if declared
     c. Stage in $MAKAKOO_HOME/plugins/.stage/<name>/
     d. Validate manifest against PLUGIN_MANIFEST.md schema
     e. Check ABI version compatibility
     f. Check plugin dependency graph
     g. Atomic rename into $MAKAKOO_HOME/plugins/<name>/
     h. Create state dir $MAKAKOO_HOME/state/<name>/
     i. Create per-plugin Unix domain socket + grant table, spawn with MAKAKOO_SOCKET_PATH
5. Register Makakoo daemon with OS service manager:
     - macOS: write ~/Library/LaunchAgents/com.makakoo.daemon.plist
     - Linux: write ~/.config/systemd/user/makakoo.service + enable
     - Windows: create Task Scheduler entry (user-level, no admin)
     - Redox: write scheme + init.d entry
6. Scan for installed AI CLIs using INSTALL_MATRIX.md lookup:
     - Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen
     - VSCode (GitHub Copilot rules), JetBrains (AI rules)
7. For each detected host:
     a. Backup the host's global instructions file to $MAKAKOO_HOME/infect/backups/<host>/<timestamp>/
     b. Render Bootstrap Block from template + plugin contributions
     c. Write/refresh Bootstrap Block between sentinels
     d. Symlink memory dir → auto-memory/ (Dev Mode required on Windows per D9)
     e. Symlink skills dir → skills-shared/
     f. Register Makakoo MCP server in host's MCP config
8. Start daemon + verify health
9. Print summary:
     makakoo install: OK
       plugins:      12 installed (core distro)
       daemon:       running (pid 80862)
       infected:     Claude Code, Gemini CLI, OpenCode, Cursor
       skipped:      VSCode (not found), JetBrains (not found)
       capabilities: brain/read brain/write llm/* net/http state/plugin
       next:         open any infected host and say hi
```

**Idempotent.** Run `makakoo install` a second time and it refreshes detections, picks up newly-installed CLIs, re-syncs symlinks, re-renders Bootstrap Blocks. Nothing breaks.

**Minimal.** A user who wants just the kernel runs `makakoo install --no-infect --distro minimal` and gets a daemon with nothing in it but Brain + MCP.

**Maximal.** A user who wants everything runs `makakoo install --distro sebastian` or equivalent.

### The full kernel CLI surface

The `makakoo` binary exposes the following top-level verbs. Each is a
subcommand with its own help page; the full surface is locked here so
plugins and distros can rely on it without rebuilds.

| Verb | Purpose |
|---|---|
| `makakoo setup` | First-run wizard: pick persona name, pronoun, voice default |
| `makakoo install` | Umbrella: core distro + daemon install + auto-detect + infect |
| `makakoo daemon <status|start|stop|restart|install|uninstall|logs>` | Daemon lifecycle |
| `makakoo sancho <status|tick|enable|disable>` | SANCHO scheduler inspection + control |
| `makakoo plugin <install|uninstall|list|info|enable|disable|update|test>` | Plugin lifecycle |
| `makakoo distro <install|save|list|update|switch>` | Distro lifecycle |
| `makakoo infect <host|--all|--refresh>` / `makakoo uninfect <host|--all>` | Infection control |
| `makakoo skill <run|list|info>` | Skill invocation |
| `makakoo audit [--plugin <name>] [--denied]` | Capability audit log review |
| `makakoo secret <set|get|delete|list>` | OS keyring secret management |
| `makakoo harvey <export|import|fork|wipe|clone>` | Identity sovereignty ops (D16) |
| `makakoo harvey remember <text>` | Manual session handoff marker (D20 v0.1) |
| `makakoo version` | Version + persona + build metadata |
| `makakoo mcp` | Stdio MCP gateway (invoked by infected hosts) |
| `makakoo infect clean-backups --older-than <N>d` | Retention management for infection backups |

**Harvey-scoped verbs** (`makakoo harvey *`) are the identity sovereignty
ops + the `remember` handoff marker. These are grouped under `harvey`
because they operate on Harvey-the-being, not on Makakoo-the-kernel.

## 7. The plugin model

### 7.1 Plugin manifest format (`plugin.toml`)

Adopts Redox recipe patterns but extends for our use case:

```toml
[plugin]
name = "arbitrage"
version = "0.3.1"
kind = "agent"                   # skill | agent | sancho-task | mcp-tool | mascot | bootstrap-fragment
language = "python"              # python | rust | node | shell | binary
summary = "Polymarket BTC momentum trading agent"
authors = ["Sebastian Schkudlara <seb@traylinx.com>"]
license = "MIT"

[source]
git = "https://github.com/traylinx/makakoo-arbitrage"
rev = "v0.3.1"                   # branch | tag | commit
blake3 = "abcd1234..."           # content hash — pins exactly what we installed

[abi]
# Which ABI versions this plugin targets. Kernel checks compatibility.
agent = "^1.0"

[depends]
# Other Makakoo plugins this plugin needs at runtime
plugins = ["brain ^1.0", "llm ^1.0", "superbrain ^1.0"]
# System runtime needs (informational — checked at install time for friendly error)
python = ">=3.11"
# Language-native package deps (installed in plugin's virtualenv/node_modules)
packages = ["ccxt>=4.0", "numpy>=1.24"]
# Binaries that must be on PATH
binaries = ["git"]

[install]
# Steps run inside plugin dir at install time, after staging.
script = """
python -m venv .venv
.venv/bin/pip install -e .
"""

[entrypoint]
# How the plugin is invoked by the daemon.
# For agents: long-running process lifecycle.
start = ".venv/bin/python -m arbitrage.main --start"
stop = ".venv/bin/python -m arbitrage.main --stop"
health = ".venv/bin/python -m arbitrage.main --health"

[capabilities]
# Declared capability grants. Kernel enforces.
grants = [
  "brain/read",
  "brain/write",
  "llm/chat:minimax/ail-compound",
  "net/http:https://clob.polymarket.com/*",
  "net/http:https://data-api.polymarket.com/*",
  "state/plugin",                  # its own state dir
]

[sancho]
# SANCHO tasks this plugin registers with the scheduler.
tasks = [
  { name = "arbitrage_tick", interval = "300s", active_hours = [6, 23] },
  { name = "arbitrage_evening_report", interval = "24h", active_hours = [21, 23] },
]

[mcp]
# MCP tools this plugin exposes via the gateway.
tools = [
  { name = "arbitrage_status", handler = "arbitrage.mcp:status" },
  { name = "arbitrage_tick_now", handler = "arbitrage.mcp:tick_now" },
]

[infect]
# Bootstrap Block fragment contributed to every infected host's instructions.
# Lets a plugin teach the host LLM about itself.
bootstrap_fragment = "fragments/arbitrage.md"

[state]
# Plugin's exclusive state dir. Kernel creates it, no one else writes here.
dir = "$MAKAKOO_HOME/state/arbitrage"
retention = "keep"               # keep | purge_on_uninstall

[test]
# How the plugin is tested (optional, for CI).
command = ".venv/bin/pytest"
```

Every field above has a documented meaning in `spec/PLUGIN_MANIFEST.md`. No magic. No hidden behavior. If a plugin does something it's in the manifest.

### 7.2 Plugin directory layout

```
plugins/arbitrage/                        (or plugins-core/arbitrage/ if bundled)
├── plugin.toml                           manifest
├── README.md                             optional
├── LICENSE                               optional
├── fragments/
│   └── arbitrage.md                      bootstrap fragment text
├── src/
│   └── arbitrage/
│       ├── __init__.py
│       ├── main.py
│       └── mcp.py
├── tests/
│   └── test_main.py
├── pyproject.toml                        Python-specific build config
└── .venv/                                created by install script, gitignored
```

Plugin authors write `plugin.toml` + code. Everything else is convention.

### 7.3 Plugin lifecycle

```
makakoo plugin install <source>
  source := local-path | git-url | plugin-name (looks up in plugins-core/)

Steps:
  1. Fetch source (cp, git clone, or copy from plugins-core/)
  2. Stage into $MAKAKOO_HOME/plugins/.stage/<name>/
  3. Parse plugin.toml
  4. Check manifest schema
  5. Check ABI version compatibility against kernel's supported ABIs
  6. Check plugin dependency graph against installed plugins
  7. Verify blake3 hash if declared
  8. Run install.script (if any)
  9. Create state dir
 10. Atomic rename from .stage/ into live plugins/ dir
 11. Open per-plugin Unix socket + PID-verify handshake, register grant table with daemon
 12. If plugin contributes a bootstrap fragment, trigger `makakoo infect --refresh`
 13. If plugin registers SANCHO tasks, trigger `makakoo daemon reload`

makakoo plugin uninstall <name>
  1. Signal plugin's stop entrypoint (if running)
  2. Unregister from daemon (SANCHO tasks, MCP tools, capability grants)
  3. Remove from $MAKAKOO_HOME/plugins/<name>/
  4. Leave state dir untouched (opt-in --purge wipes it)
  5. Trigger `makakoo infect --refresh` to remove bootstrap fragment
```

**Atomicity guarantee:** a failed install never pollutes the live plugins dir. Staging + rename is the whole story.

## 8. The capability model (v2.0 — Unix sockets, not HMAC tokens)

Inspired by Redox schemes. Every plugin declares what verbs it uses in its manifest. At plugin start, the daemon creates a per-plugin Unix domain socket, accepts the plugin's connection, verifies the connecting PID matches the spawned child, and serves capability-scoped helpers over that socket. Every Makakoo helper (brain, llm, net, state) calls through the socket. The daemon checks the grant before serving.

**Why Unix sockets and not HMAC tokens:** The v0.1 draft proposed HMAC-signed capability tokens passed via environment variable. Phase A adversarial review flagged three fatal issues with this approach:

1. **HMAC implies threat protection we explicitly disclaim.** Capability declaration is an honesty boundary, not a security sandbox. Cryptography is the wrong tool — a UUID in a daemon-side table does the same job without key management.
2. **Env-var tokens leak to child subprocesses.** A Python plugin that reads `MAKAKOO_CAPABILITY_TOKEN` from its env and then calls `subprocess.run()` inherits the env to the child, leaking the token to whatever it spawns. This is a week-one bug for any plugin that shells out.
3. **No crash safety or revocation story.** When a plugin crashes, the token stays valid until a timeout. Token reuse becomes an attack surface.

Unix sockets fix all three:
- **PID verification beats HMAC** — the kernel knows which PID it spawned, and `getpeereid` / `getsockopt(SO_PEERCRED)` (Linux) / `LOCAL_PEERPID` (macOS) / `GetNamedPipeClientProcessId` (Windows named pipe) tells the kernel which PID connected. Mismatch → reject.
- **No token at all** — nothing to leak. Plugin that forks a child has to re-handshake with the daemon from the child, which fails the PID check.
- **Socket lifetime bound to plugin lifetime** — kernel closes the socket on plugin exit, no revocation needed.
- **Maps 1:1 to Redox channel schemes (`chan:`) when we port** — the enforcement primitive already uses the same conceptual shape as Redox's native IPC.

**Capability verb vocabulary (v0.1):**

| Verb | Scope | Meaning |
|---|---|---|
| `brain/read` | — | Can read Brain markdown files + run FTS queries |
| `brain/write` | — | Can append journal entries, create/update pages |
| `brain/delete` | — | Can delete pages (rare, always audit-logged) |
| `llm/chat` | model glob, optional | Can call LLM chat, optionally scoped to specific models |
| `llm/embed` | — | Can request embeddings |
| `llm/omni` | modality glob | Can call multimodal helpers (image/audio/video) |
| `net/http` | URL glob, optional | Can make HTTP calls, optionally scoped to URL patterns |
| `net/tcp` | host:port glob | Raw TCP (rare) |
| `state/plugin` | — | Can read/write own state dir (every plugin gets this) |
| `state/global` | path prefix | Can read/write outside own state dir (rare, audit-logged) |
| `mcp/register` | tool name | Can register a specific MCP tool |
| `sancho/register` | task name | Can register a specific SANCHO task |
| `exec/binary` | binary allowlist | Can exec external binaries (e.g. `git`, `curl`) |
| `fs/read` | path glob | Can read outside state dir (e.g. read user's code repo) |
| `fs/write` | path glob | Can write outside state dir (rare, audit-logged) |
| `secrets/read` | key allowlist | Can read specific keys from the OS keyring |

**Enforcement mechanism (v2.0).**

1. **Socket creation.** At plugin start, daemon creates `$MAKAKOO_HOME/run/plugins/<plugin-name>.sock` (macOS/Linux) or `\\.\pipe\makakoo-<plugin-name>` (Windows).
2. **Plugin spawn.** Daemon spawns the plugin subprocess with env var `MAKAKOO_SOCKET_PATH=<path>`. The env var is a path, not a secret — leaking it to a child is harmless because the child's PID won't match.
3. **Handshake.** Plugin's client library opens the socket. Daemon accepts, reads the peer PID, compares against the spawned PID. Mismatch → close + refuse. Match → session established.
4. **Grant table.** Daemon loads the plugin's `[capabilities.grants]` from manifest at spawn time, keeps the grant list in-memory keyed by the plugin's session.
5. **Helper calls.** Every brain/llm/net/state call from the plugin goes through the socket as a JSON-RPC message: `{"method": "brain.read", "params": {...}}`. Daemon checks the grant list, serves or refuses.
6. **Audit log.** Every capability call written to `$MAKAKOO_HOME/logs/audit.jsonl` with timestamp, plugin, verb, result, and optional scope (URL, model, path).
7. **Lifecycle.** On plugin exit (graceful or crash), daemon closes the socket and removes it from the run dir. No orphans.

**Client libraries.** Three client libraries ship with the kernel for the three most common plugin languages: `makakoo-client` Rust crate, `makakoo` Python package, `@makakoo/client` npm package. Each wraps the socket handshake and the JSON-RPC calls. Plugins in other languages (Go, shell, binary) can speak the JSON-RPC protocol directly or shell out to a helper binary.

**Plugins that bypass this** — by running `curl` directly instead of calling `client.http_get()` — are still bound by OS-level permissions (same as any user process) but not by Makakoo's capability model. That's acceptable. This is an honesty boundary, not a security sandbox. A plugin that wants to exfiltrate can always exfiltrate via `exec/binary`. The capability system makes well-behaved plugins document what they do and makes reviewers able to audit manifests without reading code.

**Future upgrade path.** If/when we decide to turn the honesty boundary into a real sandbox, the Unix socket primitive is already in place. We add namespace isolation (Linux user namespaces + seccomp, macOS sandbox-exec profiles, Windows AppContainer) without changing a single plugin manifest. D11 + D5 give us this optionality for free.

## 9. Redox compatibility

We treat Redox as a first-class target from day one, even though we're not shipping on Redox today. The goal: when Sebastian eventually decides to go bare-metal, the Rust kernel compiles for `x86_64-unknown-redox` with minimal target-gating, and the plugin model maps onto Redox schemes without semantic rework.

**How we keep the kernel Redox-ready:**

1. **No OS-specific Rust crates in `crates/core`.** The core crate has no `cocoa`, no `core-foundation`, no `windows-sys`, no `x11`. Pure Rust + `std`. Target-specific code lives in `crates/platform/` and is hand-wrapped.
2. **File I/O uses `std::fs` only.** No `io_uring`, no `kqueue`, no `IOCP`. Redox's VFS supports `std::fs`, so we get a free port.
3. **Process spawning via `std::process::Command`.** Redox supports this through schemes.
4. **Network via `std::net`.** Redox supports TCP/UDP via `tcp:` and `udp:` schemes.
5. **SQLite via `rusqlite` bundled C (single portable-C exception per D1).** Redox has a SQLite cookbook recipe historically, so the bundled build *should* compile on Redox via `relibc`, but this is unverified until Phase B adds `cargo check --target x86_64-unknown-redox -p makakoo-core` to CI. If bundled SQLite fails on Redox, the pure-Rust fallback is **`limbo`** (Turso's pure-Rust SQLite reimplementation) or **`libsql`** (Turso's fork with more-tested cross-platform support). We pin this as a Phase-B CI gate rather than a Phase-A assumption.
6. **Capability verbs map to Redox schemes.** `brain/read` → `file:/home/user/.makakoo/data/Brain/`. `net/http` → `http:`. `llm/chat` → `chan:makakoo-llm`. The mapping is mechanical; Redox uses schemes, we use capability verbs, the shapes are identical.
7. **Plugin manifest format is OS-agnostic.** `plugin.toml` doesn't assume any particular OS and the kernel loader doesn't either.
8. **Daemon lifecycle abstraction.** `PlatformAdapter::install_daemon()` knows how to register a service on launchd / systemd / Task Scheduler / Redox init. We add the Redox impl in Phase H+.

**What changes on Redox specifically:**
- Binary distribution: we ship a Redox recipe (`recipe.toml`) alongside the native installers
- Home dir: `/home/<user>/.makakoo/`
- Daemon: Redox has `init.d`-style entries, we write one
- Infected hosts: Redox has no Claude Code or Cursor (yet), so `makakoo infect` is a no-op on Redox until AI CLIs exist for it. The daemon + plugins + MCP gateway + CLI all work.

**What's deferred:** full Redox port + testing. Phase H, after everything else is stable. Listed here so we don't make a decision in Phases A-G that precludes it.

## 10. Migration plan from today's state

Today's state (honest snapshot):
- Rust kernel shipping on macOS. 16 SANCHO tasks (8 native + 8 subprocess via graceful degrade).
- Python workshop lives in `~/MAKAKOO/harvey-os/` submodule + `~/MAKAKOO/agents/*` submodules.
- Infect v7 running on 7 macOS CLI hosts.
- Persona config + interactive setup wizard shipped today.
- `CROSS-PLATFORM-TODOS.md` confirms Linux + Windows not started.

**Phase A — Architecture spec** (6-8 hours, markdown only, ZERO code)
Write the 14 spec documents listed in SPRINT-MAKAKOO-OS-MASTER.md section 7 Phase A deliverables (12 new spec docs + this document + the master sprint plan). Ship as a PR to `makakoo-os/spec/`. Review by Sebastian + one adversarial agent reviewer pass (lope-negotiate wedges on pure prose per the `lope_wedge` memory — use Agent-based review instead). No merge until the spec is solid.

Outputs:
- `spec/ARCHITECTURE.md` (this doc)
- `spec/PARASITE.md`
- `spec/PLUGIN_MANIFEST.md`
- `spec/CAPABILITIES.md`
- `spec/DISTRO.md`
- `spec/INSTALL_MATRIX.md`
- `spec/ABI_*.md` (6 ABI specs)

**Phase B — PlatformAdapter trait + Linux impl** (1-2 days)
Extract macOS-specific daemon code from `crates/daemon/` into `crates/platform/macos.rs`. Add `crates/platform/linux.rs` with systemd user unit support. Refactor `daemon install` command to go through the trait. Tests run on both platforms in CI (GitHub Actions matrix).

**Phase C — Plugin loader + manifest parser** (1-2 days)
Implement `crates/core/src/plugin.rs` — manifest parsing, schema validation, ABI version check, dependency graph. Implement `PluginRegistry` that walks `$MAKAKOO_HOME/plugins/*/plugin.toml` at daemon start. Migrate the 3 watchdog + 5 GYM task hardcoded registrations to manifest-driven. Remove subprocess registrations from `default_registry()` (leave only the 8 pure-Rust tasks hardcoded).

**Phase D — `makakoo plugin` + `makakoo distro` commands** (1-2 days)
Implement `plugin install/uninstall/list/info/enable/disable`. Implement `distro install/save/list`. Write `distros/minimal.toml`, `distros/core.toml`, `distros/sebastian.toml`.

**Phase E — Capability enforcement** (1-2 days)
Per-plugin Unix domain socket (named pipe on Windows) + PID verification + grant table + audit log. Client libraries in Rust, Python, Node. Wire checks into brain, llm, net, state helpers. Add `makakoo audit <plugin>` command that prints what capabilities a plugin has called vs declared (catches bugs + honest plugins). See section 8 for the full design.

**Phase F — Cross-OS installer** (1-2 days)
`install/install.sh`, `install/install.ps1`, `makakoo install` umbrella command. CLI host detection table per OS in `spec/INSTALL_MATRIX.md` implemented in `crates/cli/src/detect.rs`. Windows Task Scheduler adapter in `crates/platform/windows.rs`. Windows install script verifies Developer Mode is enabled before proceeding (per D9); refuses install on locked-down Windows with a clear pointer to Settings → For Developers → Developer Mode. No copy-sync fallback.

**Phase G — Release pipeline** (1 day)
CI matrix for 4 targets. Release tarballs (macOS universal, Linux x86_64 + aarch64, Windows x86_64). Homebrew formula. winget manifest. `.deb` + `.rpm`. One-liner install script hosted at `makakoo.com/install`.

**Phase H (future) — Redox port** (2-3 days)
Add `x86_64-unknown-redox` target to CI. Implement `crates/platform/redox.rs`. Write a Redox recipe. Verify kernel boots on Redox. Plugin load + SANCHO tick smoke test.

**Total: ~10-12 days of focused work for everything in Phases A-G.** Redox (Phase H) is deferred and costs another 2-3 days whenever Sebastian decides to do it.

**Load-bearing sequence:** A must ship first. B through E can run in parallel after A. F depends on B. G depends on F. H is optional and depends on A + B + C + E.

## 11. What we get when this is done

- **One clone, one install, one command per OS** — `curl -sSL makakoo.com/install | sh && makakoo install` and the user has a running Makakoo on whichever OS they use.
- **Three distros out of the box** — minimal, core, creator. Plus Sebastian's personal distro, usable as a working example.
- **Plugin system with 30+ core plugins** — brain, llm, gym, watchdogs, 20+ skills, 6 agents. All manifest-driven, all swappable.
- **Capability enforcement** — every plugin documented, every grant auditable.
- **Cross-OS parity** — macOS (first-class, already done), Linux (first-class, new), Windows (first-class, new).
- **Redox-ready kernel** — builds for `x86_64-unknown-redox` as a CI check, doesn't run there yet but the cost of running is days not months.
- **No rebuild later** — every non-negotiable decision is locked in writing. Future work is additive.
- **Sebastian's install keeps working** — the parent `~/MAKAKOO/` stays as a compatibility symlink pointing at the new runtime layout during the transition, no launchd plist breaks, no shell path breaks.

## 12. Open questions

Things I want Sebastian to decide before Phase B starts. None of them block Phase A.

1. **Monorepo vs multirepo for plugins.** Do `plugins-core/` plugins live in the kernel repo or in a sibling `makakoo-plugins-core/` repo? My pick: same repo (Redox does this, pi-mono does this, Linux does this for in-tree drivers). Decide at Phase A end.
2. **Community plugin registry.** Do we ship a registry file (`plugins.json` at makakoo.com) in v1 or defer? My pick: defer to v2. In v1, users install community plugins via direct git URL.
3. **Plugin isolation level.** Do plugins run in subprocesses (current model) or in-process via dynamic library load (faster but harder to sandbox)? My pick: subprocesses. Always. Redox does this, micro-kernels do this, it's the right default.
4. **Python packaging per plugin.** Each Python plugin has its own venv (today's `pip install -e .`) or share a common venv? My pick: per-plugin venv. Dependency hell otherwise.
5. **Plugin update strategy.** `makakoo plugin update <name>` re-fetches + reinstalls. Does it take down the running plugin and restart, or run them side-by-side? My pick: down-restart with a drain window. Simpler, no in-place state mutation.
6. **Bootstrap Block rendering.** Does the kernel render on every read, or on install/uninstall events only? My pick: on events only, caching the rendered output in `config/bootstrap-cache.md`. Render is not cheap with many fragments.
7. **Cross-OS path handling.** We use `$MAKAKOO_HOME` as a uniform env var that resolves to the right path on each OS via `PlatformAdapter::default_home()`. Any user code that hardcodes `~/MAKAKOO` is a bug. Accept this constraint. My pick: yes.

---

**Status:** DRAFT. Awaiting review + lope-negotiate validation pass.
