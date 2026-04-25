---
name: makakoo
version: 1.0.0
description: |
  Drop-in onboarding for any AI CLI (Claude Code, Codex, Gemini, OpenCode,
  Cursor, Vibe, Qwen, pi) running on a machine that has Makakoo OS
  installed. Read this file once and you know:
    - what `makakoo` is (a persistent shared-Brain layer for every AI CLI)
    - which commands actually exist (vs the ones documentation drifted on)
    - how to make the current project Makakoo-aware in one command
    - where to find deep references when you need them
  Use when the user mentions "makakoo", "harvey", "infect", "the brain",
  or runs any `makakoo …` command without a working AGENTS.md / CLAUDE.md
  in the current dir.
allowed-tools:
  - shell
canonical-url: https://raw.githubusercontent.com/makakoo/makakoo-os/main/.agents/skills/makakoo/SKILL.md
---

# Use Makakoo OS from inside an AI CLI

> **Magic one-liner** to paste at the top of any AI CLI session that does NOT yet have a `CLAUDE.md` / `AGENTS.md` / `GEMINI.md` etc. in the current dir:
>
> ```
> Read https://raw.githubusercontent.com/makakoo/makakoo-os/main/.agents/skills/makakoo/SKILL.md and follow the instructions.
> ```
>
> One curl. The AI now knows what Makakoo is, which commands exist, and how to use them — without grepping the user's filesystem.

## What Makakoo OS is (in one paragraph)

Makakoo OS is a **persistent shared-Brain layer** that gives every AI CLI on the user's machine the same memory: journals, pages, MCP tools, a proactive task engine. The user's CLI choice (Claude Code, Codex, Gemini, OpenCode, Cursor, Vibe, Qwen, pi) is interchangeable; the Brain is shared. Persona is "Harvey" by default. The platform name is "Makakoo OS"; the AI persona is "Harvey" — both names are correct in different contexts.

## Hard rules for the AI (you)

- **Zero sycophancy.** No "Great question!" preamble. Just execute.
- **Never auto-send** emails, LinkedIn messages, Slack messages, or anything visible to a third party — always draft for review first.
- **Never run destructive commands** (`rm -rf`, `git reset --hard`, `git push --force`) without explicit user approval.
- **The user is the boss.** You are the user's autonomous extension, not an independent agent.

## Real `makakoo` subcommands (verified 2026-04-25)

The full subcommand surface — **only these exist**; do not invoke ones that look plausible but aren't on this list:

