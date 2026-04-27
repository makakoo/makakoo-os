# PLUGIN_MANIFEST — The plugin.toml schema

**Status:** v0.1 LOCKED — 2026-04-15
**Owner:** Makakoo kernel, `crates/core/src/plugin/manifest.rs`
**Governs:** the contract between plugins and the kernel. Every plugin
shipped for Makakoo OS ships a `plugin.toml` at the root of its directory
conforming to this schema.

---

## 0. Design principles

1. **TOML, not JSON or YAML.** TOML is human-editable, comment-friendly,
   and already the format Cargo uses. Users who touch plugin manifests
   will already know it.
2. **Flat where possible, nested only when it adds meaning.** `[plugin]`
   is flat; `[infect.fragments]` is nested because "fragments per host"
   is a meaningful grouping.
3. **Every field has a documented meaning.** No magic, no implicit
   behavior. If a field doesn't appear in this doc, the loader rejects
   the plugin.
4. **Forward-compatible by default.** Unknown top-level tables produce a
   warning, not a fatal error. Unknown fields inside known tables are
   errors. This lets us add new top-level sections (e.g. `[mascot]`,
   `[telemetry]`) without breaking old plugins.
5. **Redox-recipe-inspired.** Source format, dependency declaration,
   install template — all patterned on Redox `recipe.toml` because that
   system is battle-tested in a similar "ship code for another
   environment" context.

## 1. Top-level table summary

| Table | Required? | Purpose |
|---|---|---|
| `[plugin]` | Yes | Identity: name, version, kind, language, authors, license |
| `[source]` | Yes | Where the plugin source comes from (git, tar, local) |
| `[abi]` | Yes | Which ABI versions this plugin targets |
| `[depends]` | No | Other plugins + system requirements |
| `[install]` | No | How to install the plugin (dual-shell scripts) |
| `[entrypoint]` | Depends on kind | How the kernel invokes the plugin |
| `[capabilities]` | No | Which capability verbs the plugin uses |
| `[sancho]` | No | SANCHO tasks this plugin registers |
| `[mcp]` | No | MCP tools this plugin exposes |
| `[infect]` | No | Bootstrap Block fragments contributed to infected hosts |
| `[mascot]` | No | Mascot declaration (species + stats) — only if kind=mascot |
| `[state]` | No | Plugin state dir policy |
| `[test]` | No | How to test the plugin |
| `[embedding]` | No | Embedding model + dim — reserved for superbrain-style plugins |

## 2. The `[plugin]` table (identity)

**Required fields.**

```toml
[plugin]
name = "arbitrage"              # globally unique plugin name (lowercase + dashes)
version = "0.3.1"               # semver
kind = "agent"                  # skill | agent | sancho-task | mcp-tool | mascot | bootstrap-fragment
language = "python"             # python | rust | node | shell | binary
```

**Optional fields.**

```toml
summary = "Polymarket BTC momentum trading agent"
description = """
Multi-paragraph description used by `makakoo plugin info`.
Supports markdown.
"""
authors = ["Sebastian Schkudlara <seb@traylinx.com>"]
license = "MIT"                 # SPDX identifier
homepage = "https://github.com/traylinx/makakoo-arbitrage"
repository = "https://github.com/traylinx/makakoo-arbitrage"
keywords = ["trading", "polymarket", "btc"]
```

**Name rules.**
- Lowercase, digits, and single dashes. No dots, underscores, or uppercase.
- Must match regex `^[a-z][a-z0-9-]{1,62}$`
- Reserved prefixes: `core-*` (kernel-shipped plugins), `official-*`
  (kernel-team maintained but not shipped by default). Community plugins
  use anything else.

**Kind rules.**
- Exactly one kind per plugin. A plugin that wants to act as both agent
  and mcp-tool should ship as `kind = "agent"` and declare its MCP tools
  in `[mcp]`. Kind is the *primary* role, not an exclusive one.

