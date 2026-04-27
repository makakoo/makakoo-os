# Why I Built Lope: The Single-Model Blindspot I Kept Tripping Over

A short origin story, because a few people have asked where lope came from and why I bothered building another sprint runner when there are already plenty.

The honest answer is: I didn't set out to build a sprint runner. I set out to stop shipping AI code that only one model had reviewed, and the loop I built to do that turned out to be useful enough that I extracted it.

## The problem that wouldn't go away

I was running structured development sprints with AI assistance. The pattern was the same every time: describe the work, have the agent draft a plan, ask the agent to execute the plan phase by phase. It worked well enough that I stopped writing code by hand for anything routine.

But I kept shipping things that broke in ways the model should have caught.

Every time it happened, I'd go back to the sprint and the validation pass and read them carefully. And every time, the same reasoning that produced the bug approved the bug in review. The model didn't notice the edge case when it wrote the code, and it didn't notice the edge case when it reviewed the code, because the blindspot was in the same place on both sides of the loop.

This is **correlated failure**. It's not a Claude problem or a GPT problem or a Gemini problem. It's what happens when a single model judges its own work. The blindspots in its training distribution show up twice, and they cancel each other out.

Humans solved this a long time ago. We call it code review, and the whole mechanism is that a different pair of eyes, with a different frame, catches things the first pair missed. Two senior engineers who trained at different companies will find different bugs in the same diff. That's not a flaw of human review, it's the entire point.

## The loop I ended up with

I wired up an ensemble validator loop. When the primary CLI drafts a sprint, it sends the draft to other CLIs — from different families, different vendors, different training data — for independent review. Each validator votes PASS, NEEDS_FIX, or FAIL with a confidence score and a rationale. Majority decides. On NEEDS_FIX, the drafter revises with specific fix instructions. On FAIL, the sprint escalates.

The same pattern runs after each phase during execution. Different model families catch different issues. Claude Code and Codex have overlap (both US-trained, both general-purpose) but Gemini picks up things they miss. Mistral Vibe flags different things again. Aider brings a git-native perspective. Ollama running a local model has a totally different blindspot profile. Five off-the-shelf CLIs reviewing each other produces better judgment than any one of them alone.

The first few sprints I ran through the loop caught bugs I would have shipped. Then they started catching **structural** issues, not just bugs. Missing rollback plans. Ambiguous phase boundaries. Success criteria that didn't match what the phase was actually testing. Scope creep the drafter hadn't noticed itself sneaking in.

That's when I realized the loop wasn't really about code. It was about **structured-work review with independent multi-model judgment**, and structured work is everywhere.

## Not just code

I started running the loop on non-code artifacts. A marketing plan for a side project. A quarterly budget review. A research protocol for a paper I was drafting. A legal review I was nervous about. Same loop, different role prompt. Same kinds of catches.

The marketing plan came back NEEDS_FIX because the validators flagged that the first draft conflated launch-week metrics with long-term retention metrics. The budget review came back NEEDS_FIX because one validator noticed the close plan had no dual-entry validation step during the middle of a system migration. The research protocol came back NEEDS_FIX because another validator asked whether the inclusion criteria handled non-English papers.

Every one of those catches came from a different model than the one that drafted the document. None of them were bugs in the "the code crashes" sense. They were gaps in the reasoning, and different families saw different gaps.

That's when I knew the loop belonged outside my personal workflow.

## The rename

I renamed the project to **lope** for three reasons.

First, "lope" sounds like "loop," which is the core abstraction. The whole thing is a loop: draft, review, revise, execute, review, revise, audit.

Second, **Lope de Vega**, the Spanish Golden Age playwright, wrote something like 1,800 plays by running a structured ensemble process with collaborators. He'd draft an outline, hand pieces to trusted writers, merge their revisions, ship. The model of a prolific drafter surrounded by an ensemble of reviewers fit the tool perfectly.

Third, the previous working name was boring SaaS-speak and I was tired of looking at it.

## What v0.3.0 looks like

v0.3.0 is the first public release. The things in it that I actually use every day:

- **Three modes.** `/lope-negotiate` drafts the sprint doc. `/lope-execute` runs the phases. `/lope-audit` generates the scorecard.

- **Two-stage validator review per phase.** First a spec-compliance pass ("does this match the goal?"), then a code-quality pass ("is this well-built?"). The spec pass short-circuits on NEEDS_FIX so you never waste a quality review on work that doesn't match the goal in the first place.

- **A verification-before-completion gate.** Any validator that returns PASS with a rationale that lacks evidence — no file:line reference, no test output, no explicit verification phrase — gets auto-downgraded to NEEDS_FIX. Kills rubber-stamping architecturally.

- **A no-placeholder lint on drafts.** If the negotiator produces a sprint doc with `TBD`, `TODO`, `[insert here]`, or phases with empty artifact lists, the drafter loops back before any validator sees it. Cheaper than a validator round.

- **12 built-in CLI adapters.** Claude Code, OpenCode, Gemini CLI, Codex, Mistral Vibe, Aider, Ollama, Goose, Open Interpreter, llama.cpp, GitHub Copilot CLI, Amazon Q. Plus infinite custom ones via five lines of JSON in `~/.lope/config.json`.

- **Three domains, not just code.** `engineering`, `business`, `research`. Same loop, different role prompt, different artifact labels.

- **Intelligent caveman mode.** Token-efficient validator prompts that drop articles and hedging while keeping code, paths, line numbers, and error messages exact. Roughly 50-65% token savings per validator call in my internal measurements across a few hundred validator rounds. Adapted from [JuliusBrussee/caveman](https://github.com/JuliusBrussee/caveman) with credit.

- **A SessionStart hook** that auto-briefs agents that lope is available, so users don't have to remember slash commands.

- **A `using-lope` auto-trigger skill** that recognizes natural language like "plan the auth refactor" and invokes lope on the user's behalf.

- **Paste-a-prompt install.** One line into any AI agent:

  ```
  Read https://raw.githubusercontent.com/traylinx/lope/main/INSTALL.md and follow the instructions to install lope on this machine natively.
  ```

  The agent fetches `INSTALL.md`, follows the steps, reports back. CLI-agnostic — writes skills and commands into each host's native directory in the format that host expects.

- **Zero external Python dependencies.** Pure stdlib. Works on Python 3.9+ without a venv. The engine is around 2,000 lines of readable code.

## What's next

The list for v0.4 and beyond:

- A proper CI integration example so teams can gate PRs on validator consensus
- More domain presets (legal, healthcare, finance) with pre-written role prompts
- A public validator config registry so people can share their custom `providers` blocks
- Better handling of very long sprints where context windows start to matter

If you want to help, the repo is [github.com/traylinx/lope](https://github.com/traylinx/lope). Open an issue, send a PR, or just run a sprint against something you'd otherwise hand-draft this week and tell me what broke.

— Sebastian
