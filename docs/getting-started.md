# Getting Started with Makakoo OS

A step-by-step install guide aimed at first-time users. If you have never
used a terminal before, you can still follow along — every command is
explained, every expected output is shown, and the most common errors
have "what to do if it happens" right next to them.

Read time: 10 minutes. Install time: 5 minutes.

---

## Before you start — what you need

| Thing | Why | How to check you have it |
|---|---|---|
| A Mac, Linux PC, or Windows PC | Makakoo runs on all three | Any modern laptop or desktop will do |
| An internet connection | To download Makakoo | You're reading this page — good, you have it |
| A "terminal" app | Where you type commands | On Mac: Spotlight → type `Terminal` → Enter. On Windows: Start → type `PowerShell` → Enter. On Linux: you already know |
| At least one AI CLI (optional but recommended) | Makakoo's main value is cross-CLI memory; with zero CLIs installed, it still works but has less to do | Run `which claude` or `which gemini` in the terminal — if it prints a path, you have it |

**You do NOT need:**
- A GitHub account (Makakoo is private but you just download the binary, no login).
- An API key (you can add one later if you want your own LLM; Makakoo works without).
- Admin / root / sudo (except to install Homebrew on macOS if you don't have it).

---

## Step 1 — Open a terminal

<details>
<summary><strong>🍎 macOS</strong></summary>

1. Press `⌘` (Command) + `Space` to open Spotlight.
2. Type `Terminal`.
3. Press Enter. A black or white window opens with a blinking cursor.

That's it. Don't close the window until you're done.

</details>

<details>
<summary><strong>🐧 Linux</strong></summary>

1. Press `Ctrl` + `Alt` + `T` (works on most distros).
2. Or open your application menu and search for `Terminal` / `Konsole` / `gnome-terminal`.

</details>

<details>
<summary><strong>🪟 Windows</strong></summary>

1. Press the Windows key.
2. Type `PowerShell`.
3. Click `Windows PowerShell` (NOT "PowerShell ISE").
4. A blue window opens.

**Note:** Makakoo on Windows needs **Developer Mode** on. To enable: Settings
→ Privacy & security → For developers → Developer Mode → toggle ON. This
lets Makakoo create symbolic links which it needs for plugins.

</details>

---

## Step 2 — Install Makakoo

Copy the right line for your OS, paste it into the terminal, press Enter.

<details>
<summary><strong>🍎 macOS / 🐧 Linux</strong></summary>

```sh
curl -fsSL https://makakoo.com/install.sh | sh
```

What happens: the script detects your OS + CPU, downloads the matching
pre-built `makakoo` and `makakoo-mcp` binaries from the latest GitHub
Release, drops them in `~/.local/bin/` (with bundled runtime data under
`~/.local/share/makakoo/`), and then **automatically execs `makakoo
install`**, which itself hands off to the interactive setup wizard.
You'll walk straight from one curl-pipe into a working install + the
wizard.

</details>

<details>
<summary><strong>🍺 macOS / 🐧 Linux — Homebrew alternative</strong></summary>

```sh
brew install traylinx/tap/makakoo
makakoo install
```

`brew install` only places the binaries; `makakoo install` then runs
the orchestrator (distro + daemon + infect + wizard hand-off). Future
upgrades: `brew upgrade traylinx/tap/makakoo`.

</details>

<details>
<summary><strong>🪟 Windows (PowerShell)</strong></summary>

```powershell
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
```

Same auto-handoff as macOS / Linux — drops the binaries under
`%LOCALAPPDATA%\Makakoo\bin\` and execs `makakoo install` at the
end.

</details>

**Expected output (snipped — the install + wizard run inline):**

```
→ installing makakoo (latest) for aarch64-apple-darwin into /Users/you/.local/bin
→ downloading https://github.com/makakoo/makakoo-os/releases/latest/download/makakoo-aarch64-apple-darwin.tar.gz
✓ installed makakoo to /Users/you/.local/bin
✓ bundled distros + plugins-core to /Users/you/.local/share/makakoo
→ launching: /Users/you/.local/bin/makakoo install
…
```

(The full install + wizard output is shown in Step 3 below.)

> **Want to skip the auto-handoff?** Set `MAKAKOO_NO_AUTORUN=1` (sh)
> or `$env:MAKAKOO_NO_AUTORUN = "1"` (ps1) before running. Used by
> CI / scripted provisioning — the script will install the binaries
> and exit, leaving `makakoo install` for you to run manually.

### "Which-makakoo not found" after installing?

The installer puts `makakoo` in `~/.local/bin/`. If your shell can't find
it after install, that directory isn't on your `PATH`. Fix:

```sh
# Mac / Linux (zsh — the default on macOS 10.15+)
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc

# Mac / Linux (bash)
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

Close and re-open the terminal, then try `makakoo --version` again.

---

## Step 3 — Walk through the install + wizard

Step 2 already invoked `makakoo install` for you (the curl-pipe and
the iwr-pipe both auto-handoff into it). This step shows what you'll
see scrolling past — and the questions the wizard asks.

If you opted out with `MAKAKOO_NO_AUTORUN=1`, run it yourself:

```sh
makakoo install
```

> 🍎 **macOS users — heads up.** The first time the install runs, macOS
> may show a security prompt about a "background item" trying to load.
> **Click Allow** in the dialog (or open **System Settings → General →
> Login Items → Allow in the Background** and toggle Makakoo on). If
> you miss the prompt, the install finishes but the daemon won't start
> — you'll see "daemon install failed" and need to re-run
> `makakoo daemon install` after enabling the background item. This
> is a one-time approval; future installs are silent.

**What this does:** installs the `core` set of plugins (the default
bundle), starts the background daemon that keeps Makakoo alive, and
"infects" every AI CLI you have installed so they all share the same
brain. Takes about 30 seconds.

**Expected output:**

```
install plan
  distro:   core
  daemon:   install
  infect:   global CLI slots

  detected hosts:
    - claude:  binary=/opt/homebrew/bin/claude
               config=~/.claude/CLAUDE.md (will create)
    - gemini:  binary=/opt/homebrew/bin/gemini
               config=~/.gemini/GEMINI.md (will create)
    …

[1/3] installing distro core…
✓ 38 plugins installed
[2/3] installing daemon…
✓ LaunchAgent registered
[3/3] infecting 4 detected CLI host(s)…
✓ 4 slots updated

install complete
  detected hosts: 4
    claude, gemini, codex, opencode

→ launching the setup wizard
[1/6] Persona configuration
Persona name (what should I call myself?) [Harvey]:
…
```

The wizard hand-off is automatic in interactive shells. It walks you
through six sections — persona, brain, CLI hosts, terminal, LLM
provider, and a final infect pass. Press Enter to accept defaults at
any prompt; the wizard is fully idempotent and you can re-run any
section later with `makakoo setup --only <section>`. See
[Setup wizard](user-manual/setup-wizard.md) for what each section
does.

### "Permission denied" or "command not found: makakoo"

- `command not found: makakoo` → see Step 2's PATH fix above.
- `Permission denied (publickey)` when the wizard tries to clone
  something → you can skip that step with `n` and set up the
  optional piece later.
- "makakoo install failed: daemon install failed" → re-run
  `makakoo daemon install` alone; the distro is already there and
  this is usually a macOS Gatekeeper prompt you missed.

---

## Step 4 — Verify it works

Run these three commands. All three should succeed:

```sh
makakoo --version
makakoo status
makakoo query "hello"
```

**Expected** (your version may be newer):

```
makakoo 0.1.5
```

```
✓ daemon running (pid 51234)
✓ 4 CLI hosts infected (claude, gemini, codex, opencode)
✓ 38 plugins installed
✓ Brain at ~/MAKAKOO/data/Brain (0 journals, 0 pages — fresh install)
```

```
I don't have any records yet — your Brain is empty. Try writing a
journal entry with `makakoo journal add "<text>"`.
```

If any command errors, jump to [Troubleshooting](troubleshooting/index.md).

---

## Step 5 — Open any AI CLI and say hi

```sh
claude
# or
gemini
# or whichever you have
```

You'll notice the first message of every session now starts with a
short "Makakoo bootstrap" block explaining which tools the AI has and
where your Brain lives. That block is what makes cross-CLI memory work
— every AI CLI now reads from and writes to the same `~/MAKAKOO/data/Brain/`.

Try asking: *"What do you know about me?"*

On a fresh install the answer will be brief (no journals yet). After
a few days of use the assistant will surface things you told it days
ago, in different tools.

---

## You're done

Where to go next depends on what you want to do:

| I want to... | Read |
|---|---|
| See what Makakoo can do day-to-day | [Use cases](use-cases.md) |
| Connect my Obsidian vault or Logseq graph | [Brain sources](user-manual/brain-sources.md) |
| Understand the setup wizard's 6 sections | [Setup wizard](user-manual/setup-wizard.md) |
| Add pi (blessed CLI coding agent) or Ghostty (blessed terminal) | `makakoo setup cli-agent` / `makakoo setup terminal` |
| Pick which LLM Makakoo routes through | [Model providers](user-manual/adapters.md) |
| Install a new plugin | [Plugins](plugins/index.md) |
| Fix something that broke | [Troubleshooting](troubleshooting/index.md) |
| Uninstall Makakoo cleanly | [Uninstall](troubleshooting/uninstall.md) |

---

## If you get stuck

- **Search the Troubleshooting page** first: [docs/troubleshooting/](troubleshooting/index.md).
  Common errors have explicit fixes.
- **Run `makakoo status`** — shows which subsystems are healthy.
- **Run `makakoo health --verbose`** — deeper self-check with links
  to the failing subsystem's docs.
- **Ask one of your infected AI CLIs directly.** Once infected, every
  AI CLI knows how Makakoo works and can walk you through issues in
  plain language.

Help is also in `makakoo --help` and `makakoo <command> --help` for
any subcommand.
