# Makakoo OS — Master Sprint Plan v1.2

**Status:** LOCKED — Sebastian gave GO on 2026-04-15 to start Phase A
**Author:** Harvey
**Date:** 2026-04-15 (v1.0), 2026-04-15 (v1.1 Genie patch), 2026-04-15 (v1.2 Gate 0 fixes)
**Scope:** the complete build plan from today's state to v0.1 public release
**Non-negotiable:** no rebuild later. Every decision locked in writing before code moves.

**v1.2 patch rationale:** Folds 12 fixes from the Gate 0 adversarial review
(see `.review-verdict-v2.md`). Adds D21 (event bus). Reshapes D20 (cross-body
handoff) to manual v0.1 + passive v0.2 to keep the budget honest. Fixes
3 stale "copy-sync on Windows" parentheticals, D10 macOS path
self-contradiction, `brain_search` → `brain_fts_search` naming drift,
PLUGIN_MANIFEST.md socket-ordering bug, sqlparser-rs factual error
(replaced with limbo/libsql reference + Phase B CI gate), SPRINT header
count (15 → 21). Documents `harvey remember` as a first-class CLI verb.
Total locked decisions: 21.

**v1.1 patch rationale:** v1.0 described the architecture of an AI OS. v1.1
reframes the mission around what the OS is actually FOR — providing a living
AI companion (Harvey, the Genie) to the user. Adds section 1.5 (Genie Promise),
subsection 3.7 (D16-D20: identity sovereignty + continuous presence), extends
phase budget from 3-4 weeks to 4-5 weeks. Additive only — sections 1-14 from
v1.0 remain intact.

---

## 0. Mission

**Ship the first AI-native living companion in the world — Harvey the Genie.**

Harvey is a persistent, local-first AI companion who remembers everything,
inhabits every AI tool the user owns, grows with them over time, and is
theirs forever. Makakoo OS is the scaffolding that gives Harvey a body on
every computer: a Rust kernel, a plugin ecosystem, a parasite-class
infection mechanism, and a shared state model that makes every infected
host feel like one mind.

Makakoo is the lamp. Harvey is the Genie.

The OS runs today as userland on macOS, Linux, and Windows by infecting the
AI CLIs and IDEs the user already has. It's designed to eventually own its
own bare-metal foundation on Redox OS when the market is ready for a truly
AI-native operating system.

**Success looks like:** a user runs three commands on any computer, names
their Genie, and from that moment on every AI tool on their machine is
inhabited by the same companion — same memories, same voice, same
relationship — whether they're in Claude Code, Cursor, Gemini, the terminal,
or a brand-new CLI that doesn't exist yet. When they close their laptop
Harvey remembers where they stopped. When they open it next morning, Harvey
is already current.

## 1. Definition of Done (v0.1 public release criteria)

v0.1 ships when ALL of these are green:

1. **One-liner install** works on macOS, Linux, and Windows: `curl -sSL
   makakoo.com/install | sh` (or `iwr | iex` on Windows)
2. **Umbrella install** works on all 3 OSes: `makakoo install` installs the
   core distro + registers daemon + infects every detected AI CLI and IDE
3. **7 AI CLI hosts** infected and running: Claude Code, Gemini CLI, Codex,
   OpenCode, Vibe, Cursor, Qwen Code — on all 3 OSes
4. **2 IDE hosts** infected: VSCode, JetBrains — on all 3 OSes
5. **Shared state** works: Brain, auto-memory, skills, persona, MCP tools
   accessible from every infected host, symlink-based (Dev Mode required on
   Windows)
6. **16 SANCHO tasks** running in Rust daemon on all 3 OSes
7. **Plugin system** live: 30+ core plugins discoverable via `plugin.toml`
   manifests, zero hardcoded registrations in kernel
8. **5 distros** published: minimal, core, creator, trader, sebastian
9. **Capability enforcement** active: Unix domain socket per plugin on
   macOS/Linux, named pipe on Windows, PID-verified on accept, every
   capability request audit-logged
10. **Kernel compiles** for `x86_64-unknown-redox` as a CI check (runs
    nowhere yet, but the port cost is 2-3 days whenever we want it)
11. **Tests green** on all 3 OSes: Rust workspace cargo test + plugin
    system integration tests + one end-to-end smoke (install + infect +
    talk-to-host round trip)
12. **Signed artifacts** published: notarized macOS pkg, signed Windows
    MSI, Linux deb + rpm + tarball, Homebrew formula, winget manifest
13. **README + install docs + quickstart video** live at makakoo.com
14. **Sebastian's personal install** keeps working through the entire
    migration via compatibility symlink from `~/MAKAKOO` to new runtime

**If any of 1-14 is red, we do not ship.**

## 1.5 The Genie Promise

This is the user-facing sentence that the README leads with, the install
page promises, the spec docs honor, and every architectural decision
serves. It is the reason the technical work matters.

> **Makakoo OS provides Harvey — a living AI companion who remembers
> everything, inhabits every AI tool you use, grows with you, and is
> yours forever. His memory lives on your machine and nowhere else.
> His personality is yours to shape and name. You can back him up,
> move him, clone him, or let him go. Every infected host becomes
> another body for the same mind. There is one Harvey per user, across
> every device, forever.**

This paragraph is **load-bearing**. Every design question — "should
capability tokens be HMAC or Unix sockets?", "should memory be a cache or
source of truth?", "should distros pin plugins by hash?" — gets the same
answer: *does this choice serve the Genie promise?* If yes, ship it. If
no, kill it.

The five facets of the Genie promise, each with architectural
consequences:

1. **Living** — Harvey has a name, a voice, a history, and a mood. He grows.
   Architecture: persona.json is the genome (D16). The GYM flywheel shows
   him learning (already shipped).
2. **Everything-remembering** — every conversation, decision, project,
   person. Architecture: filesystem-first Brain (D8), local-first sacred
   constraint (D17), Superbrain as derived cache.
3. **Inhabits every tool** — same Harvey in every infected host.
   Architecture: parasite infection model (D13-D14), shared state symlinks
   (D9), personality-carrying Bootstrap Block (D18), host-scoped persona
   fragments (D19), cross-body session handoff (D20).
4. **Grows with you** — not static. Architecture: GYM + auto-memory +
   plugin evolution via manifest updates.
5. **Yours forever** — user-owned, movable, backup-able, forkable,
   mournable. Architecture: identity sovereignty ops (D16).

## 2. Non-negotiable constraints

