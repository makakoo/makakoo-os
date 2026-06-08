# Walkthrough 01 — Fresh install on a new Mac

## What you'll do

Install Makakoo OS on a Mac, verify the binary is on your `$PATH`, and run three small health checks to confirm the install is healthy.

**Time:** about 5 minutes. **Prior experience needed:** none.

## Prerequisites

- A Mac running **macOS 12 Monterey or later**. Check yours: press `⌘` + `Space`, type `Terminal`, press Enter, then run:
  ```sh
  sw_vers -productVersion
  ```
  Any number that starts with `12.`, `13.`, `14.`, `15.`, or higher is fine.
- **An internet connection.** The installer downloads a small binary (~20 MB) from GitHub.
- **Python 3.12 + `uv`.** Required for running Python plugins and performing security audits via the SkillSpector gate. You can install them easily using Homebrew: `brew install python@3.12 uv`
- **No other setup.** You do not need Rust, admin privileges, an AI API key, or a GitHub account to get started.

> **Apple Silicon vs Intel:** the installer detects your CPU automatically. You do not need to pick.

## Steps

### 1. Open Terminal

Press `⌘` + `Space` to open Spotlight, type `Terminal`, press Enter. A window with a blinking cursor opens. Leave it open for the rest of this walkthrough.

### 2. Run the installer

Copy the next line exactly, paste it into Terminal, press Enter.

```sh
curl -fsSL https://makakoo.com/install.sh | sh
```

The installer detects your CPU, downloads pre-built binaries from the latest GitHub Release, drops them under `~/.local/bin/`, and then **automatically execs `makakoo install`** (the one-shot orchestrator: distro + background daemon + AI-CLI infect + health check), which itself hands off to the interactive setup wizard.

You'll see roughly:

```text
→ installing makakoo (latest) for aarch64-apple-darwin into /Users/you/.local/bin
→ downloading https://github.com/makakoo/makakoo-os/releases/latest/download/makakoo-aarch64-apple-darwin.tar.gz
✓ installed makakoo to /Users/you/.local/bin
✓ bundled distros + plugins-core to /Users/you/.local/share/makakoo
→ launching: /Users/you/.local/bin/makakoo install
  Sets up the core distro, the background daemon, infects every
  AI CLI we detect, then hands off to the setup wizard.

distro install  → core
daemon install  → ~/Library/LaunchAgents/com.makakoo.daemon.plist
infect          → claude, gemini, codex, opencode, vibe, cursor, qwen, pi (detected)
✓ install complete

[1/6] Persona configuration
Persona name (what should I call myself?) [Harvey]:
…
```

Walk through each wizard prompt — defaults are sensible on a fresh install. The wizard is fully idempotent; you can re-enter any section later with `makakoo setup --only <section>`.

> **Don't have any AI CLI installed yet?** The `infect` step will say `no CLI hosts detected`. That's fine — install Claude Code, Gemini CLI, or any of the others later and rerun `makakoo infect` when you do.

> **Want to skip the auto-handoff** (CI / unattended install)? Set `MAKAKOO_NO_AUTORUN=1` before piping into `sh`. The script will install the binaries and exit, leaving `makakoo install` for you to run manually.

### 3. Reload your shell so the new command is found

The wizard opened the binary with an absolute path; the rest of this walkthrough invokes `makakoo` by name, so make sure `~/.local/bin` is on your `$PATH`:

```sh
exec $SHELL
```

No output — that's correct. If your shell didn't have `~/.local/bin` on `$PATH` already, the installer's tail printed the line to add to your shell rc.

### 4. Verify the binary is on `$PATH`

```sh
makakoo --version
```

Expected output (your version may be newer):

```text
makakoo 0.1.6
```

If you get `command not found`, skip to **If something went wrong** → *PATH issue*.

### 5. See who lives here

```sh
makakoo version
```

Expected output (the `home` path will reflect your username; the version will match step 4):

```text
makakoo 0.1.6 (release)
persona: Harvey (pronoun=he/him, voice_default=caveman)
home: /Users/you/MAKAKOO
```

`~/MAKAKOO` was created by the install step in §2.

### 6. Confirm the task engine is alive

```sh
makakoo sancho status
```

Expected output (the exact task list depends on which distro you installed):

