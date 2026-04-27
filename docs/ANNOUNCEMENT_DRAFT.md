# v0.1 announcement — draft

> **Status**: draft. Sebastian approves + customises the voice before any of this goes public.
> **Channels**: blog post → HN Show HN → LinkedIn → Twitter/X thread → optional Reddit r/rust + r/opensource crosspost.

---

## Blog post — "Makakoo OS 0.1: one brain for every AI CLI on your machine"

AI agents don't remember. Every Claude session starts from zero. Every Gemini CLI conversation is a goldfish. Every terminal assistant you've ever opened has its own little fishbowl, and the one you'll open tomorrow won't know what the one you opened yesterday decided.

That's a dumb way to live.

**Makakoo OS** is an open-source autonomous cognitive extension. One install gives every AI CLI on your machine — Claude Code, Gemini CLI, Codex, OpenCode, Vibe, Cursor, Qwen, plus VSCode Copilot and JetBrains AI — the same persistent brain. A Logseq-backed journal you can read. A superbrain query (FTS5 + vector + LLM synthesis) they can all call. A capability-sandboxed plugin system so skills written once run from every host. Proactive maintenance tasks that run while you sleep.

Many bodies. One mind.

### Why this exists

I've been building Sebastian's daily-driver AI rig for about a year. Every time a new CLI showed up — Claude Code came out, Gemini CLI shipped, Cursor added agent mode — I'd spend a weekend wiring it into the same Brain I'd spent the last six months teaching. By month eight, the wiring itself had become the product: a parasitic bootstrap block, a stable MCP server, a capability socket, a SANCHO engine for the proactive tasks. I was installing Makakoo on every new CLI before I used the CLI.

Makakoo OS is that parasite, extracted, cleaned up, rewritten in Rust, made portable, and shipped as an open-source OS anyone can install in 30 seconds.

### What you get in v0.1

- **`curl | sh` install** on macOS + Linux, `iwr | iex` on Windows. One-liner ceremony you already recognise.
- **7 AI CLIs + 4 IDE hosts infected** from one command. `makakoo infect --global` writes a shared bootstrap into every detected host's instructions slot. `makakoo uninfect` reverses it cleanly.
- **Persistent Brain** — Logseq-format journals + pages the AI reads and writes through the same fast superbrain search you use from your own terminal.
- **38 plugin manifests** out of the box — skills, watchdogs, monitors, the mascot GYM. Capability-sandboxed. Write a new plugin in 20 seconds via the batch migrator.
- **Five distros** — `minimal`, `core`, `sebastian` (the dogfood install), `creator` (writers + streamers), `trader` (market-facing agents). Pick the bundle that matches the work.
- **Linux + macOS + Windows** first-class. CI runs all three on every commit. 839 tests, 0 failures.

### What it costs you

- MIT. Forever. No dual-license, no enterprise tier, no upsell.
- Zero telemetry. Your data never leaves your machine.
- No VC. No acquisition path. Community-maintained.

### What v0.1 explicitly does NOT ship

- Apple notarization / Windows code signing — runbook is in the repo; certs are the gating step.
- Homebrew tap + winget submission — manifests drafted, submissions happen in the week after launch.
- Full Python agent migration — 38 of 188 skills carry manifests; the remaining 150 + 11 agents port through v0.2.
- Demo video — filming during the launch week; pinned Discussion link will follow.

I'm shipping a thing that's honestly 90% done because sitting on the 10% for another three weeks doesn't help anyone. The README calls the gaps out; the CHANGELOG tracks them.

### How to try it

```sh
# macOS / Linux
curl -fsSL https://makakoo.com/install | sh

# Windows (Developer Mode on)
iwr -UseBasicParsing https://makakoo.com/install.ps1 | iex
```

Or from source:

```sh
git clone https://github.com/makakoo/makakoo-os
cd makakoo-os
cargo install --path makakoo
makakoo install
```

Everything's at [github.com/makakoo/makakoo-os](https://github.com/makakoo/makakoo-os). Issues welcome. Weird PRs — a new mascot, a translation of the README, a GPU-accelerated superbrain — especially welcome.

🦉 _eat sleep go bananas repeat_ 🍌

---

## HN Show HN post

