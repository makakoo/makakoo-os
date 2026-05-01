# Frequently asked questions

Short answers to the questions people ask before they install Makakoo —
and a few that come up in the first week after.

Need a full walk-through? See [Getting started](getting-started.md) or
the topical [Use cases](use-cases.md).

---

### Is Makakoo free?

Yes. MIT-licensed. The binary is free to install and use. If you route
Makakoo through a paid LLM (Anthropic, OpenAI, etc.) the token bill is
yours, but Makakoo itself costs nothing and can run with zero API keys
if you point it at a local model.

### Does Makakoo send my data anywhere?

No. All Brain journals + pages live on disk at
`~/MAKAKOO/data/Brain/`. Makakoo doesn't phone home, doesn't upload
usage metrics, doesn't call out to any server except the LLM gateway
*you* configure (default is localhost via switchAILocal).

If you connect an adapter like Anthropic direct, your prompts go to
that provider — same as if you used their CLI directly. The adapter
surface is opt-in per-call.

### Do I need an AI CLI installed before installing Makakoo?

No. Makakoo installs and runs even with zero AI CLIs on your machine.
The infect step just finds nothing to infect and moves on. Install an
AI CLI any time later, then run `makakoo infect` to light it up.

### Which AI CLIs does Makakoo know about?

Nine first-class hosts at v0.1.3, and the roster grows as new CLIs land:

- Claude Code (`claude`)
- Gemini CLI (`gemini`)
- Codex (`codex`)
- OpenCode (`opencode`)
- Mistral Vibe (`vibe`)
- Cursor (`cursor`)
- Qwen Code (`qwen`)
- Kimi (`@moonshotai/kimi-cli`) — added v0.1.3
- pi (the blessed CLI coding agent Makakoo ships with)

IDE assistants (GitHub Copilot, Continue, Cline, JetBrains AI) are detected
but not infected — they handle their own persona.

### Does installing Makakoo change any files outside its own directory?

Only three places:

1. **`~/.local/bin/makakoo`** (or your chosen install dir) — the binary.
2. **Each infected CLI's config file** — adds a fenced bootstrap block.
   Example: `~/.claude/CLAUDE.md`, `~/.gemini/GEMINI.md`. The block has
   explicit `START` / `END` markers so it can be cleanly removed.
3. **A LaunchAgent / systemd unit / auto-launch entry** for the daemon.

Your shell dotfiles (`.zshrc`, `.bashrc`, `.profile`) are never touched.

### Can Makakoo edit my code without asking?

No. The default write-sandbox is four paths:
`~/MAKAKOO/data/reports/`, `~/MAKAKOO/data/drafts/`,
`~/MAKAKOO/tmp/`, `/tmp/`. Everything else requires either a CLI-level
grant (`makakoo perms grant <path> --for 1h`) or a conversational
approval ("Want me to grant myself 1h access to `~/code`?" → "yes").
Broad scopes (`/`, `~`, `*`, `**`) are refused regardless.

Full model: [docs/user-manual/makakoo-perms.md](user-manual/makakoo-perms.md).

### Can I use my own LLM API key?

Yes. Either store it via `makakoo secret set AIL_API_KEY` (OS keyring,
preferred) or set the `AIL_API_KEY` env var. Point at your gateway via
`AIL_BASE_URL`. Makakoo supports any OpenAI-compatible endpoint out of
the box; other shapes (Anthropic direct, OpenRouter, etc.) are
first-class via the adapter registry — see `makakoo adapter list`.

### What if I don't want ANY cloud LLM in the loop?

Point `AIL_BASE_URL` at a local runtime — Ollama, LM Studio, llama.cpp,
switchAILocal, or your own proxy. Everything in Makakoo (Brain search,
SANCHO tasks, agent tool-calls) routes through the same gateway, so
flipping one env var keeps everything local.

### How do I uninstall?

Clean removal in three commands:

```sh
makakoo uninfect         # strip bootstrap from every CLI config
makakoo daemon uninstall # remove LaunchAgent / systemd unit / auto-launch
rm -rf ~/.makakoo ~/MAKAKOO   # ⚠️  deletes your Brain
```

Full guide: [Uninstall](troubleshooting/uninstall.md).

### How do I upgrade to a newer version?

```sh
makakoo upgrade            # auto-detects how you installed (cargo / brew / curl-pipe)
makakoo upgrade --dry-run  # preview the plan without spawning anything
makakoo upgrade --reinfect # also refresh CLI bootstrap fragments after the binary swap
```

