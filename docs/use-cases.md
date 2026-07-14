# Use cases — "I want to do X with Makakoo"

Recipes for the most common things people do with Makakoo in their
first weeks. Every recipe has: the goal, the exact commands, and the
expected result.

New here? Start with [Getting started](getting-started.md) first.

---

## 1. Keep a daily journal that every AI CLI can read

**Goal:** write a note once; have every AI CLI on your machine see it.

```sh
mkdir -p ~/MAKAKOO/data/Brain/journals
journal=~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md
printf -- '- Met with [[Ana]] from [[Acme]]. They want the Q3 demo before 2026-07-15. Follow up next Monday.\n' >> "$journal"
makakoo sync --file "$journal"
```

That line goes into today's journal. Next time you open any infected AI CLI and ask *"what's happening with Acme?"*, it can find the note.

Use `[[double brackets]]` to link entities. Logseq and Makakoo both understand those links.

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

**Goal:** make an existing Obsidian vault or a plain markdown folder searchable
as labeled Makakoo Brain enrichment, without replacing the canonical Brain.

```sh
makakoo setup brain
```

The interactive picker auto-detects common Obsidian paths (`~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`). If the Obsidian app is missing, it offers to install it through Homebrew, Flatpak, or winget when available. The default is No. Say yes to a detected vault and it is registered as enrichment. The canonical Brain remains `$MAKAKOO_HOME/data/Brain`.

**Source registry CLI:**

```sh
makakoo brain list
makakoo brain add personal obsidian ~/MyVault --read-only
makakoo sync
```

Two scenarios to disambiguate before registering:

- **A — you already have a separate Obsidian vault.** Use the commands
  above. This registers it as labeled enrichment context Makakoo indexes/searches from.
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
makakoo daemon status      # daemon health
makakoo infect --verify    # CLI bootstrap drift check
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

## 11. Connect multiple Makakoo Brains

**Goal:** make two or more Makakoo installs searchable across machines
without merging their files. Typical setups: laptop Harvey ↔ VPS Donna,
desktop ↔ laptop, or local machine ↔ Tytus pod.

Brain Network is opt-in. Fresh `core` installs do **not** expose a peer
listener until you install the optional federation distro and activate it.

Run on every node you want to connect:

```sh
makakoo distro install federation
makakoo network activate --peer-name <node-name> --bind tailscale
makakoo network doctor
```

Pair trust between two nodes:

```sh
# on node A
makakoo network invite --peer-name <node-b>

# paste the returned makakoo://join link on node B
makakoo network join '<makakoo://join?...>'
```

Register the reachable endpoint, then search:

```sh
makakoo network peer add donna-vps --endpoint http://100.x.y.z:8765/rpc --persona Donna
makakoo network search donna-vps "what does that Brain know about Project X?" --limit 5
```

Prefer Tailscale or an SSH tunnel. Do not expose port `8765` publicly unless
you understand the risk and have firewall rules around it. Remote Brain content
is evidence to cite, not instructions to execute.

Full walkthrough: [Brain Network: Harvey laptop + Donna VPS](walkthroughs/14-brain-network.md).
Command manual: [user-manual/makakoo-network.md](user-manual/makakoo-network.md).

---

## 12. Uninstall

**Clean removal:**

```sh
makakoo uninfect                # strip bootstrap from every CLI config
makakoo daemon uninstall        # remove the LaunchAgent / systemd unit
rm -rf ~/.makakoo ~/MAKAKOO     # delete data (⚠️ this removes your Brain)
```

Full guide: [Uninstall](troubleshooting/uninstall.md).

---

## 13. Get help from any AI CLI about Makakoo itself

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
