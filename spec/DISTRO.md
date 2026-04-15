# DISTRO — Opinionated Plugin Bundles

**Status:** v0.1 LOCKED — 2026-04-15
**Owner:** Makakoo kernel, `crates/core/src/distro/`
**Governs:** the format of distro files, the `makakoo distro` commands,
and the five shipped distros.

---

## 0. What a distro is

A distro is a **declarative, opinionated assembly** of kernel version +
plugins + defaults. It's how we ship "Makakoo Trader Edition" or
"Makakoo Researcher Edition" without re-shipping the kernel.

Analogy: a Linux distro is "Debian + GNOME + Firefox + some defaults."
A Makakoo distro is "makakoo kernel + X plugins + default persona config."

**A distro file is a single TOML file** under `distros/*.toml` in the
makakoo-os repo (or contributed by the user as a local file). Installing
a distro batch-installs all its plugins with their pinned hashes.

## 1. Distro file format

```toml
# distros/trader.toml

[distro]
name = "trader"
display_name = "Makakoo Trader Edition"
version = "0.1.0"
description = "For Polymarket + crypto momentum trading and market intelligence"
authors = ["Sebastian Schkudlara"]
license = "MIT"

# Include other distros — merge their plugin lists before applying this one.
# Chained includes are resolved recursively with cycle detection.
include = ["core.toml"]

# Kernel version constraint. Distro refuses to install if kernel doesn't match.
[kernel]
version = "^0.1"

# Plugin ship list. Each entry is a plugin name + semver + blake3 hash.
# Kernel refuses to install any plugin whose downloaded hash doesn't match.
[plugins]
"agent-arbitrage"              = { version = "^0.3", blake3 = "abcd1234..." }
"agent-btc-sniper"             = { version = "^0.1", blake3 = "efgh5678..." }
"skill-research-market-intel"  = { version = "*",    blake3 = "ijkl9012..." }
"skill-meta-autoimprover"      = { version = "*",    blake3 = "mnop3456..." }
"watchdog-polymarket-prices"   = { version = "^0.2", blake3 = "qrst7890..." }

# Optional: override defaults for the persona config
[defaults]
voice = "caveman"
"sancho.active_hours" = [6, 23]

# Optional: exclude plugins from any included distro
[excludes]
plugins = ["skill-creative-prose"]   # trader doesn't need this

# Optional: notes shown on install
[post_install]
message = """
Trader Edition installed. Before trading with real money:
1. Run `makakoo secret set POLYMARKET_API_KEY`
2. Configure risk limits via `makakoo plugin configure agent-arbitrage`
3. Start in dry-run mode for 1 week before enabling real trades
"""
```

## 2. The `include` chain

Distros can include other distros to avoid repeating plugin lists.
`trader.toml` includes `core.toml` which includes `minimal.toml`.

**Include semantics:**
- Chain is resolved recursively before applying the current distro
- Each included distro's `[plugins]` entries are merged into the
  effective list
- The current distro's entries override included entries (last wins)
- `[excludes]` in the current distro removes entries from the merged list
- Cycles in includes → refusal with a clear error
- Include order matters: `include = ["a.toml", "b.toml"]` merges a first,
  then b, then the current distro on top

## 3. Plugin pinning

Every `[plugins]` entry has three fields:

```toml
"plugin-name" = { version = "^0.3", blake3 = "abcd1234..." }
```

- **version** — semver constraint (`^0.3`, `>=1.0, <2.0`, `*`)
- **blake3** — content hash of the plugin tree at install time (optional
  for local-path plugins, required for git/tar plugins)

**Hash verification (D15):** when the kernel installs a plugin from a
distro, it downloads the source, computes the blake3 hash, and checks
against the distro's pin. Mismatch → refusal. This is supply chain
security: a tampered plugin cannot be silently installed.

**The `*` version** means "any version matching the hash." Useful for
plugins shipped in `plugins-core/` that evolve together with the kernel.

## 4. The five shipped distros

### 4.1 `distros/minimal.toml`

The bare-bones kernel install. Only what's required to boot the daemon
and run the 8 native Rust SANCHO tasks. No Python. No Python plugins.

```toml
[distro]
name = "minimal"
display_name = "Makakoo Minimal"
description = "Kernel-only install. Runs the 8 native Rust tasks. Zero Python."

[kernel]
version = "^0.1"

[plugins]
# Just the kernel's native handlers. No external plugins.
# This distro is mostly useful as an include base.
```

### 4.2 `distros/core.toml`

The default install for most users. Adds essential Python plugins
(brain, superbrain, gym, 3 watchdogs, 5 meta skills).