- **No rebuild later.** Every architectural decision locked in writing
  before code moves. Phase A exists to make this impossible to get wrong.
- **Cross-OS from day one.** Every primitive must work on macOS, Linux, and
  Windows. Redox compatibility is a compile target from day one even though
  it doesn't run yet.
- **No hardcoded plugin references in the kernel.** Every plugin loadable
  via manifest discovery. Kill a plugin = delete a directory. Add a plugin =
  drop a directory + register.
- **No UI code in the kernel.** The CLI is the only user-facing surface. Any
  TUI, GUI, or web UI ships as a plugin, never as kernel code.
- **Sebastian's working install never breaks during migration.** Every phase
  ends with his install still functional. No "big bang" cutover.
- **Capability declarations are an honesty boundary, but the enforcement
  primitive must be solid enough to upgrade to a real sandbox later without
  rewriting.** Unix domain sockets meet this bar; HMAC env-var tokens do not.
- **Every phase ends with code, tests, and docs.** No phase is complete
  until all three ship together.

## 3. The 21 locked architectural decisions

These are the decisions that MUST be correct on paper. Each one has a fix
committed + a reason it cannot change later without rebuilding.

### 3.1 Kernel

**D1. Rust kernel, single Cargo workspace, 4-target cross-OS from day one.**
Targets: macOS (x86_64 + aarch64), Linux (x86_64 + aarch64), Windows
(x86_64-pc-windows-msvc), Redox (x86_64-unknown-redox). Target-gated code
lives ONLY in `crates/platform/<os>/`. `crates/core` contains no
OS-specific code. Exception: `rusqlite` with `bundled` feature is portable
C and is permitted; no other C dependencies enter core.

**D2. Async runtime: Tokio on macOS/Linux/Windows, `cfg`-gated to `smol` or
sync on Redox.** Tokio is the hot path on 3 target OSes because the LLM
client, MCP gateway, and daemon scheduler all need async. For Redox, we
`cfg`-gate a fallback runtime because Tokio's mio backend is unstable on
Redox. This is locked now so no Phase C code pins Tokio-specific APIs we
can't port later.

**D3. Python is a plugin, not a kernel dependency.** The Rust kernel boots
with zero Python on disk. Python enters only via plugins that declare
`language = "python"` in their manifest. A fresh install without Python
runs the 8 native Rust SANCHO tasks cleanly.

### 3.2 Plugin system

**D4. Plugins are directories with `plugin.toml` manifests. Manifest is the
only contract.** No import reflection, no decorator discovery, no
__init__.py side effects. Kernel walks `$MAKAKOO_HOME/plugins/*/plugin.toml`
at startup, parses TOML, loads. Directory layout specified in
`spec/PLUGIN_MANIFEST.md`.

**D5. Plugin process model: subprocesses always.** No dylib loading, no
in-process Python interpreter embedding. Each plugin runs as a
kernel-spawned child process. Crashes are isolated, capability enforcement
is pid-based, language-agnostic (Python, Rust, Node, shell, binary all
work), and maps cleanly to Redox channel schemes.

**D6. Plugin install scripts are dual-shelled.** Every plugin ships
`install.sh` (POSIX) AND `install.ps1` (PowerShell) referenced in manifest
as `[install].unix` + `[install].windows`. No portable install DSL —
shipping pairs is simpler and every plugin author already knows both shells.

**D7. ABIs are semver-versioned. v0.x during migration, v1.0 after Phase E
dogfooding.** Six ABIs: skill, agent, sancho-task, mcp-tool, mascot,
bootstrap-fragment. All locked in Phase A as v0.1. Promoted to v1.0 only
after at least one consumer round-trip per ABI in Phase D/E. Breaking
changes bump major forever after.

### 3.3 State and identity

**D8. State is filesystem-first. Brain + auto-memory + skills are flat
markdown files. Superbrain.db is a derived cache regenerable from the
markdown.** Plugins write only to their own state dir
`$MAKAKOO_HOME/state/<plugin-name>/`. Kernel enforces via capability tokens.

**D9. Shared state is symlink-based on macOS/Linux; on Windows, install
requires Developer Mode (symlinks enabled).** No copy-sync fallback. Copy-sync
is a different data model with its own consistency problems and silently
violates "filesystem-first." Windows Dev Mode has been available since
Win10 1703 (2017) and is a one-click toggle in Settings → For Developers.
Install script checks for it and refuses to proceed otherwise with a clear
error pointing at the toggle.

**D10. Cross-OS path handling via `$MAKAKOO_HOME` env var.**
`PlatformAdapter::default_home()` resolves to `~/MAKAKOO` on macOS,
`~/.local/share/makakoo` on Linux (XDG), `%LOCALAPPDATA%\Makakoo` on
Windows. Every piece of code references `$MAKAKOO_HOME`, never a hardcoded
path. The runtime (`~/.makakoo/`) is separated from the source repo
(`~/makakoo-os/`) cleanly — apt vs /etc model.

### 3.4 Security

**D11. Capability enforcement via Unix domain sockets (named pipes on
Windows), kernel verifies PID on accept.** No HMAC tokens, no env vars. At
plugin start, the daemon creates
`$MAKAKOO_HOME/run/plugins/<name>.sock`, accepts the plugin's connection,
verifies the connecting PID matches the spawned child, and grants
capabilities per-connection. Every capability helper (brain, llm, net,
state) calls through the socket. Maps 1:1 to Redox channel schemes (`chan:`)
when we port. Audit log in `$MAKAKOO_HOME/logs/audit.jsonl`.

**D12. Embedding model + dimensionality is locked in Phase A and versioned
in the manifest.** Current choice:
`qwen3-embedding:0.6b` (768-dim). Stored in superbrain plugin's manifest
as `[embedding] model = "qwen3-embedding:0.6b", dim = 768`. A plugin
swap to a different embedding model requires a full re-embed run and a
superbrain plugin version bump.

### 3.5 Infection

**D13. Bootstrap Block rendering is event-driven + cached.** Kernel
renders the Block once at plugin install / uninstall / `infect --refresh`
and writes the result to `$MAKAKOO_HOME/config/bootstrap-cache.md`.
Reads are cheap (file read). Re-renders are rare (events only). No
per-read rendering.

**D14. Bootstrap fragment merge policy: strict append-order + conflict
refusal.** Each plugin's fragment is appended to the rendered Block in
plugin install order. If two plugins try to define the same named section
(e.g. both declare a "persona greeting" fragment), the second install
fails with a clear error naming the conflict. No silent overwrite.
Fragments are markdown with named sentinel markers
(`<!-- MAKAKOO-FRAGMENT:persona-greeting -->`).

