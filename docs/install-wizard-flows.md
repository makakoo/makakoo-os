# Install + Wizard — every flow, every use case

This is the **single comprehensive reference** for installing Makakoo OS and walking the setup wizard. Every command verified live against `v0.1.0` (bootstrap v12, infect pointer pattern, 2026-04-25).

If you only have 30 seconds: jump to [§1.1 The fastest path](#11-the-fastest-path-fresh-mac-or-linux). Everything else is for when something doesn't fit that path.

---

## Table of contents

- [§1 — Install paths](#1-install-paths) — pick your starting point
- [§2 — What `makakoo install` actually does](#2-what-makakoo-install-actually-does)
- [§3 — `makakoo setup` wizard, section by section](#3-makakoo-setup-wizard-section-by-section)
- [§4 — Per-CLI infect behavior](#4-per-cli-infect-behavior)
- [§5 — Verification](#5-verification--did-it-actually-work)
- [§6 — Re-running, upgrading, uninstalling](#6-re-running-upgrading-uninstalling)
- [§7 — Common situations](#7-common-situations--decision-tree)
- [§8 — Troubleshooting](#8-troubleshooting)
- [§9 — Reference: every wizard prompt](#9-reference-every-wizard-prompt)

---

## §1 — Install paths

There are three valid ways to install. Pick by your context, not by preference.

### 1.1 The fastest path — fresh Mac or Linux

You have a clean machine, you trust the install script, you want one command:

```sh
curl -fsSL https://makakoo.com/install | sh
```

This bootstraps cargo, builds + installs `makakoo` and `makakoo-mcp`, runs `makakoo install` (distro + daemon + infect + health check), and starts the setup wizard. End state: every AI CLI on the machine knows about Makakoo.

**Caveats**:
- Public install URL is live **after** the v0.1.0 release tag lands. Until then use §1.3 (from source).
- Requires a working `cargo` (or willingness to install Rust toolchain).

### 1.2 Windows — PowerShell one-liner

Developer Mode must be on (`Settings → For developers → Developer Mode`). Then:

```powershell
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
```

Same end state as §1.1. Developer Mode is required because Makakoo creates symlinks for the auto-memory shared store.

### 1.3 From source — works today, every platform

For when you want to inspect the code before running it, contribute, or you're pre-v0.1.0:

```sh
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo install --path makakoo
cargo install --path makakoo-mcp
makakoo install
makakoo setup           # interactive wizard
```

`cargo install` builds in release mode, ~3-5 minutes on a modern Mac. The two binaries land in `~/.cargo/bin/`.

### 1.4 Already have Makakoo? (re-install / upgrade)

```sh
cd makakoo-os
git pull
cargo install --path makakoo --force
cargo install --path makakoo-mcp --force
makakoo install         # re-runs distro + infect; idempotent
```

Note: **MCP children don't auto-upgrade** — restart any open AI CLI session after upgrading `makakoo-mcp` so the new binary is loaded.

---

## §2 — What `makakoo install` actually does

`makakoo install` is the one-shot orchestrator. It does **four** things in order, each idempotent:

### Step 1 — distro install

A distro is a curated bundle of plugins. The default is `core` (~30 plugins: brain tools, search, MCP server, mascot system). Other choices: `minimal`, `sebastian`, `creator`, `trader`. `makakoo install` picks `core` unless you've already chosen one.

```sh
makakoo install                       # uses default distro 'core'
makakoo install --distro minimal      # ~10 plugins, no mascots, no SANCHO tasks
makakoo install --distro sebastian    # personal full-stack (CV, career, arbitrage)
makakoo distro list                   # see what's available
```

End state: plugins extracted to `$MAKAKOO_HOME/plugins/`, manifest validated.

### Step 2 — daemon registration

A background daemon manages the SANCHO proactive task engine, plugin lifecycle hooks, and the unix-domain-socket capability gateway.

- **macOS**: registers a `LaunchAgent` at `~/Library/LaunchAgents/com.makakoo.daemon.plist`
- **Linux**: writes a user systemd unit at `~/.config/systemd/user/makakoo-daemon.service` and `systemctl --user enable --now makakoo-daemon`
- **Windows**: adds an auto-launch entry under `HKCU\…\Run` and starts the daemon

You can verify with `makakoo daemon status`. To pause: `makakoo daemon stop`. To remove: `makakoo daemon uninstall`.

### Step 3 — global infect

Writes the bootstrap pointer block into every CLI's global instructions slot. v12 pointer pattern (since 2026-04-25): each slot holds a 15-line pointer to `$MAKAKOO_HOME/bootstrap/global.md`, the canonical source of truth.

| CLI | Slot file | Format |
|---|---|---|
| Claude Code | `~/.claude/CLAUDE.md` | Markdown |
| Gemini CLI | `~/.gemini/GEMINI.md` | Markdown |
| Codex (OpenAI) | `~/AGENTS.md` + `~/.codex/config.toml model_instructions_file` | Markdown + TOML key |
| OpenCode | `~/.config/opencode/opencode.json` (`instructions[]`) | JSON entry |
| Vibe (Mistral) | `~/.vibe/instructions.md` | Markdown |
| Cursor | `~/.cursor/rules.md` | Markdown |
| Qwen Code | `~/.qwen/QWEN.md` | Markdown |
| pi (badlogic) | `~/.pi/AGENTS.md` | Markdown |

For Codex specifically, the pointer block alone is not enough — Codex doesn't walk up to `~/AGENTS.md` from arbitrary cwds. So infect ALSO sets `model_instructions_file = "$MAKAKOO_HOME/bootstrap/global.md"` in `~/.codex/config.toml`. That key is Codex's official "load custom system instructions every session" knob (replacing the deprecated `experimental_instructions_file`).

If a CLI isn't installed (no `~/.<cli>/` dotdir), infect skips it cleanly. Re-runs on the same CLI are no-ops if content matches.

### Step 4 — health check

A short audit that confirms:
- Each slot has the expected v12 marker block
- The MCP server is registered in every detected CLI's MCP config
- The canonical bootstrap exists at `$MAKAKOO_HOME/bootstrap/global.md`
- The daemon is running and reachable on its socket

Output is a human-readable summary, exit code 0 if healthy, exit code 1 + diagnostic if drift detected.

---

## §3 — `makakoo setup` wizard, section by section

`makakoo setup` is the interactive configuration tool. It runs after `makakoo install`, but you can run it any time — it picks up from where you left off (each section is independently committable).

It has **six sections**. You can run all of them sequentially, or skip to one with `--only <name>`.

```sh
makakoo setup                          # all sections, interactive
makakoo setup --only persona           # just the persona section
makakoo setup --skip terminal,brain    # skip those two
makakoo setup --non-interactive        # use defaults, no prompts
makakoo setup --reset                  # clear stored answers and re-prompt
```

### Section 1 — `persona`

Tells Makakoo who YOU are and what to call your AI assistant. Defaults to "Harvey" (Sebastian's persona); can be any name.

Asks:
- Persona name (default: `Harvey`)
- User name (default: from `git config user.name` or `whoami`)
- Tone (sharp/blunt/casual/formal — default `sharp`)

Writes to `$MAKAKOO_HOME/config/persona.json`. Read at every CLI session start by the bootstrap.

**Use case**: you want the AI to call you by name and identify as your custom persona instead of "Harvey".

### Section 2 — `brain`

Configures where your Brain lives. Logseq is the default (Markdown outliner format with `[[wikilinks]]`). You can also point at an existing Obsidian vault or a plain folder.

Asks:
- Brain location (default: `$MAKAKOO_HOME/data/Brain`)
- Format (logseq / obsidian / plain — default `logseq`)
- Whether to seed with starter pages (default: yes)

Initializes the journals dir + pages dir. You can add more vaults later with `makakoo brain add <path>`.

**Use case**: you already use Obsidian for personal notes and want Makakoo to read/write into your existing vault instead of creating a new one.

### Section 3 — `cli-agent`

Tells Makakoo which AI CLI hosts to detect + infect. By default, every CLI present on PATH gets infected. You can opt-out individual CLIs here.

Asks for each detected CLI: include or skip?

This section drives the `infect --global` step. Re-running it after installing a new CLI (e.g. you just installed Cursor) re-detects and adds it to the infected set.

**Use case**: you have Codex installed but don't want Makakoo to touch it. Skip it here; infect will leave its slot alone forever.

### Section 4 — `terminal`

Optional: configures your terminal emulator (Ghostty preferred, but works with iTerm2, Alacritty, Warp, kitty) so Makakoo can open new tabs / panes for parallel agent runs.

Asks:
- Terminal binary (auto-detected; confirm or pick from list)
- Auto-spawn behavior (default: ask each time)

Skip this section if you only ever run one Makakoo command at a time.

**Use case**: you run multiple agents in parallel and want SANCHO/swarm to open new panes automatically.

### Section 5 — `model-provider`

The most consequential section — without an LLM provider, `makakoo query`, `makakoo dream`, semantic memory, and most agents won't function.

Choices, in order of recommendation:
1. **switchAILocal** (default): a local LLM gateway running on `localhost:18080` — free, private, fast. Requires `switchAILocal` running.
2. **Anthropic API** — direct to Claude. Needs an API key.
3. **OpenAI API** — direct to GPT. Needs an API key.
4. **Local Ollama** — point at a running ollama daemon.
5. **Custom** — any OpenAI-compatible endpoint.

Asks:
- Provider choice
- API key (stored in OS keyring via `makakoo secret set AIL_API_KEY`)
- Default model (e.g. `ail-compound`, `claude-sonnet-4-6`)

After config is saved, the wizard runs a smoke test (`makakoo query "hello"`) and tells you whether the provider responded. If smoke fails, you stay in this section and try again.

**Use case**: you don't have a local LLM gateway and want to use the Anthropic API directly. Pick option 2, paste your key, done.

### Section 6 — `infect`

Runs `makakoo infect --global` against the CLIs you opted in to in §3.3. Reports what was added / updated / unchanged.

This section is mostly informational at this point — `makakoo install` already ran infect. But if you changed CLI selection in §3.3, this re-applies the change.

Also runs the daemon GYM-hook installer for the AI CLIs that support hooks (Claude/Gemini/OpenCode), so cross-session error funneling works.

**Use case**: confirm everything's wired up before you exit the wizard.

---

## §4 — Per-CLI infect behavior

Each AI CLI has different conventions about *where* to load global instructions and *how strict* its persona override is. Here's the full matrix.

| CLI | Reads from | Persona override | Notes |
|---|---|---|---|
| **Claude Code** | `~/.claude/CLAUDE.md` (auto) | Strong — fully respects custom persona | "Harvey" works end-to-end |
| **Gemini CLI** | `~/.gemini/GEMINI.md` (auto) | Strong — same behavior as Claude | Persona-renames work |
| **Codex (OpenAI)** | `~/.codex/config.toml model_instructions_file` (set by infect) | **Brand-locked** — name stays "Codex" | But it follows ALL bootstrap rules + tools + paths |
| **OpenCode** | `~/.config/opencode/opencode.json instructions[]` (auto) | Strong | Pointer is one JSON string entry |
| **Vibe (Mistral)** | `~/.vibe/instructions.md` (auto) | Strong | |
| **Cursor** | `~/.cursor/rules.md` (auto) | Strong but project-rules override | Rules also load via `.cursor/rules/*.mdc` per-project |
| **Qwen Code** | `~/.qwen/QWEN.md` (auto) | Strong | Same shape as Claude/Gemini |
| **pi (badlogic)** | `~/.pi/AGENTS.md` (auto) | Strong | pi reads AGENTS.md from any anchor dir |

### Codex special case — why "what is your name?" still says "Codex"

OpenAI hard-codes "you are Codex" into the system prompt at a layer above `model_instructions_file`. The infect block IS loaded and IS followed (rules, tools, journal paths, brain conventions, persona traits) — but the literal name "Codex" outranks "Harvey" for the brand-identity question.

To verify Codex IS reading the bootstrap, ask it questions that REQUIRE the bootstrap content:

```
> what's $MAKAKOO_HOME?
> how do I save a memory to my brain?
> what canonical bootstrap file are you reading?
```

If Codex answers correctly with paths from `~/MAKAKOO/bootstrap/global.md` (no shell tricks, no grepping), the bootstrap is loaded.

### What `--target` does

You can scope `infect` to one CLI:

```sh
makakoo infect --global --target codex             # only ~/AGENTS.md + .codex/config.toml
makakoo infect --global --target claude,gemini     # comma-separated for multi
makakoo infect --global --target qwen,vibe,pi      # any subset
```

Tokens accepted: `claude`, `gemini`, `codex`, `opencode`, `vibe`, `cursor`, `qwen`, `pi`.

### What `--local` does (rarely needed)

`infect --local` is for **project-scoped** customization that overrides the global Makakoo config. Writes per-CLI derivative files (`AGENTS.md`, `CLAUDE.md`, etc.) into the project root, NOT the global slot.

```sh
makakoo infect --local                       # all 6 derivatives in current project
makakoo infect --local --target codex        # only AGENTS.md in current project
makakoo infect --local --dir <path>          # pin project root explicitly
```

Use this only when a specific repo needs different rules from your global Makakoo config (e.g. "this project uses Yarn not npm; tell every CLI"). 99% of the time you want `--global`, not `--local`.

---

## §5 — Verification — did it actually work?

After install + setup, run these to confirm everything is wired up:

```sh
makakoo version                  # binary version + persona + $MAKAKOO_HOME
makakoo daemon status            # daemon running?
makakoo plugin list              # installed plugins
makakoo sancho status            # proactive task engine
makakoo memory stats             # recall log + promotion candidates
makakoo infect --verify          # drift check across every infected CLI / IDE slot
```

Each prints a colorized summary. Exit code 0 = healthy.

For a deeper sweep that also walks per-project `.claude.json` MCP entries, workspace `.mcp.json` files, and prunable git worktrees:

```sh
makakoo infect --verify --deep
makakoo infect --verify --deep --repair    # auto-fix any drift found
```

### Live behavior smoketest

Start each AI CLI and ask:

```
What is $MAKAKOO_HOME on this machine?
How do I save a memory to my brain?
What command should I run after significant work?
```

Expected behavior:
- Claude/Gemini/Qwen/Vibe/OpenCode/pi: answers as **your persona name** (e.g. "Harvey"), references `~/MAKAKOO/data/Brain/journals/YYYY_MM_DD.md`, mentions `makakoo sync`.
- Codex: answers as "Codex" but references the same paths and commands (proof bootstrap loaded).
- Cursor: similar to Claude.

If any CLI doesn't know the paths, run `makakoo infect --verify --deep` for that CLI's name.

---

## §6 — Re-running, upgrading, uninstalling

### Re-run setup

The wizard is fully idempotent — running it again preserves your previous answers as defaults. To force a clean re-prompt:

```sh
makakoo setup --reset            # clears stored answers, re-prompts everything
makakoo setup --only model-provider --reset    # reset just one section
```

### Upgrade

```sh
cd makakoo-os
git pull
cargo install --path makakoo --force
cargo install --path makakoo-mcp --force
makakoo install                  # idempotent — picks up new bootstrap version, refreshes pointers
```

After upgrading `makakoo-mcp`, **restart your AI CLIs** so they spawn the new MCP child binary. Pre-existing CLI sessions keep running the old one.

If the bootstrap version bumped (v11 → v12 → v13...), all 8 slot pointers get rewritten on the next `makakoo infect --global` run. Old marker blocks are stripped cleanly.

### Uninstall — symmetric inverse

```sh
makakoo uninfect                 # strip bootstrap from every CLI's global slot
makakoo daemon uninstall         # remove auto-launch agent
rm -rf ~/.makakoo ~/MAKAKOO      # delete config, brain, plugins
cargo uninstall makakoo makakoo-mcp
```

The `makakoo uninfect` command also cleans Codex's `model_instructions_file` key from `~/.codex/config.toml` and removes orphan blocks from any pre-v12 slot locations.

To preview without deleting:

```sh
makakoo uninfect --dry-run       # show what would be stripped
```

---

## §7 — Common situations — decision tree

### "I'm on a fresh Mac and want everything"

```sh
curl -fsSL https://makakoo.com/install | sh    # post-v0.1.0
# OR
git clone https://github.com/makakoo/makakoo-os && cd makakoo-os && \
  cargo install --path makakoo && cargo install --path makakoo-mcp && \
  makakoo install && makakoo setup
```

→ §1.1 / §1.3 → §3 (full wizard)

### "I have Claude Code installed but no other CLIs"

`makakoo install` will detect just Claude and skip the rest. Adding more CLIs later just requires re-running `makakoo infect --global` after the new CLI's `~/.<cli>/` dotdir appears.

### "I already use Obsidian; I don't want a new Brain"

Run `makakoo setup --only brain` and pick `obsidian` + point at your existing vault path. Makakoo will index it and write journals into it directly.

### "I don't have a local LLM; I have an OpenAI key"

Run `makakoo setup --only model-provider` and pick `openai`. Paste the key when prompted. The smoke test (`makakoo query "hello"`) confirms it works before exiting the section.

### "I just want to play with `makakoo search` — minimal install"

```sh
cargo install --path makakoo --path makakoo-mcp
makakoo distro install minimal
makakoo sync           # index existing markdown if any
makakoo search "term"  # works without LLM
```

You don't need the daemon, the wizard, or infect for read-only Brain search.

### "I want to install Makakoo on a remote server (no AI CLI)"

```sh
makakoo distro install minimal
makakoo daemon install
# skip 'makakoo infect --global' since no CLI to infect
```

The MCP server (`makakoo-mcp`) still works for remote MCP clients.

### "My company forbids cloud LLMs — local-only"

`makakoo install` runs entirely offline once `cargo` has the crates cached. For LLM, choose `ollama` in the wizard's model-provider section. switchAILocal also runs locally.

### "I need to undo everything"

→ §6 Uninstall.

### "A CLI's bootstrap looks stale"

```sh
makakoo infect --verify --deep --repair
```

That walks every known slot (global + per-project + worktrees) and rewrites any drift to the current bootstrap version.

---

## §8 — Troubleshooting

### Install errors

| Symptom | Likely cause | Fix |
|---|---|---|
| `cargo: command not found` | No Rust toolchain | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| `error: failed to run custom build command for openssl-sys` | Missing system OpenSSL | macOS: `brew install openssl pkg-config` · Linux: `apt-get install libssl-dev pkg-config` |
| `error: linking with cc failed` (macOS) | Outdated Xcode CLT | `xcode-select --install` |
| `error: linking with link.exe failed` (Win) | Missing MSVC build tools | Install Visual Studio Build Tools 2022 with the C++ workload |
| `Permission denied (os error 13)` writing to `~/.cargo/bin/` | `~/.cargo` owned by another user | `sudo chown -R $(whoami) ~/.cargo` |

### Wizard errors

| Symptom | Likely cause | Fix |
|---|---|---|
| Smoke test in `model-provider` returns `unknown provider for model …` | switchAILocal not running, but configured as default | Start switchAILocal first, OR pick a different provider |
| Smoke test returns `401 Unauthorized` | API key wrong / not stored | `makakoo secret set AIL_API_KEY` and paste again |
| Wizard hangs on `cli-agent` step | One CLI's binary unresponsive on `--version` | `Ctrl-C`, then `makakoo setup --only cli-agent --skip <slow_cli>` |
| `refusing to infect $HOME` from `infect --local` | Stray `~/.harvey/` (legacy) anchoring walk-up | Fixed in v12 — upgrade with `cargo install --path makakoo --force` |

### Post-install errors

| Symptom | Likely cause | Fix |
|---|---|---|
| `makakoo daemon status` says "not running" but you just installed | macOS gatekeeper blocked LaunchAgent | Run `launchctl load ~/Library/LaunchAgents/com.makakoo.daemon.plist`; check `Settings → Privacy → Full Disk Access` |
| `makakoo query` returns "fragment renderer failed" | Plugin manifest parse failure | Run `makakoo plugin list` to see which plugin's manifest is broken; report or remove it |
| AI CLI says "I don't know what Makakoo is" after infect | CLI session was open before infect | Restart the CLI; new session loads the bootstrap |
| Codex still says "Codex" not "Harvey" | OpenAI's brand lock — can't be overridden | Expected. Test with `what's $MAKAKOO_HOME?` instead — that proves bootstrap is loaded |

### "It worked yesterday but doesn't today"

```sh
makakoo infect --verify --deep      # drift report
makakoo daemon logs --tail 200      # recent daemon errors
ls -la ~/MAKAKOO/bootstrap/global.md   # canonical exists?
makakoo memory stats                # any pending writes stuck?
```

For a deeper symptom-rooted decision tree, see [`docs/troubleshooting/tree.md`](troubleshooting/tree.md).

---

## §9 — Reference: every wizard prompt

This is what the user sees, end to end, in the default wizard run. Use it to know what's coming before you start.

### Section 1 — persona

```
[1/6] Persona configuration

Persona name (what should I call myself?) [Harvey]:
Your name (what should I call you?) [Sebastian]:
Tone (sharp / blunt / casual / formal) [sharp]:

✓ Saved to ~/MAKAKOO/config/persona.json
```

### Section 2 — brain

```
[2/6] Brain configuration

Brain location [~/MAKAKOO/data/Brain]:
Format (logseq / obsidian / plain) [logseq]:
Seed with starter pages? (Y/n):

✓ Initialized Brain at ~/MAKAKOO/data/Brain
   - journals/  (chronological notes)
   - pages/     (entity wiki)
```

### Section 3 — cli-agent

```
[3/6] AI CLI hosts to infect

Detected CLIs:
  ✓ claude (~/.claude/)
  ✓ gemini (~/.gemini/)
  ✓ codex  (~/.codex/)
  ✗ opencode (not installed)
  ✗ vibe (not installed)
  ✓ cursor (~/.cursor/)
  ✓ qwen (~/.qwen/)
  ✓ pi (~/.pi/)

Include all detected CLIs? (Y/n):
[If 'n']: Skip which CLIs? (comma-separated tokens):

✓ Will infect 6 CLIs on next run
```

### Section 4 — terminal

```
[4/6] Terminal emulator

Detected: Ghostty (preferred)
Use Ghostty for parallel agent panes? (Y/n):

[If 'n']: Pick from: iTerm2 / Alacritty / Warp / kitty / skip

✓ Saved terminal preference
```

### Section 5 — model-provider

```
[5/6] LLM model provider

Choose a provider:
  1. switchAILocal (recommended — local, free, fast)
  2. Anthropic API (Claude)
  3. OpenAI API (GPT)
  4. Ollama (local)
  5. Custom OpenAI-compatible

Choice [1]:
[If 1]: switchAILocal base URL [http://localhost:18080/v1]:
[If 2]: Anthropic API key (paste; not echoed):
        Default model [claude-sonnet-4-6]:
[If 3]: OpenAI API key (paste; not echoed):
        Default model [gpt-5.5]:

Smoke test: makakoo query "hello"
  ✓ Response: "Hello! How can I help?"

✓ Provider configured
```

### Section 6 — infect

```
[6/6] Infect AI CLI hosts

Running: makakoo infect --global
  ✓ claude     installed   ~/.claude/CLAUDE.md
  ✓ gemini     installed   ~/.gemini/GEMINI.md
  ✓ codex      installed   ~/AGENTS.md (+ ~/.codex/config.toml)
  ✓ cursor     installed   ~/.cursor/rules.md
  ✓ qwen       installed   ~/.qwen/QWEN.md
  ✓ pi         installed   ~/.pi/AGENTS.md

Running: makakoo daemon gym-hook install
  ✓ claude/gemini/opencode hooks installed

Done. Restart any open AI CLI sessions for the bootstrap to take effect.

Setup complete. Run `makakoo --help` to see the full command surface.
```

---

## See also

- [`docs/getting-started.md`](getting-started.md) — beginner's narrative install guide
- [`docs/walkthroughs/01-fresh-install-mac.md`](walkthroughs/01-fresh-install-mac.md) — copy-paste runnable Mac install
- [`docs/user-manual/setup-wizard.md`](user-manual/setup-wizard.md) — terse reference for each wizard section
- [`docs/user-manual/makakoo-infect.md`](user-manual/makakoo-infect.md) — the `infect` subcommand reference
- [`docs/troubleshooting/tree.md`](troubleshooting/tree.md) — symptom-rooted decision tree
- [`spec/CAPABILITIES.md`](../spec/CAPABILITIES.md) — what plugins can/can't do, the capability sandbox model