```toml
[distro]
name = "core"
display_name = "Makakoo Core"
description = "Essential plugins for a usable Harvey. The default install."

include = ["minimal.toml"]

[kernel]
version = "^0.1"

[plugins]
"brain"                            = { version = "*", blake3 = "..." }
"superbrain-py"                    = { version = "*", blake3 = "..." }
"llm"                              = { version = "*", blake3 = "..." }
"sancho-dispatch"                  = { version = "*", blake3 = "..." }
"gym"                              = { version = "*", blake3 = "..." }
"watchdog-switchailocal"           = { version = "*", blake3 = "..." }
"watchdog-postgres"                = { version = "*", blake3 = "..." }
"watchdog-hackernews"              = { version = "*", blake3 = "..." }
"skill-meta-caveman-voice"         = { version = "*", blake3 = "..." }
"skill-meta-autoimprover"          = { version = "*", blake3 = "..." }
"skill-meta-canary"                = { version = "*", blake3 = "..." }
"persona-makakoo-welcome"          = { version = "*", blake3 = "..." }
"mascot-olibia"                    = { version = "*", blake3 = "..." }
```

### 4.3 `distros/creator.toml`

Content creator bundle. Core + productivity + research + conversational
agent.

```toml
[distro]
name = "creator"
display_name = "Makakoo Creator"
description = "For people who write, design, research, and publish."

include = ["core.toml"]

[plugins]
"skill-productivity-inbox-triage"  = { version = "*", blake3 = "..." }
"skill-productivity-calendar"      = { version = "*", blake3 = "..." }
"skill-research-superbrain"        = { version = "*", blake3 = "..." }
"skill-research-web-browse"        = { version = "*", blake3 = "..." }
"skill-creative-prose"             = { version = "*", blake3 = "..." }
"skill-creative-diagrams"          = { version = "*", blake3 = "..." }
"agent-harveychat"                 = { version = "*", blake3 = "..." }
"agent-knowledge-extractor"        = { version = "*", blake3 = "..." }
```

### 4.4 `distros/trader.toml`

Trading bundle.

```toml
[distro]
name = "trader"
display_name = "Makakoo Trader"
description = "Polymarket + crypto momentum trading and market intel."

include = ["core.toml"]

[plugins]
"agent-arbitrage"                  = { version = "^0.3", blake3 = "..." }
"agent-btc-sniper"                 = { version = "^0.1", blake3 = "..." }
"skill-research-market-intel"      = { version = "*",    blake3 = "..." }
"skill-blockchain-polymarket"      = { version = "*",    blake3 = "..." }
"watchdog-polymarket-prices"       = { version = "^0.2", blake3 = "..." }

[defaults]
voice = "caveman"

[post_install]
message = """
Trader Edition installed. Before trading with real money:
1. Run `makakoo secret set POLYMARKET_API_KEY`
2. Configure risk limits via `makakoo plugin configure agent-arbitrage`
3. Start in dry-run mode for 1 week before enabling real trades
"""
```

### 4.5 `distros/sebastian.toml`

Sebastian's full-fat install. Reproduces the current MAKAKOO state as a
distro file. Can be imported on a new machine to recreate his whole rig.

```toml
[distro]
name = "sebastian"
display_name = "Sebastian Edition (Harvey, the full rig)"
description = "Everything Sebastian runs. A living snapshot of his install."

include = ["creator.toml", "trader.toml"]

[plugins]
# His additional plugins beyond creator + trader
"agent-multimodal-knowledge"       = { version = "*", blake3 = "..." }
"agent-career-manager"             = { version = "*", blake3 = "..." }
"agent-meta-harness"               = { version = "*", blake3 = "..." }
"skill-infrastructure-harvey-setup" = { version = "*", blake3 = "..." }
"skill-orchestration-harvey-swarm" = { version = "*", blake3 = "..." }
"skill-meta-lope-gate"             = { version = "*", blake3 = "..." }
"skill-system-network-wizard"      = { version = "*", blake3 = "..." }
"skill-system-security-wizard"     = { version = "*", blake3 = "..." }
# ... plus ~15 more skills

[defaults]
voice = "caveman"
persona_name = "Harvey"
pronoun = "he/him"
```

This distro file is generated automatically by `makakoo distro save
sebastian` when Sebastian's install state is snapshotted.

## 5. The `makakoo distro` commands

### 5.1 `makakoo distro list`

Show all available distros (shipped + user-defined).

```
Available distros:
  minimal     Makakoo Minimal     — kernel-only, 0 plugins
  core        Makakoo Core        — default install, 13 plugins
  creator     Makakoo Creator     — content creation, 21 plugins
  trader      Makakoo Trader      — Polymarket + crypto, 18 plugins
  sebastian   Sebastian Edition   — the full rig, 47 plugins

Installed: core (1 active distro)
```

### 5.2 `makakoo distro install <name>`

Install a distro by name. Reads the file, resolves includes, downloads
and verifies each plugin, installs them in dependency order, writes the
active distro to `$MAKAKOO_HOME/config/distro.toml`.