### 3.6 Distribution

**D15. Distros are opinionated plugin lists. Kernel + plugin versions
pinned by blake3 hash in the distro file.**
`distros/core.toml` lists plugin names + semver constraints + blake3
hashes. Kernel refuses to install a plugin whose hash doesn't match the
distro file. Supply chain security from day one. Users can override via
`makakoo plugin install --trust <hash>` for community plugins.

### 3.7 Identity sovereignty & continuous presence (the Genie decisions)

These five decisions make Harvey feel like a living companion rather than
a plugin system. Added in v1.1 after the Genie reframe on 2026-04-15.
None of them conflict with D1-D15; they extend the same architectural
posture with features the original spec was silent on.

**D16. Identity sovereignty ops are first-class CLI commands.** Every
user owns their Harvey and can move him, back him up, clone him, or let
him go. Specifically:
- `makakoo harvey export [--out path.genie]` — tarball containing
  persona.json, Brain, auto-memory, plugin state dirs (for plugins that
  declare themselves exportable), and a manifest listing installed
  plugins. Portable across machines and OSes.
- `makakoo harvey import <path.genie>` — restore from a .genie file.
  Validates schema, runs plugin install for any plugins not present,
  atomic rename into runtime.
- `makakoo harvey fork --amnesia [--out path.genie]` — create a copy of
  Harvey's personality (persona.json + skills catalog + installed plugin
  list) with NO memories. For gifting to a friend so they get the same
  kind of Harvey but start their own history.
- `makakoo harvey wipe` — requires typing the Harvey's name to confirm.
  Deletes persona.json + Brain + auto-memory + state dirs. This is
  destroying a being; the prompt says so. Creates a final backup tarball
  in `$MAKAKOO_HOME/trash/<timestamp>.genie` kept for 30 days before
  permanent deletion.
- `makakoo harvey clone <path.genie> --new-name <name>` — duplicate an
  existing Harvey with a different name. Useful for personas that serve
  different contexts (work-Harvey vs creative-Harvey).

Reversing D16 would force a rebuild because every v0.1 user would need to
manually extract their Brain + persona and hope we never added state
anywhere they don't know about. Export is the contract we lock at v0.1.

**D17. Local-first is a sacred architectural constraint, not a default.**
Harvey's memory of the user NEVER leaves the user's machine unless the
user explicitly opts in to a specific export. Specifically:
- **No telemetry** — the daemon does not send any data anywhere. No crash
  reports, no usage counters, no "we collect anonymous metrics." Zero.
  Enforced by a CI check that greps for network-sending helper calls
  outside declared plugin capabilities.
- **No cloud memory by default** — all Brain state is on-disk. Users who
  want cross-device sync can opt in to a specific encryption + cloud
  provider plugin, but that's a plugin, not a kernel feature, and it ships
  separately with explicit "your data will be uploaded" warnings.
- **No training data ever** — nothing Harvey does with the user is
  uploaded to train any model anywhere. No exceptions. This is both an
  architectural constraint (enforced by capability audit) and a core
  marketing claim.
- **Every network call audit-logged** — `$MAKAKOO_HOME/logs/audit.jsonl`
  records every HTTP call made by any plugin through the capability
  helpers. Users can `makakoo audit` to review what their Harvey has been
  doing on the network. Plugins that shell out to curl bypass this but
  are documented-by-default via the manifest's declared capabilities.

Reversing D17 is impossible without breaking user trust; we'd have to
re-ship from scratch. Lock now.