**Title:** `Show HN: Makakoo OS – one persistent brain for every AI CLI on your machine`

**Body:**

> Hi HN. I've been building my own AI rig for about a year — every time a new CLI ships (Claude Code, Gemini CLI, Cursor agent mode) I'd wire it into the same Logseq-backed Brain I've been teaching. By month eight the wiring had become the product. So I extracted it, rewrote it in Rust, and open-sourced it.
>
> Makakoo OS is a thin kernel that gives every AI CLI on your machine the same persistent memory + tool surface. One install infects 7 CLIs + 4 IDE hosts with a shared bootstrap; a daemon runs proactive maintenance tasks (dream passes, memory consolidation, superbrain sync) while you sleep; a capability-sandboxed plugin system lets you write a skill once and call it from every host.
>
> v0.1 ships macOS + Linux + Windows. MIT. Zero telemetry. No VC. The README is honest about what's gated on external stuff (signing certs, Homebrew tap) vs what's code-complete (everything else).
>
> Repo: https://github.com/makakoo/makakoo-os
>
> Happy to answer Qs about the capability socket design (Unix domain socket + Windows named pipe, PID-verified, per-plugin grant table), the SANCHO engine (tokio-based gated tick loop), or the plugin manifest format. Or why the mascot is an owl.

---

## LinkedIn

**Post** (short-form, Sebastian-voice):

> Makakoo OS 0.1 just shipped.
>
> It's an open-source kernel that gives every AI CLI on your machine — Claude Code, Gemini, Codex, Cursor, Copilot, JetBrains AI — the same persistent brain. One install, one set of tools, one cognitive model across every body you open.
>
> Many bodies, one mind. MIT. No VC. Local-first.
>
> I've been dogfooding this for a year as my personal rig. Today it becomes a thing anyone can install.
>
> github.com/makakoo/makakoo-os
>
> 🦉

---

## Twitter/X thread

**1/**
Shipped **Makakoo OS v0.1** today — an open-source autonomous cognitive extension that gives every AI CLI on your machine the same persistent brain.

Claude Code, Gemini CLI, Codex, Cursor, Copilot, JetBrains AI → one Brain. One command installs into all of them.

**2/**
The problem: AI agents don't remember. Every session = zero. Every CLI has its own fishbowl.

Makakoo fixes that. `makakoo infect --global` writes a shared bootstrap block into every detected host. They all read + write the same Logseq-format Brain.

**3/**
Under the hood:

- Rust kernel (843 tests, CI green on macOS + Linux + Windows)
- Per-plugin capability socket (Unix domain socket / Windows named pipe, PID-verified)
- Manifest-driven plugin system — 38 shipped
- SANCHO proactive task engine running while you sleep

**4/**
What's honest about v0.1:

✅ Works end-to-end today — install, infect, shared Brain, plugins, daemon
✅ 3 OSes in CI on every commit
🟡 Signing certs + Homebrew tap coming v0.1.1
🟡 150 more skills + 11 agents to port → v0.2

**5/**
MIT. Zero telemetry. No VC. Community-built.

Try it: `curl -fsSL https://makakoo.com/install | sh`

Or from source: github.com/makakoo/makakoo-os

Weird PRs welcome. Especially mascot art.

🦉 _eat sleep go bananas repeat_

---

## Reddit (optional)

**Title:** `[Open Source] Makakoo OS — one persistent brain shared across every AI CLI on your machine`

**Subreddits:** `r/rust` (framework angle), `r/opensource` (philosophy angle), `r/LocalLLaMA` (local-first angle).

**Body:** start with a 2-paragraph hook from the blog, end with repo URL + install one-liner. Respond to comments for 2 hours after posting.

---

## Checklist before posting

- [ ] v0.1.0 tag is live on GitHub
- [ ] `makakoo.com/install` redirect works (serves `install.sh` over HTTPS)
- [ ] Release artifacts uploaded and checksums match Homebrew formula placeholders
- [ ] README renders cleanly on github.com (badges load, tables wrap)
- [ ] Demo video uploaded + link swapped into blog post
- [ ] CHANGELOG.md [Unreleased] section moved under [0.1.0] with the tag date
- [ ] Sebastian gives the explicit go-ahead for each channel individually