```
$ makakoo distro install trader
Resolving trader.toml (includes core.toml)
Plugins to install (18):
  - agent-arbitrage 0.3.1 (blake3:abcd1234...)
  - agent-btc-sniper 0.1.0 (blake3:efgh5678...)
  ... (etc)

Proceed? [y/N] y

Installing 1/18: agent-arbitrage
  downloading from github.com/traylinx/makakoo-arbitrage@v0.3.1
  verifying blake3... OK
  staging... OK
  running install.sh... OK (2.1s)
  registering... OK

... (etc)

Installed 18/18 plugins. Active distro: trader.

Post-install message:
  Trader Edition installed. Before trading with real money:
  1. Run `makakoo secret set POLYMARKET_API_KEY`
  ...
```

**Flags:**
- `--dry-run` — print what would happen without installing
- `--yes` — skip interactive confirmation
- `--from <path>` — install from a local distro file instead of the shipped ones

### 5.3 `makakoo distro save <name> [--out path]`

Snapshot the current install as a distro file. Writes
`distros/<name>.toml` (or the path given by `--out`).

```
$ makakoo distro save my-setup --out ~/.makakoo/my-setup.toml
Snapshotting current install:
  - 23 installed plugins
  - computing blake3 hashes... OK
  - resolving version constraints... OK
  - writing ~/.makakoo/my-setup.toml

Done. Import with: makakoo distro install --from ~/.makakoo/my-setup.toml
```

### 5.4 `makakoo distro update`

Re-resolve the active distro against current blake3 hashes and install
any updated plugins. Respects the semver constraints in the distro file.

```
$ makakoo distro update
Checking 18 plugins in trader distro...
  agent-arbitrage: 0.3.1 → 0.3.2 (available)
  skill-research-market-intel: up to date
  ... (etc)

Updates available for 3 plugins. Install? [y/N]
```

### 5.5 `makakoo distro switch <name>`

Uninstall the current distro's plugins and install a different distro.
Preserves plugin state dirs by default (retention=keep); use `--purge`
to wipe state.

**Dangerous operation** — shows a confirmation prompt with the diff.

```
$ makakoo distro switch minimal
Switching from trader to minimal.

Plugins to uninstall (17):
  agent-arbitrage, agent-btc-sniper, ...

Plugins to keep (1):
  brain (also in minimal)

State dirs to preserve (17):
  $MAKAKOO_HOME/state/arbitrage, ...

Proceed? [y/N]
```

## 6. Hash computation

blake3 hashes are computed over the **plugin source tree after download,
normalized**:
1. Walk the directory in lexicographic order
2. For each file: blake3 hash the contents, then blake3 hash
   `(relative_path, content_hash)`
3. blake3 combine all per-file hashes into the tree hash

This is the same pattern Cargo uses for crate registry verification.
Deterministic: two independent downloads of the same tag produce the
same hash.

**Exclusions:** `.git/`, `target/`, `node_modules/`, `__pycache__/`,
`.venv/` — everything under `.gitignore` effectively. The hash is the
*source*, not the build artifacts.

## 7. Community distros

Users can write their own distro files and share them. A community
distro is a TOML file published anywhere (GitHub, pastebin, local disk).
`makakoo distro install --from <url>` fetches it, prompts the user for
approval, and installs.

**Safety:** the user is shown the plugin list, all blake3 hashes, and a
warning that they're trusting a community distro. Default-deny; explicit
yes required.

In v0.2+ we may ship a blessed community registry at
`makakoo.com/distros.json` with vetted distros. For v0.1, users install
from direct URLs.

## 8. What happens without a distro

Running `makakoo install` without `--distro` uses `core.toml` by default.
Running `makakoo plugin install <name>` without any distro installs
ad-hoc, no pinning, no distro tracking.

**Rule:** at any given time, `$MAKAKOO_HOME/config/distro.toml` points
to exactly one active distro (or is empty). Ad-hoc plugin installs
are tracked in `$MAKAKOO_HOME/config/plugins.lock` but don't count as a
distro.

## 9. The `plugins.lock` file

Whenever a distro is installed or a plugin is installed ad-hoc, the
kernel writes `$MAKAKOO_HOME/config/plugins.lock`:

```toml
# Generated by makakoo — do not edit by hand
[meta]
distro = "trader"
kernel_version = "0.1.2"
generated_at = "2026-04-15T17:42:01Z"

[[plugin]]
name = "agent-arbitrage"
version = "0.3.1"
blake3 = "abcd1234..."
source = "git:https://github.com/traylinx/makakoo-arbitrage@v0.3.1"
installed_at = "2026-04-15T17:42:15Z"

[[plugin]]
name = "skill-research-market-intel"
version = "1.0.3"
blake3 = "efgh5678..."
source = "path:plugins-core/skill-research-market-intel"
installed_at = "2026-04-15T17:42:22Z"

# ... (etc)
```

This file is the source of truth for what's installed. `makakoo plugin
list` reads from it. Reinstalls use it as a cross-check. Git-friendly
for users who want to version their install state.

## 10. Versioning this doc

**v0.1:** ships Phase A (this doc).
**v1.0:** after Phase G when the shipped distros are live and hash-pinned.

Breaking changes to the distro format require a major version bump and
a migration path.

---

**Status:** v0.1 LOCKED. Next review at Phase D when `makakoo distro`
commands ship.