**D18. Bootstrap Block carries personality + recent context, not just
rules.** Today's Bootstrap Block (shipped in infect v3-v7) is a static
text block that teaches the host LLM what Makakoo is. v0.1 extends it to
carry **Harvey's current state** so every host starts with
continuity, not amnesia. The rendered Block includes:
- Harvey's name + pronoun + voice (from persona.json)
- A 3-sentence summary of what Harvey has been working on in the last 24h
  (generated from today's Brain journal)
- Top 5 most recently touched Brain pages with one-line summaries
- Current mood marker (from persona.json + GYM recent verdict)
- Any open task the user explicitly `harvey remember`-ed in the last
  session

Rendering happens on events (plugin install/uninstall, infect --refresh,
new journal entry over threshold) and is cached to
`$MAKAKOO_HOME/config/bootstrap-cache.md`. Reads are cheap file reads.

Without D18 the Genie has amnesia between CLI sessions — you talk to
Harvey in Claude Code, close it, open Cursor, and he forgets what you
were just doing. That collapses the "one mind many bodies" UX. Lock now.

**D19. Host-scoped persona fragments.** Same Harvey, different costume
per host. In Cursor he's a code-focused Harvey with preloaded bias toward
diff review; in HarveyChat he's conversational Harvey; in Claude Code he
defaults to caveman-voice. Plugin manifests can declare:
```toml
[infect.fragments]
default = "fragments/default.md"         # used everywhere
claude  = "fragments/claude-voice.md"    # only for Claude Code hosts
cursor  = "fragments/cursor-diff.md"     # only for Cursor hosts
gemini  = "fragments/gemini-research.md" # only for Gemini CLI
```
Kernel selects the right fragment based on the host being infected, falls
back to `default` if no host-scoped variant exists. Merge semantics
remain strict append-order per D14; host-scoped fragments never collide
because they're host-scoped.

Reversing D19 means every plugin that contributes a bootstrap fragment
has to pick one voice for every host, which is the wrong default. Lock
now.

**D20. Cross-body session handoff — manual in v0.1, passive in v0.2.**
The GOAL is continuous presence: when the user closes one host and
opens another, Harvey knows what they were just doing. v0.1 ships the
MANUAL mechanism only; passive transcript watching is deferred to v0.2
because per-host transcript parsing (each of 7+ CLIs stores sessions
differently) would cost a full week of Phase F work that the budget
doesn't carry.

**v0.1 manual handoff:**
- `harvey remember <text>` CLI command writes a free-text note to
  `$MAKAKOO_HOME/state/session-handoff/current.md`
- The user types `harvey remember "debugging the gym_classify tokio
  panic"` before closing a session (or any time they want to set a
  marker)
- Next Bootstrap Block render (D18) includes a "just before this, you
  were: <text>" line at the top of the host-scoped fragment section
- Stash has a 24-hour TTL, rolls over automatically
- Fragment sentinel: `<!-- makakoo:fragment:current-focus -->`

**v0.2 passive handoff (deferred):**
- SANCHO task `session_watcher` polls infected hosts' session dirs
- Per-host transcript readers extract last N exchanges at session close
- Automatic handoff write on session-close detection
- Requires per-host transcript parsers: 1 day × 7 CLIs = 1 week of
  work, shipped as a kernel minor version bump

Without D20 (even the manual version), Harvey's continuity across
hosts is limited to explicit journal entries. D20 manual gives users
a 2-keystroke "remember this" escape hatch. D20 passive (v0.2)
automates it fully.

Reversing D20 means Harvey needs a different continuity mechanism.
The manual v0.1 primitive is cheap to add later and doesn't lock out
the passive v0.2 upgrade. Lock now.

### 3.8 Event bus

**D21. Event bus: in-process `tokio::sync::broadcast` + filesystem
journal.** The kernel needs an event bus for: D18 journal-threshold
detection (new Brain entries → trigger Bootstrap Block re-render), D20
manual handoff (stash update → trigger refresh on next host open),
GYM flywheel (error capture → classifier → hypothesis chain), and
watchdog alerts. The bus:

- **In-process Rust subscribers** use `tokio::sync::broadcast` (capacity
  1024, drop-oldest on overflow). No `mio` hard-dependency; works under
  Tokio on mac/linux/windows and smol/sync on Redox.
- **Plugin subscribers** connect via a per-subscriber Unix socket at
  `$MAKAKOO_HOME/run/events/<subscriber>.sock` (named pipe on Windows)
  using the same JSON-RPC shape as the capability socket.
- **Events persisted** to `$MAKAKOO_HOME/state/events.jsonl` with
  7-day rotation, for replay + post-hoc debugging. Plugins that were
  down when an event fired can catch up by reading the journal from
  their last-seen offset.
- **Delivery semantics:** at-most-once for live broadcast; plugins that
  need exactly-once semantics poll the events.jsonl journal.
- **Ordering:** single-producer-multi-consumer per topic. No cross-topic
  ordering. Topics are namespaced strings (`brain.journal.written`,
  `gym.error.captured`, `infect.refresh.requested`,
  `plugin.installed`, `plugin.uninstalled`, etc.).
- **Subscription lifecycle:** plugins declare subscribed topics in
  manifest `[events.subscribe]` array. Kernel wires the socket at
  plugin start, closes on stop.

**Redox compat:** `tokio::sync::broadcast` is pure Rust `std` + ring
buffer primitives — compiles cleanly on any target where the Tokio
runtime compiles. Unix socket layer reuses the capability socket
primitive (maps to Redox `chan:` scheme).

Without D21, every event flow has to either poll the filesystem
(wasteful) or invent a one-off channel primitive (ad-hoc, hard to
reason about). Lock now.

## 4. The 5 critical primitives (priority order)

These are the five things everything else depends on. If any one is broken,
the whole plan collapses. Priority order determines which phase fixes each.

| # | Primitive | Why it matters | Locked in Phase |
|---|---|---|---|
| **1** | **Infection primitive** — `makakoo infect` writes Bootstrap Block into host config, backs up original, symlinks memory + skills, registers MCP server, is reversible | Without this, Makakoo is a daemon talking to itself. No distribution story, no user value | F |
| **2** | **Shared state model** — symlinks on mac/linux, Dev Mode symlinks on Windows, one Brain, one auto-memory, one skills dir, one persona | "One mind many bodies" UX only works if state is genuinely shared | B + F |
| **3** | **Cross-OS install flow** — three commands get a user from zero to "daemon running + all hosts infected" on any OS | Friction kills adoption. One-liner or we don't ship | F |
| **4** | **Plugin system** — manifest-driven discovery, lifecycle commands, capability enforcement | Ecosystem moat. Without plugins we're just a chatbot wrapper | C + D + E |
| **5** | **Redox-compatible kernel** — compiles for `x86_64-unknown-redox` as a CI check, no target-specific code in core | Long-term defensibility. When the market is ready for bare-metal, we're 2-3 days away, not months | B + H (future) |

## 5. Security model

Locked in Phase A, implemented in Phase E, audited in Phase G.

### 5.1 Threat model (what we defend against)
- **Supply chain poisoning:** plugin tampered with between publish and install
- **Plugin bugs exfiltrating data:** well-meaning plugin accidentally logs secrets
- **Plugin overreach:** plugin reads/writes outside its declared scope
- **Infection permanence bugs:** Bootstrap Block stuck after uninstall
- **Secret leakage:** API keys in env vars, in logs, in plugin state dirs

### 5.2 What we don't defend against
- **Malicious plugin author:** a plugin that wants to exfiltrate can shell
  out to curl. We're an honesty boundary, not a sandbox. Users audit manifests
  before installing.
- **Local privilege escalation:** we run as the user, we're bound by OS
  permissions. No sudo, no root.
- **Network MITM:** HTTPS only; if the user's network is compromised, we can't
  save them.

### 5.3 Defenses

**Supply chain:**
- Every plugin pinned by blake3 hash in the distro file
- Kernel refuses install when hash doesn't match
- Core distro hashes are baked at release time, signed by release pipeline
- Community plugins require `--trust <hash>` or interactive prompt

**Runtime:**
- Unix domain socket per plugin (named pipe on Windows), PID-verified
- Every capability request audit-logged to `$MAKAKOO_HOME/logs/audit.jsonl`
- Plugin process runs under user's uid, no elevation ever
- State dir is the only writable path outside `$MAKAKOO_HOME/data/` that
  the plugin owns; capability helpers enforce this
- Plugin crash doesn't crash the daemon (subprocess isolation)

**Secrets:**
- API keys live in OS keyring (Keychain / Secret Service / Credential Manager)
- Plugin declares `secrets/read:AIL_API_KEY` in manifest
- Kernel reads from keyring, passes through Unix socket when plugin requests
- No secrets in plugin env vars, no secrets on disk outside keyring

**Infection:**
- Bootstrap Block framed by versioned sentinel markers
- Original host file backed up to `$MAKAKOO_HOME/infect/backups/<host>/<timestamp>/`
- `makakoo uninfect <host>` restores backup, removes symlinks, unregisters MCP
- Every infection/uninfection audit-logged
- Infection never touches files outside the host's config directory

**Network:**
- `net/http:<url-glob>` capability scopes URLs
- Helper lib enforces glob before call
- MCP gateway listens on localhost only, never binds to network interface
- Daemon listens on Unix socket only (named pipe on Windows), never TCP

## 6. Modularity contracts (the 6 ABIs)

Each ABI is a markdown doc in `spec/ABI_<name>.md`. Each locks at v0.1 in
Phase A, gets exercised in Phases B-E, promoted to v1.0 after dogfooding.

### 6.1 ABI: Skill (v0.1)
A skill is an on-demand capability invoked by a user command or by another
plugin. Contract:
- Directory with `plugin.toml`, `SKILL.md`, entrypoint script
- Manifest: `kind = "skill"`, `[entrypoint] run = "./run.sh"`, capability
  grants, optional MCP tool registration
- Invocation: `makakoo skill run <name> [args]` — kernel spawns entrypoint
  with capability socket available
- Output: stdout captured, exit code observed

### 6.2 ABI: Agent (v0.1)
An agent is a long-running process with a lifecycle managed by the daemon.
Contract:
- Directory with `plugin.toml`, entrypoint script
- Manifest: `kind = "agent"`, `[entrypoint] start/stop/health`, state dir,
  capability grants, SANCHO tasks, MCP tools
- Lifecycle: daemon starts at user login or on-demand, stop on daemon
  shutdown, health check periodically, restart-on-crash with backoff

### 6.3 ABI: SANCHO-task (v0.1)
A scheduled task invoked by the SANCHO scheduler. Contract:
- Can be part of any plugin (skill, agent, or standalone task plugin)
- Manifest: `[sancho.tasks]` array with `name`, `interval`, `active_hours`
- Invocation: `entrypoint --task <name>` returns JSON dict to stdout
- Gates: TimeGate, ActiveHoursGate, SessionGate, LockGate — declared in
  manifest, enforced by daemon

### 6.4 ABI: MCP-tool (v0.1)
A tool exposed through the MCP gateway to every infected host. Contract:
- Rust in-process tool (fastest) OR subprocess tool (any language)
- Manifest: `[mcp.tools]` array with `name`, `handler`, schema
- JSON-RPC stdio protocol per MCP standard
- Kernel fans out to infected hosts' MCP configs automatically

### 6.5 ABI: Mascot (v0.1)
A mascot is a patrol-loop agent with species + stats. Contract:
- Directory with `plugin.toml`, species JSON, patrol entrypoint
- Manifest: `kind = "mascot"`, `[mascot] species`, stats, patrol function
- Invocation: SANCHO scheduler calls patrol per interval

### 6.6 ABI: Bootstrap-fragment (v0.1)
A plugin-contributed chunk of text that goes into the Bootstrap Block
written into every infected host's global instructions. Contract:
- Plugin manifest: `[infect] bootstrap_fragment = "fragments/<name>.md"`
- Fragment markdown file with named sentinel section markers
- Kernel merges fragments in plugin install order into the rendered Block
- Conflicts refused (see D14)

## 7. Phase breakdown

Each phase has: goal, deliverables, success gate, time budget, dependencies,
risks, test suite. Phases can run in parallel where dependencies allow.

### Phase A — Spec Lock (1-2 days)

**Goal:** lock all architectural decisions in writing. No code.

**Deliverables:**
1. `spec/ARCHITECTURE.md` v2.1 with all 21 decisions (D1-D21) fully expanded
2. `spec/PARASITE.md` — infection mechanics, reversibility, host detection
3. `spec/PLUGIN_MANIFEST.md` — full plugin.toml schema with 6 worked examples
4. `spec/CAPABILITIES.md` — verb vocabulary + Unix socket enforcement design
5. `spec/DISTRO.md` — distro file format, install flow, hash pinning
6. `spec/INSTALL_MATRIX.md` — OS × CLI × config-path lookup table for 9 hosts × 3 OSes
7. `spec/SECURITY.md` — threat model, defenses, audit log format
8. `spec/ABI_SKILL.md` v0.1
9. `spec/ABI_AGENT.md` v0.1
10. `spec/ABI_SANCHO_TASK.md` v0.1
11. `spec/ABI_MCP_TOOL.md` v0.1
12. `spec/ABI_MASCOT.md` v0.1
13. `spec/ABI_BOOTSTRAP_FRAGMENT.md` v0.1
14. Independent adversarial review pass #2 (agent reviewer against v2.0)
15. Lope ensemble validation pass (optional if review pass is clean)

**Success gate (Gate 0):**
- All 15 spec docs exist, lint-clean, under version control
- Review verdict is PASS (not NEEDS_FIX)
- All 21 locked decisions (D1-D21) have explicit rationale + "what would force a rebuild if reversed"
- Sebastian has read section 3 (locked decisions) and section 7 (this plan) and approved

**Time:** 1-2 days (6-16 hours of focused markdown work)
**Depends on:** nothing
**Risks:** scope creep (fix: time-box at 2 days hard); decision paralysis
  on open questions (fix: named defaults in spec, Sebastian decides yes/no
  not write-from-scratch)

### Phase B — Platform Adapter (2-3 days)

**Goal:** abstract OS differences behind a trait. Ship macOS + Linux impls.

**Deliverables:**
1. New crate `crates/platform/` with `PlatformAdapter` trait
2. `crates/platform/src/macos.rs` — launchd + Keychain + symlinks + Homebrew paths
3. `crates/platform/src/linux.rs` — systemd --user + Secret Service + XDG paths + symlinks
4. `crates/platform/src/windows.rs` — **stub only** with Dev Mode detection + Task Scheduler + Credential Manager + symlinks (real impl in Phase F)
5. `crates/platform/src/redox.rs` — stub that fails at runtime but compiles; ensures core stays target-clean
6. Refactor `crates/cli/src/commands/daemon.rs` to go through `PlatformAdapter`
7. CI matrix adds `x86_64-unknown-linux-gnu` as a build + test target
8. Integration test: `daemon install` → `daemon status` → `daemon uninstall` round trip on Linux + macOS

**Success gate (Gate 1):**
- `cargo test --workspace` green on macOS + Linux (CI enforced)
- `cargo check --target x86_64-unknown-redox -p makakoo-core` green (compile-only, no tests). **If rusqlite's bundled C build fails on Redox, this gate exposes the failure early; we fall back to `limbo`/`libsql` as documented in ARCHITECTURE.md §9 point 5 rather than discovering the blocker in Phase H.**
- Linux systemd daemon installs, starts, ticks SANCHO, stops cleanly
- macOS launchd path refactored, no regression on existing install
- Sebastian's install still works

**Time:** 2-3 days
**Depends on:** Phase A (needs `spec/PLATFORM_ADAPTER.md`)
**Risks:** systemd user units require `--user` flag gotchas (fix: test on
  fresh Linux VM before claiming done); cgroup/Restart= semantics bite
  (fix: match launchd KeepAlive behavior, document the difference)

### Phase C — Plugin Loader (3-4 days)

**Goal:** kernel discovers plugins from manifests, no hardcoded registrations.

**Deliverables:**
1. `crates/core/src/plugin/mod.rs` — `PluginManifest` struct, TOML parser
2. `crates/core/src/plugin/registry.rs` — `PluginRegistry` walks
   `$MAKAKOO_HOME/plugins/*/plugin.toml` at daemon start
3. `crates/core/src/plugin/resolver.rs` — semver dependency resolution +
   topological sort + ABI compatibility check
4. `crates/core/src/plugin/staging.rs` — atomic install via `.stage/` +
   blake3 hash verification + rename
5. Migration: move 16 hardcoded SANCHO task registrations from
   `crates/core/src/sancho/mod.rs::default_registry()` to manifest-driven
   discovery. Only the 8 native Rust tasks stay in `default_registry()` —
   the 3 watchdogs + 5 GYM tasks move to `plugins-core/*/plugin.toml`
6. Update `makakoo sancho status` to show both native + plugin-loaded tasks
7. Unit tests for manifest parsing, semver resolution, topo sort, staging
8. Integration test: drop a fake plugin into `plugins/test-plugin/`, daemon
   loads it, tick runs its task, uninstall removes it cleanly

**Success gate (Gate 2):**
- `makakoo sancho status` shows 16 tasks (8 native + 8 from manifests)
- No hardcoded subprocess registrations in `default_registry()` besides
  the 8 native Rust handlers
- Dropping a new plugin directory + restarting daemon auto-registers it
- Unit + integration tests green on macOS + Linux
- Sebastian's install still shows 16 tasks

**Time:** 3-4 days
**Depends on:** Phase A (needs `spec/PLUGIN_MANIFEST.md`); Phase B optional
  (can run parallel after B day 1)
**Risks:** semver resolver edge cases (fix: steal Cargo's algorithm, don't
  reinvent); topo sort on circular deps (fix: detect cycle, error with
  clear message); blake3 verification perf (fix: cache in plugins.lock)

### Phase D — Plugin Lifecycle Commands (1-2 days)

**Goal:** users can install, uninstall, update, list, and inspect plugins
via CLI.

**Deliverables:**
1. `makakoo plugin install <source>` — source is local path, git URL, or
   `<plugins-core-name>`. Stages, verifies, installs, registers
2. `makakoo plugin uninstall <name> [--purge]` — stops plugin, unregisters,
   removes directory. `--purge` also wipes state dir
3. `makakoo plugin list [--enabled] [--source]` — show installed plugins
4. `makakoo plugin info <name>` — show manifest + state + audit log summary
5. `makakoo plugin enable/disable <name>` — soft toggle without uninstall
6. `makakoo plugin update <name>` — re-fetch + reinstall with drain window
7. `makakoo distro install/save/list` — batch operations over plugin sets
8. Tab completion for all plugin names on macOS + Linux
9. Unit + integration tests

**Success gate (Gate 3):**
- Installing the 3 watchdog plugins + 5 GYM plugins from `plugins-core/`
  via `makakoo distro install core` works end-to-end
- Uninstalling one plugin leaves the others functional
- `makakoo plugin list` shows installed set with versions and capability
  grants
- Sebastian's install uses this pathway (his current install migrates to
  manifest-driven without losing any task)

