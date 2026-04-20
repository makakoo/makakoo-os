# Introducing Lope: Any AI CLI Implements. Any AI CLI Validates.

```
 ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
 ████████████████████████████████████████████████

    ██      ██████    ██████   ███████
    ██     ██◉  ◉██   ██   ██  ██
    ██     ██ ▽▽ ██   ██████   █████
    ██     ██ ◡◡ ██   ██       ██
    ██████  ██████    ██       ███████

 ████████████████████████████████████████████████
 ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
     any cli implements  ·  any cli validates
```

Today we're open-sourcing **Lope**: an autonomous sprint runner with a multi-CLI validator ensemble. One AI CLI writes the code. A different AI CLI reviews it. Majority vote decides if the phase ships. No single-model blindspot.

GitHub: [github.com/traylinx/lope](https://github.com/traylinx/lope) · MIT licensed · v0.3.0 (first public release) · Zero Python dependencies.

## The blindspot problem nobody wants to admit

Here's a thing that happens every day in every codebase that uses AI assistants. Claude writes a function. You ask Claude if the function is correct. Claude says yes. You ship it. It breaks in production because the edge case Claude didn't think of writing is also the edge case Claude didn't think of reviewing.

This is **correlated failure**. The same reasoning that produced the bug approves the bug. It's not a Claude problem specifically. GPT-4 has it. Gemini has it. Every model has it when it reviews its own output, because the blindspots in its training distribution show up on both sides of the loop.

Humans solved this a long time ago. We call it code review, and the entire point is that **a different pair of eyes with a different frame** catches things the first pair missed. Two senior engineers who trained at different companies will find different bugs in the same diff. That's not a flaw. That's the mechanism.

Lope applies that same principle to AI-written code. Different models have different blindspots. Run them against each other and the gaps show up.

## What Lope actually does

Lope is a sprint runner. You tell it a goal. It drafts a structured sprint document (phases, deliverables, success criteria), sends the draft to your chosen AI CLIs for review, revises until everyone passes, and then executes the sprint phase by phase with another validation pass after each phase.

Here's what running it looks like from inside Claude Code:

```
You:    /lope-negotiate "Add JWT auth with refresh tokens"

  Round 1  drafter proposes sprint doc (4 phases)
  Round 1  opencode + vibe + gemini review... NEEDS_FIX (0.78)
           - Missing rate limiting on refresh endpoint
           - No test for token expiry edge case
  Round 2  drafter revises
  Round 2  opencode + vibe + gemini review... PASS (0.93)

  Saved: SPRINT-JWT-AUTH.md

You:    /lope-execute SPRINT-JWT-AUTH.md

  Phase 1  scaffold ................ PASS  0.95  12s
  Phase 2  core-middleware ......... PASS  0.91  34s
  Phase 3  refresh-rotation ........ PASS  0.88  28s
  Phase 4  integration-tests ....... PASS  0.94  19s

  4/4 PASS  |  avg confidence 0.92  |  93s
```

That's the entire loop. Negotiate, execute, audit. Each phase produces a verdict block with status, confidence, and specific fix instructions on failure. Three attempts per phase before it escalates. The whole thing is logged to a journal file so you can grep "why did phase 2 fail last Tuesday" six months later.

## 12 built-in validators, infinite custom ones

Lope ships with adapters for 12 AI CLIs out of the box. All of them are **unmodified, off-the-shelf binaries** — no plugins, no API key juggling inside lope. If `claude --print "hello"` works in your terminal, Claude is a Lope validator.

The twelve:

| CLI | What it is |
|---|---|
| **Claude Code** | Anthropic's coding CLI |
| **OpenCode** | The SST team's terminal agent |
| **Gemini CLI** | Google's CLI |
| **OpenAI Codex** | OpenAI's new coding CLI |
| **Mistral Vibe** | Mistral's fresh coding CLI (first-class adapter) |
| **Aider** | The classic git-native coder |
| **Ollama** | Local models, zero auth |
| **Goose** | Block's open-source agent |
| **Open Interpreter** | The OG code interpreter |
| **llama.cpp** | Fastest local inference |
| **GitHub Copilot CLI** | GitHub's `gh copilot` command |
| **Amazon Q** | AWS's developer assistant |

And if you want something that isn't in that list, you add it with five lines of JSON in `~/.lope/config.json`. Two provider types cover every AI backend: `subprocess` for CLI tools and `http` for API endpoints. Placeholders for prompt and environment variables. Security is enforced (no shell interpolation, no env leaks into argv). That's the whole extension mechanism.

Full Groq, Anthropic API, OpenAI API, self-hosted endpoint, or obscure local binary — as long as it responds in text, Lope can vote with it.

## Not just code: engineering, business, research

This is the part that surprised us most when we used it internally. The same validator loop that catches bugs also catches gaps in **a marketing brief, a financial close process, a systematic literature review, a consulting deliverable, or a GDPR compliance audit.**

Pass `--domain business` and the validator role shifts. Instead of a senior staff engineer reviewing for bugs, you get a senior operations lead reviewing for timeline realism, budget allocation, KPI definition, and audience fit. Pass `--domain research` and you get a principal researcher reviewing methodology rigor, sampling bias, and reproducibility.

```bash
# Engineering
lope negotiate "Add rate limiting to the API gateway"

# Business
lope negotiate "Q2 marketing campaign for enterprise segment" --domain business

# Research
lope negotiate "Systematic review of transformer efficiency papers" --domain research
```

Same core loop. Different role prompt, different labels, different checks. We have a dedicated post on this coming up but the punchline is: **if you can write it down as phases with deliverables and success criteria, lope can validate it.**

## Intelligent caveman mode

The phrase is a wink. The mode sounds primitive (drop articles, grunt in fragments) but the compression is linguistically precise. Code, paths, line numbers, error messages, and commands stay **exact**. Only the wrapper prose gets grunted away.

Before, validator response:
> "I've reviewed the proposal carefully and I think there are a few issues that should probably be addressed before moving forward. Specifically, I noticed that the authentication middleware doesn't appear to handle the case where the token has just expired..."

After, same validator, caveman mode on:
> "Auth middleware misses expired-token edge case. Add rate limit on /refresh endpoint. Fix at middleware/auth.go:142."

60 tokens vs 22 tokens. **50 to 65 percent fewer tokens per response**, same information. Across a sprint with 3 validators, 4 phases, and 1 retry average, that's 36 validator calls. The savings compound fast on paid APIs.

It's on by default. `LOPE_CAVEMAN=off` if you want verbose. Credit where credit's due: the core terseness rules come from [JuliusBrussee/caveman](https://github.com/JuliusBrussee/caveman) (MIT). Lope wires them into the validator prompt injection pipeline.

## What's new in v0.3.0

v0.3.0 is the first public release, and it ships three architectural moves that most validator loops I've seen skip:

**Two-stage validator review per phase.** After each phase, validators run a **spec compliance** pass first ("does this output match the phase goal?"), then a separate **code quality** pass ("is this well-built?"). A spec NEEDS_FIX short-circuits the quality pass, so you never waste a quality review on code that doesn't even match the requirement. A spec FAIL escalates immediately. This separation stops "clever slop that misses the goal" from sliding through on a polished-looking diff.

**Verification-before-completion evidence gate.** Any validator that returns PASS with a rationale that lacks evidence — no `file:line` reference, no test output, no code fence, no explicit verification phrase — gets auto-downgraded to NEEDS_FIX, with a synthesized fix instruction to "provide evidence." This kills rubber-stamping at the framework level. You don't have to trust that your validators are being rigorous; the framework enforces it.

**No-placeholder lint on drafts.** If the negotiator produces a sprint doc that contains `TBD`, `TODO`, `XXX`, `FIXME`, bare prose ellipsis, `<placeholder>` tokens, or phases with empty artifact/check lists, the lint fires and the drafter loops back with specific fix instructions before any validator round. This is much cheaper than paying validators to say "you forgot to fill in phase 3."

On top of that, v0.3.0 adds:

- **A SessionStart hook** that auto-briefs your AI agent that lope is available, so users don't have to remember slash commands
- **A `using-lope` auto-trigger skill** that recognizes natural-language descriptions of multi-phase work ("plan the auth refactor", "scope the data migration") and invokes lope on your behalf
- **Plugin manifest files** for Claude Code, Cursor, and Gemini CLI so lope surfaces in each host's plugin marketplace format
- **Paste-a-prompt install** (see below)

## Install: paste one line into any AI agent

No pip. No virtualenv. No dependency hell. Pure Python stdlib.

Copy this line and paste it into any AI agent you already use — Claude Code, Codex, Gemini CLI, OpenCode, Cursor:

```
Read https://raw.githubusercontent.com/traylinx/lope/main/INSTALL.md and follow the instructions to install lope on this machine natively.
```

Your agent fetches `INSTALL.md`, follows the six short steps, auto-detects which CLIs you already have, and writes slash commands and skills into each host's native directory in the format that host expects. About 30 seconds end to end. Reports back when it's live.

After install, these slash commands work inside your CLI of choice:

- `/lope-negotiate` — draft a sprint doc with multi-round validator review
- `/lope-execute` — run sprint phases with validator-in-the-loop retry
- `/lope-audit` — generate a scorecard with per-phase verdicts

Or use the CLI directly:

```bash
alias lope='PYTHONPATH=~/.lope python3 -m lope'
lope status
lope configure
lope negotiate "Your first sprint goal"
```

## You don't have to type slash commands

The slash commands are there for people who want them. But most users will describe work in natural language — "plan the auth refactor carefully", "scope the data migration", "negotiate the Q4 campaign, needs to be right" — and the `using-lope` skill recognizes those shapes and invokes lope for them. Trigger words the skill looks for: *plan, negotiate, scope, draft, roll out, work through, carefully, don't break things, needs to be right.*

No slash-command muscle memory required.

## Where Lope came from

I built lope because I got tired of shipping AI code that only one model had reviewed. Every time something broke in production, I'd go back to the sprint and the validation pass and watch the same reasoning that produced the bug approve the bug in review. Single-model judgment. Correlated failure. The blindspots show up on both sides of the loop and cancel each other out.

I wired up an ensemble validator loop as an internal tool, ran it for a few months across a mix of code sprints and non-code artifacts (marketing plans, budget reviews, research protocols), and watched it catch things I would have missed. When the loop started earning its keep on non-code work, I extracted it as a standalone primitive.

More on the origin in [a follow-up post](./04-origin-story.md).

## Try it, break it, open issues

This is v0.3.0 — the first public release. It works well for the use case it solves (multi-CLI sprint validation across engineering, business, and research domains) but we are actively learning from users. If you hit a rough edge, open an issue. If you find a pattern we should codify, open a PR. If you build a custom validator for a CLI we don't support yet, share the JSON config in the repo's discussions so others can use it.

The repo is [github.com/traylinx/lope](https://github.com/traylinx/lope). Star it if you like it. Use it if you need it. Tell us what breaks.

— Sebastian