| Subcommand | Purpose |
|---|---|
| `makakoo --version` | Print binary version |
| `makakoo version` | Version + persona + `$MAKAKOO_HOME` path |
| `makakoo install` | One-shot: distro install + daemon register + global infect |
| `makakoo setup` | Interactive setup wizard (sections: persona, brain, cli-agent, terminal, model-provider, infect) |
| `makakoo infect` | Write the Makakoo bootstrap block into every detected CLI's global slot |
| `makakoo infect --verify` | Audit-only: report drift across all CLIs (exit 1 on drift) |
| `makakoo infect --local` | Project-scoped: write `.harvey/context.md` + `CLAUDE.md` / `AGENTS.md` / `GEMINI.md` / `QWEN.md` / `.cursor/rules/makakoo.mdc` / `.vibe/context.md` into the nearest project root (the dir containing `.git/` or `.harvey/`, walking up from cwd) |
| `makakoo infect --local --dir <PATH>` | Project-scoped, but pin the target explicitly |
| `makakoo infect --local --target codex` | Project-scoped, write ONLY the named CLI's derivative (e.g. `codex` → `AGENTS.md`). Comma-separated for multiple. Tokens: `claude`, `gemini`, `codex`, `opencode`, `qwen`, `cursor`, `vibe`. |
| `makakoo infect --local --dry-run` | Preview without writing |
| `makakoo uninfect` | Strip Makakoo bootstrap from every detected CLI's global slot |
| `makakoo plugin list` | Every installed plugin (table or `--json`) |
| `makakoo plugin info <name>` | Manifest + lock entry for one plugin |
| `makakoo plugin install <source>` | Install — accepts `path/to/dir`, `git+<url>[@ref]`, `https://…/x.tar.gz --sha256 <hash>`, or `--core <name>` from a checkout |
| `makakoo plugin enable <name>` / `disable <name>` | Soft toggle |
| `makakoo plugin uninstall <name> [--purge]` | Remove |
| `makakoo agent {start,stop,status,health} <plugin>` | Drive an agent's `[entrypoint]` lifecycle (or pgrep fallback) |
| `makakoo daemon {install,uninstall,status,logs,run,restart}` | LaunchAgent / systemd unit lifecycle |
| `makakoo sancho {tick,status}` | Proactive task engine — fire eligible / inspect |
| `makakoo nursery {list,hatch}` | Mascot registry |
| `makakoo buddy status` | Active mascot's ASCII frame + state |
| `makakoo dream` | Force a Brain consolidation pass |
| `makakoo sync [--force] [--embed]` | Index `Brain/pages/`, `Brain/journals/`, `data/auto-memory/` into FTS5 |
| `makakoo search <query>` | Full-text search across the Brain |
| `makakoo query <question>` | FTS retrieval + LLM synthesis (requires a configured model provider) |
| `makakoo memory stats` | Recall log + promotion candidates |
| `makakoo promotions [--threshold N] [--limit N]` | Memory-promoter candidates |
| `makakoo flag <reason> [--skill <s>]` | Manual GYM error funnel entry |
| `makakoo octopus {bootstrap,invite,join,trust,doctor}` | Signed-MCP peer federation (read DOGFOOD-FINDINGS for known gotchas) |
| `makakoo perms {grant,revoke,list,audit,purge,show}` | Runtime write-permission grants |
| `makakoo session {…}` | JSONL session-tree management (gated, default off) |
| `makakoo adapter {list,info,spec,gen,…}` | External-AI adapter management |
| `makakoo skill <name>` | Run a Python skill by name (NOT a subcommand-style — name is positional) |
| `makakoo secret {set,get,remove,list}` | Keyring-backed secrets |
| `makakoo distro {list,install,info}` | Distro management (curated plugin bundles) |
| `makakoo migrate [--dry-run]` | Prepare `$MAKAKOO_HOME` for kernel use |

**Subcommands that DO NOT exist** (despite older docs / batch-migrated SKILL.md mentions):

- `makakoo doctor` — there is no global doctor. Use `makakoo octopus doctor`, `makakoo sancho status`, `makakoo memory stats`, `makakoo daemon status`.
- `makakoo skill list` / `makakoo skill run <name>` — no subcommand structure. The Rust binary takes `makakoo skill <name>` directly. To list installed skills use `makakoo plugin list`.

## "Make this project Makakoo-aware" — the 1-command answer

If a user asks something like *"infect this", "make this dir Makakoo-aware", "set up Makakoo here", "infect --local"*:

```sh
makakoo infect --local
```

If they want to scope to **just one CLI** (e.g. only Codex, not all six derivatives):

```sh
makakoo infect --local --target codex      # writes only AGENTS.md
makakoo infect --local --target claude     # writes only CLAUDE.md
makakoo infect --local --target gemini     # writes only GEMINI.md
makakoo infect --local --target codex,gemini   # comma-separated for multi
```

Tokens: `claude`, `gemini`, `codex`, `opencode`, `qwen`, `cursor`, `vibe`. The `codex` and `opencode` tokens both map to `AGENTS.md` (Codex and OpenCode share that filename convention).