Available since v0.1.3 (2026-05-02). For pre-v0.1.3 binaries you'll first
need a one-time manual upgrade — `brew upgrade traylinx/tap/makakoo`,
`cargo install --git https://github.com/makakoo/makakoo-os --locked --force makakoo`,
or re-run the curl-pipe install script — to land v0.1.3+, after which
`makakoo upgrade` is the one-command path forever after.

Full reference: [`docs/upgrade.md`](upgrade.md) and [`docs/user-manual/makakoo-upgrade.md`](user-manual/makakoo-upgrade.md).

### My AI CLI session feels slower after infect. Why?

The bootstrap block adds ~5 KB to every new session's system prompt.
On modern LLMs that's ~2000 tokens, which adds ~50-200ms to the first
turn. After the first turn it's cached by the LLM provider. If you
measure perceptible latency growth, run `makakoo infect --verify` —
sometimes the block got written twice.

### Can I use Makakoo with Obsidian / Logseq instead of its default Brain?

Yes. The Makakoo Brain at `~/MAKAKOO/data/Brain/` IS plain markdown —
Logseq format by default. Two scenarios:

- **Use Obsidian as a UI over the existing Brain.** No registration
  needed. Open Obsidian → `Open folder as vault` → point at
  `~/MAKAKOO/data/Brain/`. Same files, Obsidian UX.
- **Connect a SEPARATE vault.** Run `makakoo setup brain` or
  `makakoo brain add personal obsidian ~/Documents/MyVault`.

See [use case #3](use-cases.md#3-connect-my-obsidian-vault-or-a-folder-of-notes).

### Does Makakoo work on a corporate / locked-down machine?

Mostly yes. Requirements:
- Can run a non-signed binary (macOS Gatekeeper allows your own
  downloads once you right-click → Open once).
- Can install Homebrew (macOS) or use your distro's package manager
  (Linux) for optional installs like Ghostty / ripgrep — otherwise
  Makakoo still works, just with fewer built-ins.
- Windows needs Developer Mode for symlinks.

No admin / sudo needed for Makakoo itself. Some optional installs
(Homebrew taps, npm globals) may require elevation depending on how
your IT department configured npm/brew.

### Where do I report bugs?

GitHub issues at
[`github.com/makakoo/makakoo-os/issues`](https://github.com/makakoo/makakoo-os/issues)
once the repo goes public. Until then: ask on the private channels
you know about.

### What's the difference between `~/.makakoo/` and `~/MAKAKOO/`?

- `~/MAKAKOO/` — your data. Brain journals, pages, installed plugin
  copies, state files. Keep this backed up if you care about the
  content.
- `~/.makakoo/` — machine-local. Adapter registry, trust keys,
  primary-adapter choice, cache. Safe to delete (you lose adapter
  trust and primary choice).

The naming split exists because `~/.makakoo/` follows the Unix hidden-
dotfile convention for machine-local config (like `~/.aws/` or
`~/.docker/`), while `~/MAKAKOO/` is your visible content.

### Can Makakoo agents share files with each other?

Yes — via [garagetytus](concepts/shared-storage.md), a self-hosted
S3-compatible daemon. The `garage-store` plugin manifest is
**default-bundled in `core` and `sebastian` distros** (since
2026-04-27); lean distros (`minimal`, `creator`, `trader`) opt in
via `makakoo plugin install garage-store`. The garagetytus binary
itself is still a separate one-liner install (`brew install
traylinx/tap/garagetytus` or the curl recipe), but the plugin
soft-fails when the binary is absent — your CLI starts cleanly
either way.

Two flavors: a local laptop daemon at `127.0.0.1:3900` (zero
internet exposure, single user) or the Tytus shared service at
`https://garagetytus.traylinx.com` (multi-tenant, per-bucket SigV4
keys, reachable from any machine). Both speak the standard S3 wire
protocol — boto3, aws-cli, rclone all work unchanged. See
[Walkthrough 13](walkthroughs/13-shared-storage-garagetytus.md) for
the copy-paste recipe.

### Why is it named "Makakoo" (double-o)?

The owned domain is `makakoo.com` — double-o matches. The persona
("Harvey" on Sebastian's install) is configurable per-user via
`config/persona.json`; Makakoo is the platform, the name is yours.