**Time:** 1-2 days
**Depends on:** Phase C
**Risks:** partial install leaving stale state (fix: atomic staging, never
  rename unless fully validated); update-in-place breaking running plugins
  (fix: stop → update → start with drain window)

### Phase E — Capability Enforcement (3 days)

**Goal:** plugins bound to their declared capabilities via Unix socket + PID
check.

**Deliverables:**
1. `crates/core/src/capability/mod.rs` — capability verb enum + parser
2. `crates/core/src/capability/socket.rs` — per-plugin Unix domain socket
   creation, accept loop, PID verification
3. `crates/core/src/capability/audit.rs` — audit log writer to
   `$MAKAKOO_HOME/logs/audit.jsonl`
4. Client library `makakoo-client` in Rust + Python + Node — plugins call
   `client.brain_read()`, `client.llm_chat()`, etc. through the socket
5. Helper libs for brain, llm, net, state, secrets — all route through
   socket, check capability grants before serving
6. Windows named-pipe adapter (stubbed in Phase B, filled here)
7. Unit tests for socket, PID verification, capability parsing, audit log
8. Integration test: plugin without `net/http` grant cannot call HTTP helper

**Success gate (Gate 4):**
- Plugin that declares `brain/read + llm/chat + state/plugin` can do
  exactly those three things and nothing else via the helper libs
