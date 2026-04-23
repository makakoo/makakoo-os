# Use cases — "I want to do X with Makakoo"

Recipes for the most common things people do with Makakoo in their
first weeks. Every recipe has: the goal, the exact commands, and the
expected result.

New here? Start with [Getting started](getting-started.md) first.

---

## 1. Keep a daily journal that every AI CLI can read

**Goal:** write a note once; have every AI CLI on your machine see it.

```sh
makakoo journal add "Met with Ana from Acme. They want the Q3 demo before July 15. Follow up next Monday."
```

That line goes into today's journal at
`~/MAKAKOO/data/Brain/journals/2026_04_23.md` (replace with the
current date). Next time you open any AI CLI and ask *"what's
happening with Acme?"*, it'll find the note.

Add structure with [[double brackets]] to link entities:

```sh
makakoo journal add "Met with [[Ana]] from [[Acme]]. Demo target: 2026-07-15."
```

Now the `Ana` and `Acme` pages at `~/MAKAKOO/data/Brain/pages/` cross-
reference each other.

---

## 2. Ask your Brain a question

**Goal:** find what you (or any AI) wrote in the Brain about a topic.

```sh
makakoo query "what did I decide about the Acme Q3 demo?"
```

Under the hood: full-text search + vector similarity + LLM synthesis.
Returns a short answer + the journal entries it relied on.

If you just want raw search hits without LLM synthesis:

```sh
makakoo search "acme demo"
```

---

## 3. Connect my Obsidian vault (or a folder of notes)

**Goal:** make an existing Obsidian vault or a plain markdown folder a
first-class Makakoo brain substrate.

```sh
makakoo setup brain
```

The interactive picker auto-detects common Obsidian paths
(`~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`).
Say yes to the detected path, optionally change the write-default, and
you're done.

**Also available:**

```sh
makakoo brain list                                  # show registered sources
makakoo brain add personal obsidian ~/MyVault       # add a vault explicitly
makakoo brain set-default personal                  # switch where Makakoo writes
makakoo brain sync --name personal                  # scan + ingest into search
```

Two scenarios to disambiguate before registering:

- **A — you already have a separate Obsidian vault.** Use the commands
  above. This registers it as an additional source Makakoo reads from.
- **B — you want Obsidian just as a nicer editor.** Don't register
  anything. Open Obsidian → `Open folder as vault` → point at
  `~/MAKAKOO/data/Brain/`. Same files, Obsidian UX. Zero config.

---

## 4. Install the blessed CLI agent (pi) and terminal (Ghostty)

**Goal:** let Makakoo install the two first-class tools it ships with.

```sh
makakoo setup cli-agent    # pi (npm package @mariozechner/pi-coding-agent)
makakoo setup terminal     # Ghostty, macOS only, via Homebrew cask
```

Each prompts with `[Y/n/s]`. Y installs, N re-asks next setup run, S
marks skipped so it never re-asks unless you `--reset`.

If `npm` or `brew` isn't installed, the wizard tells you what to
install first — it never runs a package manager you don't have.

---

## 5. Pick which LLM Makakoo uses

**Goal:** decide whether Makakoo's internal calls go through
switchAILocal, Anthropic, OpenRouter, or another adapter.

```sh
makakoo setup model-provider
```

You'll see the adapters currently registered. Type the number of the
one you want as primary. The wizard writes
`~/.makakoo/primary_adapter.toml` naming your choice.

To see what's registered without picking:

```sh
makakoo adapter list
```

API keys for each adapter are set via its own doctor/install flow, not
here — this section only names the primary.

---

## 6. Make every AI CLI share the same Brain

**Goal:** "infect" every AI CLI on your machine with the Makakoo
bootstrap block. Already done by `makakoo install`; re-run any time.

```sh
makakoo infect --verify        # report drift
makakoo infect                 # write bootstrap block to every host
makakoo infect --target claude,gemini    # just these two
makakoo infect --verify --json # machine-readable drift report (for watchdogs)
```

The bootstrap block lands in each CLI's config file:
`~/.claude/CLAUDE.md`, `~/.gemini/GEMINI.md`, `~/.codex/AGENTS.md`,
`~/.config/opencode/AGENTS.md`, `~/.vibe/AGENTS.md`, `~/.cursor/rules/`,
`~/.qwen/QWEN.md`, `~/.pi/AGENTS.md`. Makakoo never touches your shell
dotfiles (`.zshrc`, `.bashrc`).

---

## 7. Give an agent write access to a folder outside Makakoo

**Goal:** let an infected AI CLI edit files in a project folder that
isn't in Makakoo's default write-sandbox.

**In conversation** (easiest): if an agent asks *"Want me to grant
myself 1h write access to ~/code?"*, just say yes. The agent writes
the grant via `grant_write_access`.

**From terminal:**

```sh
makakoo perms grant ~/code/scratch/ --for 1h
makakoo perms list                   # see active grants
makakoo perms revoke <grant_id>       # pull access back
```

Full reference: [user-manual/makakoo-perms.md](user-manual/makakoo-perms.md).

---

## 8. See what Makakoo is doing in the background

**Goal:** check the SANCHO task engine (proactive 24h / 30m / weekly
workers) and the daemon health.

```sh
makakoo sancho status      # registered tasks + last-run state
makakoo sancho tick        # force one tick now
makakoo health --verbose   # daemon + plugin health report
makakoo status             # short summary
```

Log locations (if you need to grep):

- `~/.makakoo/logs/makakoo.log` — the Makakoo daemon
- `~/MAKAKOO/data/Brain/journals/` — SANCHO writes task summaries to
  today's journal

---

## 9. Install a new plugin

```sh
makakoo plugin list                                    # what's installed
makakoo plugin install git+https://github.com/you/my-plugin@v1
makakoo plugin install /Users/you/local/plugin-dir
makakoo plugin remove my-plugin
```

Plugins live at `~/MAKAKOO/plugins/<name>/`, declare their own config
with `plugin.toml`, and run sandboxed under the capability model. See
[Plugins](plugins/index.md) for details.

---

## 10. Re-run the setup wizard for one thing

**Goal:** change your mind about one section without running the
whole wizard.

```sh
makakoo setup persona          # rename the assistant
makakoo setup brain            # add/remove a vault
makakoo setup cli-agent        # install pi if you skipped it
makakoo setup terminal         # macOS only
makakoo setup model-provider   # switch LLM routing
makakoo setup infect           # re-infect drifted CLI hosts
```

Or see the whole current state without any prompts:

```sh
makakoo setup --non-interactive
```

Full reference: [Setup wizard](user-manual/setup-wizard.md).

---

## 11. Uninstall

**Clean removal:**

```sh
makakoo uninfect                # strip bootstrap from every CLI config
makakoo daemon uninstall        # remove the LaunchAgent / systemd unit
rm -rf ~/.makakoo ~/MAKAKOO     # delete data (⚠️ this removes your Brain)
```

Full guide: [Uninstall](troubleshooting/uninstall.md).

---

## 12. Get help from any AI CLI about Makakoo itself

Once infected, every AI CLI knows how Makakoo works. You can just
ask it in plain language:

```
> What skills do you have?
> Can you read my brain?
> How do I add an Obsidian vault?
> Why is Makakoo saying "not on PATH"?
```

The bootstrap block teaches each CLI about `skill_discover`, the Brain
paths, the adapter registry, and the safety rules — so it answers
correctly without guessing.
