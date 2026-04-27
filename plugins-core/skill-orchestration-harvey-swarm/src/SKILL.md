---
name: harvey-swarm
description: Use when the user asks to research, investigate, compare, find sources, dig deeper, or otherwise needs more than one perspective on a topic. Routes the request through Harvey's IntelligentRouter, which classifies the intent (research / creative / archive / minimal), composes a team of specialized subagents (researcher × N → synthesizer → storage), and runs the DAG in parallel via mcp__harvey__harvey_swarm_run. Use this instead of doing the research yourself when the user wants depth or multiple sources.
---

# Harvey Swarm — Multi-Agent Research

When the user wants more than a single-shot answer — when they say
"investigate," "research," "look into," "compare," "find me sources for,"
"do a deep dive," "check multiple angles" — that's a swarm job. Don't
try to do the work yourself in one prompt. Hand it off to Harvey's
swarm so the user gets parallel researchers + a synthesis pass + a
permanent storage record.

## Trigger phrases (non-exhaustive)

- "research X"
- "investigate X"
- "look into X"
- "compare X and Y"
- "find sources on X"
- "do a deep dive on X"
- "what are the latest developments in X"
- "give me a comprehensive view of X"
- "thoroughly analyze X"

If you see scale-hint words like *thorough*, *deep*, *comprehensive*,
*exhaustive*, or *across multiple sources*, the router will scale the
team up automatically — you don't need to override `parallelism`.

## How to call it

Always go through `mcp__harvey__harvey_swarm_run`. Required arg:
`request`. Optional: `parallelism` (override team size), `timeout_s`
(default 120), `plan_only` (return the DAG without running it).

### Step 1 — usually: peek at the plan first

If the request is ambiguous or expensive, call once with `plan_only:
true` so the user sees what the swarm will do before you commit
wall-clock time:

```
mcp__harvey__harvey_swarm_run({
  request: "<the user's exact ask>",
  plan_only: true
})
```

That returns the classification (intent + confidence + keywords),
the team roster (which subagents, parallelism, roles), and the
workflow DAG. Share the plan in 2-3 lines so the user can redirect
if needed.

### Step 2 — execute

```
mcp__harvey__harvey_swarm_run({
  request: "<the user's exact ask>",
  timeout_s: 180
})
```

The result is JSON with `workflow_id`, `workflow_state`, per-step
states, and an `artifacts` map. Each artifact is the output of one
researcher / synthesizer / storage step.

### Step 3 — celebrate (optional but on-brand)

When the swarm finishes successfully, hand the mic to Olibia 🦉:

```
mcp__harvey__harvey_olibia_speak({
  message: "swarm research on <topic> complete",
  tone: "celebrate"
})
```

### Step 4 — log it (mandatory)

Every meaningful research run gets a journal line so it's discoverable
later:

```
mcp__harvey__harvey_journal_entry({
  summary: "Ran Harvey swarm on <topic> — <one-line takeaway>",
  tags: ["<EntityName>", "Research"]
})
```

## Common failure modes

- **`mode: "timeout"`** — wall time exceeded `timeout_s`. Either
  bump it (try 240 or 300) or set `plan_only: true` to inspect what
  was running. Often means a single researcher hit a slow tool.
- **`mode: "error"`** — something raised inside the DAG. The
  `error` field has the exception. Check `harvey_swarm_status` for
  circuit breaker state if it's a repeated failure on one agent.
- **Empty or thin results** — usually means the router classified
  the intent wrong. Try rephrasing the `request` with more
  research-y verbs, or override `parallelism` upward.

## When NOT to use this skill

- The user asked a one-shot factual question with a clear answer
  ("what's the capital of France"). Just answer.
- The user asked about their own past notes or decisions. Use
  `harvey-brain` instead — that's a memory query, not research.
- The user gave a precise tool-call instruction
  ("use harvey_brain_search to find …"). Honor it directly without
  routing through the swarm.

## Reference

`references/example-research-flow.md` walks through a full
investigate-and-store run end-to-end.