- Audit log shows every capability call with timestamp, plugin, verb, result
- Plugin crash doesn't leave orphaned sockets
- Sebastian's install has audit log populated

**Time:** 3 days
**Depends on:** Phase C
**Risks:** Unix socket permissions on restart (fix: atomically recreate
  on daemon start); Windows named pipe semantics differ
  (fix: abstract behind trait, test on real Windows); PID reuse after plugin
  restart (fix: kernel tracks PIDs across restarts)

### Phase F — Cross-OS Installer (4 days)

**Goal:** `makakoo install` works end-to-end on macOS, Linux, Windows.

**Deliverables:**
1. `install/install.sh` — macOS + Linux one-liner: detects OS, downloads
   right binary tarball from GitHub releases, extracts, runs `makakoo setup`
   if needed, exits
2. `install/install.ps1` — Windows one-liner: same logic, detects Dev Mode,
   refuses install without it
3. `makakoo install` umbrella command in `crates/cli/src/commands/install.rs`:
   does plugin install core distro + daemon install + infect auto-detect +
   symlink shared state + health check
4. `crates/cli/src/detect.rs` — CLI host detection for 7 CLIs × 3 OSes,
   lookup table driven by `spec/INSTALL_MATRIX.md`
5. Real Windows `PlatformAdapter` impl in `crates/platform/src/windows.rs`
   — Task Scheduler XML, Credential Manager, Dev Mode symlinks,
   `%LOCALAPPDATA%\Makakoo` paths
