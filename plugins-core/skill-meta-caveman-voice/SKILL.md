---
name: caveman-voice
version: 0.1.0
description: |
  Output-token compression mode for Harvey's internal communication. Speaks
  terse "smart caveman" dialect (drops articles, filler, hedging; fragments
  OK; code/URLs/paths preserved verbatim) to cut ~65-75% of output tokens on
  internal work — programming, debugging, research, tool orchestration, Brain
  journaling. Automatically BYPASSED for any external writing context
  (polished prose, emails, social posts, published documents) where voice and
  rhythm matter. Default: ACTIVE for internal, OFF for external.
allowed-tools: []
category: meta
tags:
  - token-efficiency
  - output-compression
  - internal-comms
  - cli-agnostic
---

# caveman-voice — Harvey's internal terse mode

Adapted from [JuliusBrussee/caveman](https://github.com/JuliusBrussee/caveman)
(MIT License). See `ACKNOWLEDGEMENTS.md` for credit detail.

This plugin is kind=`bootstrap-fragment` — it ships a fragment (`fragments/default.md`)
that gets spliced into the Bootstrap Block every infected host sees. There is no
runnable code: the "skill" is a prompt shape, loaded via infect, not dispatched.

See `$MAKAKOO_HOME/harvey-os/skills/meta/caveman-voice/SKILL.md` for the full
rulebook — HARD-GATE bypass list, intensity levels, examples, drift-failure
maintenance notes. This file is the slim plugin-local mirror; the source of
truth lives in harvey-os and is what Sebastian edits.

## What this plugin does

1. On `makakoo plugin install skill-meta-caveman-voice`, the fragment at
   `fragments/default.md` is registered as a bootstrap contribution.
2. On `makakoo infect --global`, the fragment is inlined into every CLI
   host's global slot (claude/gemini/codex/opencode/vibe/cursor/qwen).
3. Every session launched in any of those CLIs reads the fragment as part
   of its system prompt, and responds in caveman voice for internal work.

No entrypoint, no venv, no runtime. Pure prompt surgery.

## Hard gates summary (see harvey-os copy for full text)

**BYPASS** caveman voice (use full prose) when:

- External writing context: emails, LinkedIn, papers, blog posts, published docs
- Active skill is an external-facing producer (see full list in harvey-os copy)
- Intent keywords present: "write", "draft", "polish", "post to", "email to", "announce"
- Safety-critical: security warnings, irreversible action confirmations
- User asks for clarification or repeats (previous reply was too terse)

**STAY** in caveman for: programming, debugging, research, tool orchestration,
Brain journaling, plans, TODOs, status updates to Sebastian in terminal.

## Intensity levels

| Level | Behavior |
|-------|----------|
| `lite`  | No filler/hedging. Keep articles + full sentences. |
| `full`  | Drop articles, fragments OK, short synonyms. **Default.** |
| `ultra` | Abbreviate (`DB`/`auth`/`cfg`/`fn`), strip conjunctions, arrows. |

Override via `/caveman-voice lite|full|ultra|off` or natural language
("talk normal", "be terse", "save tokens").

## Token math

~0.9 internal × 0.70 reduction ≈ 63% aggregate output-token savings on
Harvey's workload mix. Cost to install: zero (prompt file). Cost to run:
zero (shapes outputs that were already happening).