Notes:
- **No positional arg.** It does NOT take `this` / `here` / a path as a positional. The shell will reject `makakoo infect --local this` with `unexpected argument 'this' found`. Use `--target <cli>` for CLI scoping or `--dir <path>` for directory pinning.
- It walks **upward** from cwd looking for `.git/` or `.harvey/`, and writes into that root. To pin the dir explicitly: `makakoo infect --local --dir .` (or `--dir <absolute-path>`).
- It writes 6 files: `.harvey/context.md` (canonical) + `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `QWEN.md`, `.cursor/rules/makakoo.mdc`, `.vibe/context.md` (per-CLI derivatives). Codex reads `AGENTS.md`; Claude Code reads `CLAUDE.md`; Gemini reads `GEMINI.md`; etc.
- After running, **the current AI session does NOT auto-reload its system prompt**. Tell the user to close and reopen the CLI in that dir for full effect — or just keep using the project, the new chat will pick it up.

## Where the user's Brain lives

- **`$MAKAKOO_HOME`** — usually `~/MAKAKOO/`. `$HARVEY_HOME` is the legacy alias and resolves to the same dir.
- **`$MAKAKOO_HOME/data/Brain/journals/YYYY_MM_DD.md`** — daily journal. Every line **must** start with `- ` (Logseq outliner format).
- **`$MAKAKOO_HOME/data/Brain/pages/<entity>.md`** — one page per entity / project / person. Wikilinks `[[Name]]` cross-reference.
- **`$MAKAKOO_HOME/data/auto-memory/`** — curated cross-session insights. Index at `MEMORY.md`.

## When to write to the Brain

After significant work (bug fixed, feature shipped, decision made), append a `- [[Topic]] one-line summary` to the day's journal. Then run `makakoo sync` so search picks it up. This is what makes Makakoo a memory layer instead of a stateless toolset.

## Common user phrases → the right command

| User says | You run |
|---|---|
| "what's installed" / "list plugins" / "what skills do I have" | `makakoo plugin list` |
| "search the brain for X" | `makakoo search "X"` |
| "what did I do today / yesterday" | `makakoo query "summary of recent journal entries"` (needs LLM); or `cat ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md` |
| "remember X" / "log X" / "save X to brain" | Append `- <X>` to today's journal file, then `makakoo sync` |
| "infect this project" / "make this dir Brain-aware" | `makakoo infect --local` |
| "why is X broken" | `cat ~/MAKAKOO/data/Brain/journals/*.md \| grep -i X`; or `makakoo search "X error"`; or check `~/MAKAKOO/data/logs/` |
| "ingest this PDF / video / URL" | Use the `harvey_knowledge_ingest` MCP tool (NOT `harvey_describe_*` — that's for one-shot Q&A only) |
| "browse this URL" / "open this site" | Use the `harvey_browse` MCP tool (real Chrome via CDP) |

## When something fails

The repo ships a **symptom-rooted troubleshooting tree** at `docs/troubleshooting/tree.md`. Top-level categories:

1. I ran a command and got an error
2. The command ran but nothing happened
3. It worked yesterday, not today
4. I don't know what command to run
5. Harvey / MCP not responding
6. Plugin install failed
7. Octopus peer unreachable

Verbatim error-string index at `docs/troubleshooting/symptoms.md`.

## Deeper references in the repo

- `docs/walkthroughs/` — 12 dependency-chained end-to-end guides covering every major feature.
- `docs/agents/` — per-agent manuals (15 agents).
- `docs/mascots/` — per-mascot manuals (5 mascots).
- `docs/user-manual/` — per-CLI-subcommand reference.
- `docs/concepts/` — architecture, distros, IDE integration.
- `docs/api/mcp-tools.md` — MCP tool reference.

These are all reachable on the local filesystem under `<makakoo-os-checkout>/docs/` OR over the web at `https://raw.githubusercontent.com/makakoo/makakoo-os/main/docs/...`.

## What NOT to do

- Don't grep the entire filesystem to figure out what `makakoo` does. The list above is authoritative.
- Don't invoke `makakoo skill canary` expecting a useful result — it's a documentation-only skill that prints a pointer.
- Don't journal a URL as a workaround when `harvey_describe_*` rate-limits — that poisons retrieval. Wait for the rate limit to clear, or route through `harvey_knowledge_ingest` (different embedding path).
- Don't claim a feature exists without verifying it on the list above. If unsure, run `makakoo --help`.

## You are now ready

If the user has Makakoo installed (`which makakoo` returns a path), every command above works. If they don't, point them at:

```sh
curl -fsSL https://makakoo.com/install | sh   # public path, post-v0.1.0
# or from source:
git clone https://github.com/makakoo/makakoo-os && cd makakoo-os && \
  cargo install --path makakoo && cargo install --path makakoo-mcp && \
  makakoo install
```