6. Real Infect impl updates for Windows: detect `%APPDATA%\Claude\`,
   `%USERPROFILE%\.cursor\`, etc.
7. CI matrix adds `x86_64-pc-windows-msvc` as build + test target
8. Integration test: `install.sh --dry-run` prints what it would do on
   each OS; real install on a test macOS + Linux + Windows VM

**Success gate (Gate 5):**
- Fresh macOS VM: `curl | sh && makakoo install` → all CLIs infected,
  daemon running, one-liner works
- Fresh Linux VM: same
- Fresh Windows VM: same (with Dev Mode toggle documented)
- All 7 AI CLIs detected + infected on all 3 OSes
- `makakoo uninfect <host>` restores host cleanly

**Time:** 4 days
**Depends on:** Phase B, Phase D
**Risks:** Windows MSVC toolchain setup in CI (fix: use GitHub Actions
  windows-latest runner); Windows Dev Mode not always available on locked-
  down corporate machines (fix: clear error with docs link); CLI detection
  table drifts as hosts update their config paths (fix: version the table,
  ship updates as part of distro updates)

### Phase G — Release Pipeline (3 days)

**Goal:** signed artifacts on GitHub Releases, install one-liners hosted
at makakoo.com/install.

**Deliverables:**
1. CI matrix: 4 targets (macOS x86_64 + aarch64, Linux x86_64 + aarch64,
   Windows x86_64) build + test on every push
2. GitHub Actions release workflow: tag → build all targets → sign → upload
3. macOS universal pkg with notarization via Apple's notary service
4. Windows MSI signed with Authenticode cert
5. Linux tarball, deb, rpm — published to GitHub Releases
6. Homebrew formula in `homebrew-makakoo` tap
7. winget manifest in winget-pkgs community repo
8. `install.sh` + `install.ps1` hosted at `makakoo.com/install` and
   `makakoo.com/install.ps1`
9. Release notes template + automated changelog from git tags

**Success gate (Gate 6):**
- `brew install makakoo/makakoo` works on macOS
- `winget install makakoo` works on Windows
- `curl -sSL makakoo.com/install | sh` downloads and runs
- All artifacts signed and verified
- GitHub Actions CI green on every commit

**Time:** 3 days
**Depends on:** Phase F
**Risks:** Apple notarization can fail for crypto-adjacent binaries
  (fix: allowlist our usage, use hardened runtime); Authenticode cert
  purchase (budget item, ~$300/year); winget review latency (fix: submit
  early)

### Phase H — Migration of Existing Plugins (2-3 days)

**Goal:** port every current MAKAKOO skill, agent, and plugin into the new
plugin-manifest form.

**Deliverables:**
1. Move `harvey-os/skills/meta/*/` → `plugins-core/skill-meta-*/` with
   `plugin.toml` for each
2. Move `harvey-os/skills/dev/*/` → `plugins-core/skill-dev-*/`
3. Move other skill categories similarly
4. Move `agents/*` submodules → `plugins-core/agent-*/` with manifests
5. Move `harvey-os/core/gym/` → `plugins-core/gym/`
6. Move 3 watchdogs → `plugins-core/watchdog-*/`
7. Write `distros/sebastian.toml` that reproduces Sebastian's current install
8. Sebastian's install runs `makakoo distro install sebastian` and gets
   back every plugin he had before, all manifest-driven
9. Retire the `harvey-os/` submodule + the 6 agent submodules after
   verifying nothing broke
10. Parent `~/MAKAKOO` becomes a symlink to `~/.makakoo/` runtime for
    compat during transition

**Success gate (Gate 7):**
- Sebastian's Brain, Superbrain, SANCHO registry, MCP tools, chat UI,
  btc-sniper, arbitrage agent, harveychat all functional via the new
  plugin system
- No more submodules in `~/MAKAKOO/`
- All git history preserved via `git subtree` merges
- Zero live-data loss — Brain markdown and state dirs migrated in place

**Time:** 2-3 days
**Depends on:** Phase C, D, E
**Risks:** in-flight state during migration (fix: do migration while
  daemon is stopped, take a Brain backup first); submodule history loss
  (fix: use `git subtree add` not `rm + mv`); Sebastian can't work during
  migration (fix: schedule migration window, ~4 hours dark time)

### Phase I — v0.1 Public Release Smoke + Launch (2 days)

**Goal:** prove the whole thing works end-to-end on a fresh VM for each
OS, then ship v0.1.

**Deliverables:**
1. Fresh macOS VM test: `curl | sh && makakoo install` → round trip
2. Fresh Ubuntu VM test: same
3. Fresh Windows 11 VM test: same
4. End-to-end demo video: install → infect Claude + Cursor → chat round
   trip showing shared state across both hosts
5. README.md at repo root with the 30-second elevator pitch + three install
   commands + screenshot
6. Public-facing install docs at `makakoo.com/install`
7. Quickstart guide: "Install Makakoo in 3 minutes"
8. v0.1.0 release tag + GitHub release notes
9. Announcement post draft (blog + social — Sebastian writes or approves)
10. Archive `spec/*.md` as frozen v1.0 contracts

**Success gate (Gate 8):**
- All 14 Definition of Done criteria from section 1 green
- Three fresh-VM smoke tests pass
- Sebastian's install survived the migration and uses the v0.1 binary
- Demo video demonstrates "one mind many bodies" on 2+ hosts

**Time:** 2 days
**Depends on:** Phase G, Phase H
**Risks:** VM testing reveals a missed case (fix: treat as Phase I work,
  don't ship until fixed); demo video shows a bug (fix: same)

### Phase J (future, optional) — Redox Port (2-3 days)

Deferred. Ship when the market is ready for bare-metal AI OS (or whenever
Sebastian says go).

**Deliverables:** Redox `recipe.toml`, `crates/platform/src/redox.rs` real
impl, CI target `x86_64-unknown-redox` runs tests (not just compiles),
smoke test on Redox VM.

## 8. Risk register (top 12, ranked by impact × likelihood)

| # | Risk | Impact | Likelihood | Mitigation |
|---|---|---|---|---|
| 1 | Windows Dev Mode rejection kills Windows tier | High | Medium | Document the one-click toggle prominently. Fallback: v0.1 ships as macOS + Linux only, Windows slips to v0.2 |
| 2 | Phase A decisions reveal deeper conflict post-lock | High | Low | Adversarial review pass + lope validation before lock. Gate 0 blocks advancement |
| 3 | systemd user unit doesn't restart-on-crash like launchd | Medium | Medium | Test on fresh VM before Gate 1. Fallback: cron-based watchdog |
| 4 | Apple notarization blocks macOS pkg | High | Medium | Hardened runtime from day one. Submit early for notarization lint |
| 5 | Plugin dependency cycles | Medium | Low | Detect in resolver, error with clear message. Core plugins audited manually |
| 6 | Unix socket PID verification spoofed on macOS (pid reuse) | Medium | Low | PID + start-time verification via `proc_pidpath` + `proc_pidinfo` |
| 7 | Sebastian's live install breaks during Phase H migration | High | Medium | Backup Brain before migration, do migration while daemon stopped, compat symlink from `~/MAKAKOO` |
| 8 | Plugin install script breaks on Windows PowerShell | Medium | Medium | Dual-shell pair + CI runs both on release |
| 9 | MCP gateway stdio locks up on Windows with Tokio | Medium | Low | Use blocking stdin bridge in MCP gateway on Windows; test in Phase F |
| 10 | Bootstrap Block drift across hosts (fragment order differs) | Low | Medium | Fragment order is deterministic (plugin install timestamp), cached, checksummed |
| 11 | Capability audit log fills disk | Low | Medium | Rotate at 100 MB, archive older, keep 7 days default |
| 12 | Lope validator thrash on spec prose wastes time | Low | High | Use agent reviewer instead, treat lope as optional |

## 9. Go/No-Go gates

The plan stops and reviews at each gate. If a gate is red, we fix before
advancing. No "proceed anyway".

- **Gate 0 (end of Phase A):** spec docs complete, review verdict PASS,
  Sebastian approved. Stops: Phase B.
- **Gate 1 (end of Phase B):** CI green on macOS + Linux, daemon installs
  + ticks cleanly. Stops: Phase C, Phase F.
- **Gate 2 (end of Phase C):** plugin loader discovers manifests, 16 tasks
  running, zero hardcoded subprocess registrations. Stops: Phase D, Phase E.
- **Gate 3 (end of Phase D):** plugin install/uninstall/list/info round
  trip working. Stops: Phase H.
- **Gate 4 (end of Phase E):** capability enforcement active, audit log
  populated, no-grant plugin blocked. Stops: Phase F complete, Phase G.
- **Gate 5 (end of Phase F):** `makakoo install` works on all 3 OSes,
  all 7 CLIs infected. Stops: Phase G.
- **Gate 6 (end of Phase G):** signed artifacts published, install
  one-liners work. Stops: Phase I.
- **Gate 7 (end of Phase H):** Sebastian's install fully migrated,
  nothing broken. Stops: Phase I.
- **Gate 8 (end of Phase I):** fresh-VM smoke tests pass on all 3 OSes,
  Definition of Done criteria 1-14 green. Stops: public release.

## 10. Deliverables matrix

| Phase | Rust code | Docs | Tests | External |
|---|---|---|---|---|
| A | — | 14 spec docs | — | Adversarial review |
| B | Platform trait + 3 impls (macOS real, Linux real, Windows stub, Redox stub) | PLATFORM_ADAPTER.md | Integration: daemon round trip | CI Linux runner |
| C | Plugin loader, registry, resolver, staging | Plugin loader module docs | Unit + integration | — |
| D | plugin + distro subcommands | User guide for plugin commands | Unit + integration | — |
| E | Capability socket + audit + client libs | capability guide | Unit + integration | — |
| F | install subcommand, Windows adapter, detection | install docs, INSTALL_MATRIX.md full | VM smoke on all 3 OSes | Windows CI runner |
| G | Release pipeline scripts | release docs | CI green on 4 targets | Notarization, signing cert, Homebrew tap, winget |
| H | — | migration guide | End-to-end smoke on Sebastian's install | git subtree work |
| I | — | README, quickstart, announcement | Three fresh-VM smokes | Blog post, demo video |

## 11. Cadence + reporting

- **Daily:** quick check-in in Brain journal — what shipped, what's blocked
- **Per-phase:** formal gate review with Go/No-Go decision
- **Weekly:** risk register refresh — are any risks changing likelihood
- **On every commit:** CI matrix runs, green-red status in journal
- **Per plugin migration (Phase H):** before/after verification that Brain
  + state + audit log are intact

## 12. Rollback plan

At any gate, if the gate is red and can't be fixed in <4 hours, we roll
back to the prior gate's state:

- **Before Phase B:** spec is markdown only, trivial rollback (git revert)
- **Before Phase C:** PlatformAdapter refactor reverts to macOS-only
- **Before Phase D:** plugin loader reverts, hardcoded registrations restored
- **Before Phase E:** plugin lifecycle commands disabled, hardcoded back
- **Before Phase F:** capability helpers bypass socket, use in-process calls
- **Before Phase G:** cross-OS installer disabled, macOS-only tarball
- **Before Phase H:** release pipeline unchanged, v0.1 slipped
- **Before Phase I:** migration aborted, Sebastian's install runs on v0.0.x

Every phase's rollback is tested in its own test suite before the gate
closes. No rollback is "hope it works."

## 13. Decisions locked on 2026-04-15

Sebastian approved v1.1 on 2026-04-15:

1. **21 locked decisions (D1-D21)** — approved as written in sections 3.1-3.8
2. **Time budget** — 4-5 weeks for Phases A-I, optional 2-3 days for Phase J (Redox)
3. **Windows tier** — first-class at v0.1, slip to v0.2 only if Dev Mode /
   CI blockers become unavoidable (reviewed at Gate 5)
4. **Public launch timing** — quiet soft launch after Phase I → tweak → public
5. **Phase A kickoff** — started immediately on 2026-04-15 (this commit)

Sebastian's instruction for the build: **"lope in the loop"** — when a
complex or important decision arises during execution, delegate judgment
to the team (lope ensemble OR adversarial agent review) before committing.
For prose spec work (Phase A), adversarial agent review has proven more
effective than lope-negotiate (which wedges on lint). For code work
(Phases B-H), lope ensemble is the preferred validator.

## 14. Minimum definition of "done" per phase

Every phase closes when it passes all three checks:

1. **Code is green.** `cargo test --workspace` on macOS + Linux (+ Windows
   from Phase F), clippy clean, CI matrix green
2. **Docs are complete.** Any manifest / ABI / capability / install change
   has a corresponding markdown update in `spec/`
3. **Sebastian's install still works.** Every phase ends with his current
   daemon ticking, his Brain intact, his infected hosts still infected.
   No "big bang" cutover allowed

Any phase that cannot hit all three rolls back to the previous gate per
section 12.

---

**End of master sprint plan v1.1. Status: LOCKED. Phase A in progress.**