**Language rules.**
- `python` — requires Python version declared in `[depends]`
- `rust` — built to a binary at install time via `cargo build --release`
- `node` — requires Node version declared in `[depends]`
- `shell` — posix shell entrypoint + PowerShell companion on Windows
- `binary` — pre-built executable shipped in the plugin directory

## 3. The `[source]` table (where the code comes from)

**Exactly one of `git`, `tar`, or `path` is required.**

```toml
[source]
# Option A: git
git = "https://github.com/traylinx/makakoo-arbitrage"
rev = "v0.3.1"                  # branch | tag | commit SHA
blake3 = "abcd1234..."          # hash of the resolved tree (supply chain pin)

# Option B: tar (like Redox recipes)
tar = "https://github.com/traylinx/makakoo-arbitrage/archive/v0.3.1.tar.gz"
blake3 = "abcd1234..."

# Option C: local path (for plugins shipped inside makakoo-os itself)
path = "plugins-core/arbitrage"
```

**Hash pinning.** The `blake3` field is required for `git` and `tar`
sources when the plugin is listed in a distro file (see DISTRO.md). For
ad-hoc local installs (`makakoo plugin install ./local-dir`), the hash
is optional but the kernel computes and displays it so the user can pin
later.

**Rev rules.**
- Branches are allowed but produce a warning on install ("tracking a
  moving ref")
- Tags and commit SHAs are the recommended pinning strategy
- `rev` is ignored for `path` sources

## 4. The `[abi]` table (which ABIs this plugin targets)

**Required.**

```toml
[abi]
# One line per ABI the plugin targets. Semver constraints follow Cargo syntax.
skill = "^0.1"
agent = "^0.1"
sancho-task = "^0.1"
mcp-tool = "^0.1"
mascot = "^0.1"
bootstrap-fragment = "^0.1"
```

Plugins only declare ABIs they actually implement. An `kind = "agent"`
plugin typically declares `agent` + optional `sancho-task` (for
scheduled work) + optional `mcp-tool` (for gateway exposure) + optional
`bootstrap-fragment` (for host customization). A `kind = "skill"` plugin
typically just declares `skill`.

**Kernel behavior:** at plugin load, kernel checks every declared ABI
against its own supported versions. Mismatch (e.g. plugin declares
`agent = "^2.0"` but kernel only supports `agent = "0.1"`) → refusal
with clear error.

## 5. The `[depends]` table (dependencies)

**Optional but strongly recommended.**

```toml
[depends]
plugins = ["brain ^1.0", "llm ^1.0", "superbrain-py ^0.3"]
python = ">=3.11"
node = ">=20.0"
rust = ">=1.75"                 # for rust-language plugins
binaries = ["git", "curl"]      # must be on PATH
system = ["libssl >= 1.1"]      # OS package requirement (informational)

[depends.packages]
# Language-native package dependencies, installed inside the plugin's
# sandbox (venv for Python, node_modules for node, Cargo for rust).
python = ["ccxt>=4.0", "numpy>=1.24", "pandas"]
node = ["axios", "commander"]
rust = ["clap = '4'"]
```

**Resolution behavior:**
- `plugins` deps are validated against installed plugins at load time.
  Missing or version-incompatible plugin deps → refusal.
- Language-runtime deps (`python`, `node`, `rust`) are checked against
  the actual runtime on the machine. Missing or wrong version → refusal
  with "install Python 3.11 via `brew install python@3.11`" style hint.
- `binaries` are probed via `which` / `where`. Missing → refusal.
- `system` is informational only; kernel doesn't enforce. It's for
  users debugging "why doesn't this plugin work."
- `[depends.packages]` is installed by the plugin's `[install]` script
  using the language's native package manager.

## 6. The `[install]` table (how to build/install)

**Optional. Required for any plugin where `[source]` points to source
code that needs compilation or package installation.**

```toml
[install]
unix = "install.sh"             # POSIX shell script, relative to plugin root
windows = "install.ps1"         # PowerShell script, relative to plugin root
```

**Dual-shell rule (D6):** every plugin ships `install.sh` + `install.ps1`
as a pair. No portable install DSL. Kernel picks the right one based on
host OS:
- macOS, Linux, Redox → `unix`
- Windows → `windows`

**The install script contract:**
- Runs inside the staged plugin directory (`$MAKAKOO_HOME/plugins/.stage/<name>/`)
- Has `$MAKAKOO_HOME` env var available
- **Does NOT have `$MAKAKOO_SOCKET_PATH`.** The capability socket is
  created AFTER install succeeds (plugin lifecycle §7.3 steps 8 → 11).
  Install scripts run in a pre-capability context; they can use the
  filesystem and PATH but cannot call brain/llm/net helpers.
- Should write all build artifacts to the plugin dir (no system-wide
  installation)
- Should not require sudo / admin. If it needs elevated permissions,
  the plugin is probably the wrong shape.
- Must exit 0 on success, non-zero on failure
- Stdout + stderr are captured and shown to the user on failure
- Optional `on_install` hook (distinct from `install`) runs AFTER the
  socket is available if you need to do capability-scoped first-time
  setup (e.g. writing an initial Brain page). See ABI_SKILL.md §4.

**Example `install.sh`:**
```sh
#!/usr/bin/env bash
set -euo pipefail

python3 -m venv .venv
./.venv/bin/pip install --quiet -e .
```

**Example `install.ps1`:**
```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

python -m venv .venv
& .\.venv\Scripts\pip.exe install --quiet -e .
```

## 7. The `[entrypoint]` table

**Required for kinds:** `skill`, `agent`, `sancho-task`.
**Not required for kinds:** `mcp-tool`, `bootstrap-fragment`, `mascot`
(those declare their entrypoints in their kind-specific tables).

```toml
[entrypoint]
# For agents: lifecycle commands
start  = ".venv/bin/python -m arbitrage.main --start"
stop   = ".venv/bin/python -m arbitrage.main --stop"
health = ".venv/bin/python -m arbitrage.main --health"

# For skills: one-shot invocation
run = ".venv/bin/python -m caveman_voice"

# For sancho-task: invoked with --task <name>
run = ".venv/bin/python -m arbitrage.sancho"
```

All commands run inside the plugin dir with the capability socket path
in env. Commands can reference the plugin dir via `./` (relative paths
are resolved from the plugin root).

## 8. The `[capabilities]` table

**Optional but always written in practice.** Plugins that omit this get
zero capabilities (can't call brain, llm, net, state — effectively
useless).

```toml
[capabilities]
grants = [
  "brain/read",
  "brain/write",
  "llm/chat",
  "llm/chat:minimax/ail-compound",   # scoped: only this model
  "llm/embed",
  "net/http",                         # unrestricted HTTP
  "net/http:https://clob.polymarket.com/*",  # scoped
  "state/plugin",                     # own state dir (every plugin gets this by default if [state] exists)
  "secrets/read:AIL_API_KEY",
  "exec/binary:git",
]
```

Full verb vocabulary in `CAPABILITIES.md`. Kernel enforcement semantics
in `ARCHITECTURE.md §8`.

**Scoping:**
- `verb` alone → unrestricted (e.g. `net/http` = can call any URL)
- `verb:scope` → scoped (e.g. `net/http:https://api.example.com/*`)
- Scopes use glob syntax, not regex

**Default grant:** every plugin with a `[state]` dir automatically gets
`state/plugin` on its own directory. Does not need to be declared.

## 9. The `[sancho]` table

**Optional. Used by plugins that contribute scheduled tasks.**

```toml
[sancho]
tasks = [
  { name = "arbitrage_tick", interval = "300s", active_hours = [6, 23] },
  { name = "arbitrage_evening_report", interval = "24h", active_hours = [21, 23], gates = ["session", "lock"] },
]
```

**Task fields:**
- `name` — task identifier, unique across all plugins + native kernel tasks
- `interval` — human-readable duration (`"5m"`, `"300s"`, `"24h"`, `"7d"`)
- `active_hours` — two-element array `[start_hour, end_hour]` in
  24h local time (optional)
- `weekdays` — array of day names (`["mon", "tue", ...]`), optional
- `gates` — list of additional gate names (`session`, `lock`, `time`,
  `weekday`, `active_hours`), optional

Kernel registers each task with the SANCHO scheduler at plugin load
time. Task is invoked via `[entrypoint].run --task <name>`.

## 10. The `[mcp]` table

**Optional. Used by plugins that expose MCP tools through the kernel gateway.**

```toml
[mcp]
tools = [
  { name = "arbitrage_status", handler = "arbitrage.mcp:status", schema = "schemas/status.json" },
  { name = "arbitrage_tick_now", handler = "arbitrage.mcp:tick_now", schema = "schemas/tick.json" },
]
```

**Tool fields:**
- `name` — tool name exposed through MCP (unique across all plugins)
- `handler` — language-specific handler reference (Python:
  `module:function`, Rust: `crate::function`, etc.)
- `schema` — optional JSON schema file for input/output validation

At plugin load, kernel registers each tool with the MCP gateway. The
gateway fans out to every infected host automatically.

## 11. The `[infect]` table

**Optional. Used by plugins that contribute Bootstrap Block fragments.**

```toml
[infect.fragments]
default = "fragments/default.md"
claude  = "fragments/claude-voice.md"
cursor  = "fragments/cursor-diff.md"
gemini  = "fragments/gemini-research.md"
```

Fragment files are markdown with sentinel section markers:
```markdown
<!-- makakoo:fragment:my-fragment-name -->
Content here.
<!-- makakoo:fragment:my-fragment-name-end -->
```

Fragment section names must be globally unique across all installed
plugins. Collision on install → refusal (D14). See PARASITE.md §5 for
rendering behavior.

## 12. The `[mascot]` table

**Required when `[plugin].kind = "mascot"`.**

```toml
[mascot]
species = "sloth"
stats = { debugging = 45, patience = 88, snark = 20 }
patrol = "patrol.py::patrol_tick"
flavor = "A slow but thorough syntax checker."
```

See `ABI_MASCOT.md` for full semantics.

## 13. The `[state]` table

**Optional. Used by any plugin that keeps state.**

```toml
[state]
dir = "$MAKAKOO_HOME/state/arbitrage"     # always resolves to this path
retention = "keep"                         # keep | purge_on_uninstall
```

**Retention modes:**
- `keep` (default) — on uninstall, state dir is left behind. User can
  reinstall the plugin later and pick up where they left off. Can be
  manually purged via `makakoo plugin uninstall <name> --purge`.
- `purge_on_uninstall` — state dir is deleted on uninstall without
  prompting. Used for plugins that keep only ephemeral state.

## 14. The `[test]` table

**Optional. For CI.**

```toml
[test]
command = ".venv/bin/pytest"
timeout = "5m"
```

`makakoo plugin test <name>` runs the command. Not invoked at install
time — only when user explicitly asks.

## 15. The `[embedding]` table

**Reserved for superbrain-style plugins that own an embedding model.**

```toml
[embedding]
model = "qwen3-embedding:0.6b"
dim = 768
provider = "switchailocal"
```

Used by D12 (embedding model lock). The superbrain plugin declares this
and re-embed is required on version bump.

## 16. Six worked examples

### 16.1 A skill plugin (`kind = "skill"`)

```toml
[plugin]
name = "skill-meta-caveman-voice"
version = "1.2.0"
kind = "skill"
language = "python"
summary = "Terse, token-efficient response mode for internal work"
authors = ["Makakoo OS contributors"]
license = "MIT"

[source]
path = "plugins-core/skill-meta-caveman-voice"

[abi]
skill = "^0.1"
bootstrap-fragment = "^0.1"

[depends]
python = ">=3.11"

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
run = ".venv/bin/python -m caveman_voice"

[capabilities]
grants = ["brain/read"]

[infect.fragments]
default = "fragments/caveman-voice.md"
```

### 16.2 An agent plugin (`kind = "agent"`)

```toml
[plugin]
name = "agent-arbitrage"
version = "0.3.1"
kind = "agent"
language = "python"
summary = "Polymarket BTC momentum trading agent"
authors = ["Sebastian Schkudlara <seb@traylinx.com>"]
license = "MIT"

[source]
git = "https://github.com/traylinx/makakoo-arbitrage"
rev = "v0.3.1"
blake3 = "abcd1234..."

[abi]
agent = "^0.1"
sancho-task = "^0.1"
mcp-tool = "^0.1"

[depends]
plugins = ["brain ^1.0", "llm ^1.0"]
python = ">=3.11"
[depends.packages]
python = ["ccxt>=4.0", "numpy>=1.24"]

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
start = ".venv/bin/python -m arbitrage.main --start"
stop = ".venv/bin/python -m arbitrage.main --stop"
health = ".venv/bin/python -m arbitrage.main --health"

[capabilities]
grants = [
  "brain/read", "brain/write",
  "llm/chat:minimax/ail-compound",
  "net/http:https://clob.polymarket.com/*",
  "net/http:https://data-api.polymarket.com/*",
  "state/plugin",
  "secrets/read:POLYMARKET_API_KEY",
]

[sancho]
tasks = [
  { name = "arbitrage_tick", interval = "300s", active_hours = [6, 23] },
  { name = "arbitrage_evening_report", interval = "24h", active_hours = [21, 23] },
]

[mcp]
tools = [
  { name = "arbitrage_status", handler = "arbitrage.mcp:status" },
  { name = "arbitrage_tick_now", handler = "arbitrage.mcp:tick_now" },
]

[state]
dir = "$MAKAKOO_HOME/state/arbitrage"
retention = "keep"

[test]
command = ".venv/bin/pytest"
```

### 16.3 A sancho-task plugin (`kind = "sancho-task"`)

```toml
[plugin]
name = "watchdog-postgres"
version = "1.0.0"
kind = "sancho-task"
language = "python"
summary = "Restart Postgres if any cluster is down"

[source]
path = "plugins-core/watchdog-postgres"

[abi]
sancho-task = "^0.1"

[depends]
python = ">=3.11"
binaries = ["pg_ctl"]

[install]
unix = "install.sh"
windows = "install.ps1"    # Postgres on Windows — rare but supported

[entrypoint]
run = "python3 -m pg_watchdog"

[capabilities]
grants = [
  "exec/binary:pg_ctl",
  "state/plugin",
  "brain/write",
]

[sancho]
tasks = [
  { name = "pg_watchdog", interval = "900s" },
]

[state]
dir = "$MAKAKOO_HOME/state/watchdog-postgres"
retention = "purge_on_uninstall"
```

### 16.4 An mcp-tool plugin (`kind = "mcp-tool"`)

```toml
[plugin]
name = "mcp-github"
version = "0.1.0"
kind = "mcp-tool"
language = "rust"
summary = "GitHub API tools (issues, PRs, gists) for every infected host"

[source]
git = "https://github.com/makakoo/mcp-github"
rev = "v0.1.0"
blake3 = "abcd..."

[abi]
mcp-tool = "^0.1"

[depends]
rust = ">=1.75"

[install]
unix = "install.sh"     # runs cargo build --release
windows = "install.ps1"

[capabilities]
grants = [
  "net/http:https://api.github.com/*",
  "secrets/read:GITHUB_TOKEN",
]

[mcp]
tools = [
  { name = "github_issue_list", handler = "target/release/mcp-github issue-list" },
  { name = "github_issue_create", handler = "target/release/mcp-github issue-create" },
  { name = "github_pr_review", handler = "target/release/mcp-github pr-review" },
]
```

### 16.5 A mascot plugin (`kind = "mascot"`)

```toml
[plugin]
name = "mascot-olibia"
version = "2.1.0"
kind = "mascot"
language = "python"
summary = "The official Makakoo mascot — Olibia the seal"

[source]
path = "plugins-core/mascot-olibia"

[abi]
mascot = "^0.1"
sancho-task = "^0.1"

[depends]
python = ">=3.11"
plugins = ["brain ^1.0"]

[install]
unix = "install.sh"
windows = "install.ps1"

[entrypoint]
run = ".venv/bin/python -m olibia.patrol"

[capabilities]
grants = ["brain/read", "brain/write", "state/plugin"]

[mascot]
species = "seal"
stats = { friendliness = 95, patience = 80, snark = 20 }
patrol = "olibia.patrol:tick"

[sancho]
tasks = [
  { name = "olibia_patrol", interval = "3600s" },
]

[state]
dir = "$MAKAKOO_HOME/state/mascot-olibia"
```

### 16.6 A bootstrap-fragment plugin (`kind = "bootstrap-fragment"`)

```toml
[plugin]
name = "persona-makakoo-welcome"
version = "1.0.0"
kind = "bootstrap-fragment"
language = "shell"             # no runtime, just static content
summary = "The default welcome fragment shipped with every Makakoo install"

[source]
path = "plugins-core/persona-makakoo-welcome"

[abi]
bootstrap-fragment = "^0.1"

[infect.fragments]
default = "fragments/welcome.md"
```

No entrypoint, no capabilities, no state. Just ships a fragment file.
Installed by the `core` distro at `makakoo install`.

## 17. Validation rules (kernel enforcement at load time)

The kernel's manifest parser refuses to load a plugin when:

1. `plugin.toml` fails TOML parsing
2. Required fields in `[plugin]` (`name`, `version`, `kind`, `language`) missing
3. `[plugin].name` fails the regex
4. `[source]` has zero of `git`/`tar`/`path`
5. `[source].git` or `[source].tar` is listed in a distro file without `blake3`
6. `[source].blake3` mismatch with downloaded content
7. `[abi]` missing or declares ABIs the kernel doesn't support
8. `[depends].plugins` references a plugin not installed or
   version-incompatible
9. `[depends].python`/`node`/`rust` version check fails on the runtime
10. `[depends].binaries` can't be found on PATH
11. `[install].unix` or `.windows` missing when compilation is needed
12. `[entrypoint]` missing required subkeys for the plugin's kind
13. `[capabilities].grants` references unknown verbs (not in
    `CAPABILITIES.md` verb vocabulary)
14. `[sancho].tasks` has duplicate `name` across plugins
15. `[mcp].tools` has duplicate `name` across plugins
16. `[infect].fragments` has a fragment name colliding with another
    plugin's fragment
17. Unknown field inside a known table (forward-compat is at the table
    level, not the field level)

Warnings (non-fatal):
- Unknown top-level table (future version expected to use it)
- `[source].rev` is a branch (moving target)
- `[depends].system` unresolvable (informational only)
- Plugin name has a reserved prefix (`core-*`, `official-*`) but source
  is not the makakoo-os monorepo

## 18. Versioning of the manifest schema itself

This document is **v0.1**. Schema changes go through the same semver
rules as ABIs:
- **Patch** (0.1.0 → 0.1.1): typo fix, clarification, documentation
- **Minor** (0.1.0 → 0.2.0): new optional field or table
- **Major** (0.1.0 → 1.0.0): renamed field, removed field, changed
  semantics of existing field

The kernel refuses to load a plugin whose `plugin.toml` targets a schema
version it doesn't support. (Schema version is carried implicitly via
the `abi` declarations plus the known top-level table set.)

**Promotion from v0.1 to v1.0** happens at end of Phase E, after at
least one plugin of each kind has been dogfooded through the full
lifecycle.

---

**Status:** v0.1 LOCKED. Next review at Phase C when the manifest parser
lands in Rust.
