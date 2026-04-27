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
- **No other setup.** You do not need Homebrew, Rust, Python, or admin privileges for Makakoo itself. You do not need an AI API key. You do not need a GitHub account.

> **Apple Silicon vs Intel:** the installer detects your CPU automatically. You do not need to pick.

## Steps

### 1. Open Terminal

Press `⌘` + `Space` to open Spotlight, type `Terminal`, press Enter. A window with a blinking cursor opens. Leave it open for the rest of this walkthrough.

### 2. Run the installer

Copy the next line exactly, paste it into Terminal, press Enter.

```sh
curl -fsSL https://makakoo.com/install | sh
```

You should see something like:

```text
Makakoo OS installer
detected: darwin-arm64 (or darwin-x86_64 on Intel)
downloading makakoo@latest...
✓ installed  /Users/you/.local/bin/makakoo
next: run `makakoo install` to set up the kernel
```

> **If `makakoo.com/install` returns 404** — the public v0.1.0 release has not landed yet. Skip to **Alternative install: from source** at the bottom of this page. That path works today from a cloned repository.

### 3. Reload your shell so the new command is found

```sh
exec $SHELL
```

No output — that's correct.

### 4. Verify the binary is on `$PATH`

```sh
makakoo --version
```

Expected output:

```text
makakoo 0.1.0
```

If you get `command not found`, skip to **If something went wrong** → *PATH issue*.

### 5. See who lives here

```sh
makakoo version
```

Expected output (the `home` path will reflect your username):

```text
makakoo 0.1.0 (unknown)
persona: Harvey (pronoun=he/him, voice_default=caveman)
home: /Users/you/MAKAKOO
```

The `home` directory (`~/MAKAKOO`) does not exist yet — `makakoo install` in the next step creates it.

### 6. Run the one-shot install

This step installs the **core distro** (a small curated set of plugins), registers the **background daemon** (the task engine that runs while you work), and **infects** every AI CLI you already have installed with the shared Makakoo bootstrap.

```sh
makakoo install
```

Answer `y` at each interactive prompt (the defaults are fine). On a fresh machine you'll see roughly:

```text
distro install  → core (plugin count: 38)
daemon install  → ~/Library/LaunchAgents/com.makakoo.daemon.plist
infect          → claude, gemini, codex, opencode, vibe, cursor, qwen, pi (detected)
✓ install complete — run `makakoo setup` for the interactive wizard
```

> **Don't have any AI CLI installed yet?** The `infect` step will say `no CLI hosts detected`. That's fine — you can install Claude Code, Gemini CLI, or any of the others later and rerun `makakoo infect` when you do.

### 7. Confirm the task engine is alive

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

### 8. Confirm the memory layer responds

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

### 9. (Optional) Run the interactive setup wizard

```sh
makakoo setup
```

The wizard walks through naming the assistant, registering brain sources, setting up the blessed terminal (Ghostty on macOS), picking a primary LLM provider, and re-infecting CLI hosts. Every section is idempotent — you can run the wizard as many times as you like, or skip it entirely for now.

## What just happened?

- `curl | sh` downloaded the Makakoo binary for your Mac's CPU and placed it in `~/.local/bin/makakoo`. Nothing else on your system changed.
- `makakoo install` created the Makakoo filesystem at `~/MAKAKOO`, registered a **LaunchAgent** (a small background service that starts automatically on login — you can remove it any time with `makakoo daemon uninstall`), and added the **Makakoo bootstrap block** to any AI CLI it detected. From now on, every AI CLI on this Mac shares the same Brain.
- `makakoo sancho status` confirmed the proactive task engine is registered and ready to run scheduled tasks in the background.
- `makakoo memory stats` confirmed the SQLite memory pipeline is alive and empty, as expected.

You now have a working Makakoo install. The next walkthrough, [02 — First skill](./02-first-skill.md), shows how to run your first command through the skill dispatcher.

## If something went wrong

| Symptom | Fix |
|---|---|
| `makakoo: command not found` | PATH issue. Run `export PATH=$HOME/.local/bin:$PATH` then retry. Make it permanent by adding that line to `~/.zshrc`. |
| `curl: (7) Failed to connect to makakoo.com` | No internet, or DNS / firewall blocking. Test with `curl https://github.com`. If that also fails, fix your network first. |
| `curl: 404` on `makakoo.com/install` | The public v0.1.0 release has not landed yet. Use the **Alternative install: from source** section below. |
| `makakoo install` prompts for a password | Makakoo itself does not need `sudo`. If you see a password prompt, cancel it (`Ctrl+C`), then inspect the prompt context — most likely Homebrew (if installed as a dependency for another plugin) is asking. |
| Anything else | See the full troubleshooting guide at [`docs/troubleshooting/index.md`](../troubleshooting/index.md). If the symptom-rooted tree (Phase 3 of the docs sprint) is live, start there. |

## Alternative install: from source

Use this if `curl https://makakoo.com/install` 404s (pre-v0.1.0 release) or if you prefer building from source.

Prerequisites for this path **only**: `git`, `rust` (install with one command: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).

```sh
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo install --path makakoo
cargo install --path makakoo-mcp
makakoo install
```

After `cargo install` finishes, `~/.cargo/bin/makakoo` is on your `$PATH` if you accepted rustup's defaults. The subsequent steps (4–9) above are identical.
