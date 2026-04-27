# Walkthrough 05 — Ask Harvey a question

## What you'll do

Ask Harvey a question **two ways** and understand when to use each:

1. **Through an AI CLI you already have** (Claude Code, Gemini CLI, …) — this is the grandma path. Your Brain shows up automatically inside every infected CLI. Works with zero LLM setup in Makakoo itself.
2. **Directly via `makakoo query`** — the power-user path. Requires configuring a model provider in Makakoo.

**Time:** about 5 minutes (path 1) or 10 minutes (path 2). **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md), at least one Brain entry (do [Walkthrough 02](./02-first-brain-entry.md) first if the Brain is empty).

## Path 1 (recommended) — Ask through an infected CLI

### Why this path is the right default

When walkthrough 01 ran `makakoo install`, one of the steps was `infect`: it added a small "bootstrap block" to the global instructions file of every AI CLI it detected on your Mac (Claude Code, Gemini CLI, Codex, OpenCode, Cursor, Vibe, Qwen). That block tells the AI where your Brain is, what persona you use (Harvey by default), and what tools are available.

So any question you ask in Claude Code (or any of the others) is already a Brain-aware question. Makakoo isn't a separate chatbot — it's **the memory layer every AI on your machine shares**.

### Steps

#### 1. Confirm the CLI is infected

```sh
makakoo infect --verify
```

Expected output (the list depends on which CLIs you have installed):

```text
makakoo infect --verify (full drift scan) — 7 target(s)
  claude     clean
  gemini     clean
  codex      clean
  opencode   clean
  vibe       clean
  qwen       clean
  cursor     clean
```

`clean` means the bootstrap block is present and up to date. If any target shows `missing` or `drifted`, rerun `makakoo infect` to fix it.

If the list is empty (`0 target(s)`), you have no AI CLI installed yet. Pick one to install (Claude Code is a good default: <https://claude.com/claude-code>) and come back.

#### 2. Open the CLI

On macOS:

```sh
claude        # or: gemini, codex, opencode, cursor, vibe, qwen
```

The CLI opens its interactive prompt.

#### 3. Ask a Brain-aware question

Type one of these at the CLI prompt (substitute your own wording):

```text
Find in my Brain what I wrote about Makakoo the first time.
```

Or something more specific to what you've put in the Brain:

```text
Harvey, summarize every journal entry from the last 7 days.
```

Or if you followed walkthrough 02:

```text
What was the first line I wrote in today's journal?
```

**What happens behind the scenes:** the CLI reads its bootstrap block on startup, notices you're asking about the Brain, reaches into `~/MAKAKOO/data/Brain/` via the MCP tools (`brain_search`, `brain_query`, etc.), and folds the result into its answer.

You should see the CLI cite specific journal entries or pages, with dates and file names.

## Path 2 — Ask directly via `makakoo query`

This uses a bare `makakoo` command instead of an AI CLI. Useful for shell scripting, piping into other tools, or simply not wanting to open a separate CLI for one question.

**Tradeoff:** `makakoo query` needs an LLM provider configured in Makakoo itself. The infected-CLI path in Path 1 reuses the AI CLI's own authentication — you don't configure anything in Makakoo for that to work. `makakoo query` is the opt-in path when you want a dedicated Makakoo ↔ LLM pipe.

### Steps

#### 1. Run the model-provider section of the setup wizard

```sh
makakoo setup model-provider
```

This is an interactive prompt. It asks which provider to use (Anthropic / OpenAI / a local gateway such as `switchAILocal` / Ollama / …), asks for an API key if needed, and writes the result to `~/MAKAKOO/config/model_provider.toml`.

Answer with the provider you already have an account for. If you don't have any, pick a local option (Ollama is free, runs on your machine, no account needed).

#### 2. Ask a question

```sh
makakoo query "what did I do today"
```

Expected output on a configured install:

```text
(retrieval: 5 hits from recall)
Answer: Today you wrote your first Brain entry at ~/MAKAKOO/data/Brain/journals/2026_04_24.md, which said "Tried Makakoo for the first time. [[Harvey]] is my assistant. The Brain is at `~/MAKAKOO/data/Brain/`."
```

The exact wording depends on the model; the shape is the same: retrieval hits first, then a synthesized answer.

#### 3. See what retrieval fed the LLM

Use `--show-memory` to inspect what the retrieval layer put into the LLM's context:

```sh
makakoo query --show-memory "what did I do today"
```

You'll see two sections: the **memory block** (L0 + L1 + L2 context) and then the **answer**.

### Known gotcha on v0.1.0

> `makakoo query` may error with `unknown provider for model ail-compound` on installs where the default model alias isn't registered with the gateway. See DOGFOOD-FINDINGS F-004. Workaround: set an explicit model with `--model <name>` if your provider exposes a different alias, or use Path 1 (ask through an infected CLI) until this is fixed.

## What just happened?

- **The Brain is not a chatbot.** It's a *memory layer*. Chat happens in the CLI of your choice; Makakoo makes sure every CLI can see the same memory.
- **`makakoo infect`** is what makes a CLI Brain-aware. It writes one short block to the CLI's global instructions file. You can see exactly what got added with `cat ~/.claude/CLAUDE.md` (or the equivalent for the CLI you checked).
- **`makakoo query`** is a convenience wrapper: FTS retrieval + LLM synthesis, in one command. Great for scripts, optional for humans.
- **You are never locked in to one model.** Switch provider, the Brain stays. Uninstall the AI CLI, install a different one, rerun `makakoo infect`, the new one reads the same Brain.

## If something went wrong

| Symptom | Fix |
|---|---|
| `makakoo infect --verify` shows `drifted` | The bootstrap block is out of sync. Run `makakoo infect` to rewrite it. |
| The CLI says it doesn't know about Makakoo | The CLI was infected but you didn't restart it after. Close and reopen the CLI. |
| The CLI asks you what Makakoo is | The bootstrap block is there, but the CLI's context window dropped it. Mention "Makakoo OS" explicitly in your question so the model pulls the block back into attention. |
| `makakoo query` errors with `unknown provider for model ail-compound` | DOGFOOD-FINDINGS F-004. Try `makakoo query --model <alias-your-provider-uses>`, or fall back to Path 1. |
| `makakoo setup model-provider` hangs | Check if stdin is a real TTY (`isatty(0)` sanity check). The wizard auto-skips in non-interactive shells; force re-prompt with `makakoo setup model-provider --reset`. |

## Next

- [Walkthrough 06 — Grant write access](./06-grant-write-access.md) — let Makakoo write to a specific folder without disabling its sandbox.
- [Walkthrough 09 — Ingest a document](./09-ingest-document.md) — pull a PDF into the Brain and then query against it.