```text
SANCHO — 8 tasks registered
- journal_compactor        last_run=-  interval=3600s
- brain_resurface          last_run=-  interval=1800s
- memory_promoter          last_run=-  interval=1800s
...
```

`last_run=-` means SANCHO hasn't fired yet (normal — you just installed it).

### 7. Confirm the memory layer responds

```sh
makakoo memory stats
```

Expected output on a brand-new install:

```text
recall_log:          0 entries
recall_stats:        0 entries (across 0 distinct tools)
memory_promotions:   0 candidates
last promoter run:   never
```

Zeros are healthy here — you haven't used Makakoo yet, so nothing has been recorded.

### 8. Re-run the wizard later (optional)

The wizard ran automatically in step 2 as part of `curl | sh`. If you want to revisit any section — e.g. switch to a different LLM provider or point Makakoo at an Obsidian vault — invoke it directly:

```sh
makakoo setup                          # walk all six sections
makakoo setup --only model-provider    # just one section
makakoo setup --reset                  # clear stored answers and re-prompt
```

Every section is idempotent; previous answers become defaults.

## What just happened?

- `curl | sh` downloaded pre-built `makakoo` and `makakoo-mcp` binaries for your Mac's CPU and placed them in `~/.local/bin/`, with bundled `distros/` + `plugins-core/` under `~/.local/share/makakoo/`. Nothing else on your system changed.
- The installer auto-execed `makakoo install`, which created the Makakoo filesystem at `~/MAKAKOO`, registered a **LaunchAgent** (a small background service that starts automatically on login — you can remove it any time with `makakoo daemon uninstall`), and added the **Makakoo bootstrap block** to any AI CLI it detected. From now on, every AI CLI on this Mac shares the same Brain.
- `makakoo install` then handed off to the interactive `makakoo setup` wizard, where you named your persona and picked an LLM provider.
- `makakoo sancho status` confirmed the proactive task engine is registered and ready to run scheduled tasks in the background.
- `makakoo memory stats` confirmed the SQLite memory pipeline is alive and empty, as expected.

You now have a working Makakoo install. The next walkthrough, [02 — First skill](./02-first-skill.md), shows how to run your first command through the skill dispatcher.

## If something went wrong

| Symptom | Fix |
|---|---|
| `makakoo: command not found` | PATH issue. Run `export PATH=$HOME/.local/bin:$PATH` then retry. Make it permanent by adding that line to `~/.zshrc`. |
| `curl: (7) Failed to connect to makakoo.com` | No internet, or DNS / firewall blocking. Test with `curl https://github.com`. If that also fails, fix your network first. |
| `curl: 404` on `makakoo.com/install.sh` | Either makakoo.com is down or you typed the URL wrong. Re-check the URL — the script is `install.sh`, not `install`. If makakoo.com is down, fall back to the source install at the bottom of this page. |
| `makakoo install` prompts for a password | Makakoo itself does not need `sudo`. If you see a password prompt, cancel it (`Ctrl+C`), then inspect the prompt context — most likely Homebrew (if installed as a dependency for another plugin) is asking. |
| Anything else | See the full troubleshooting guide at [`docs/troubleshooting/index.md`](../troubleshooting/index.md). If the symptom-rooted tree (Phase 3 of the docs sprint) is live, start there. |

## Alternative install: Homebrew

If you already use Homebrew, this is a one-liner alternative to `curl | sh`:

```sh
brew install traylinx/tap/makakoo
makakoo install         # runs distro + daemon + infect, then hands off to the wizard
```

`brew install` only places the binaries; the Homebrew formula deliberately doesn't auto-launch the wizard (Homebrew installs are typically scripted). So follow up with `makakoo install` to run the orchestrator. Future upgrades: `brew upgrade traylinx/tap/makakoo`.

## Alternative install: from source

Useful if you want to inspect the code before running it, contribute changes, or build for a target the prebuilt installer doesn't cover.

Prerequisites for this path **only**: `git`, `rust` (install with one command: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).

```sh
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo install --path makakoo
cargo install --path makakoo-mcp
makakoo install         # idempotent — runs distro + daemon + infect, hands off to the wizard
```

After `cargo install` finishes, `~/.cargo/bin/makakoo` is on your `$PATH` if you accepted rustup's defaults. The subsequent steps (4–8) above are identical.
