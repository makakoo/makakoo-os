# Swarm in Action — a real v0.6 adapter-bus session

This doc captures one real use of the Makakoo v0.6 adapter bus, not a
contrived example. Three adapters. One decision to make. One bridge
carrying three different transport shapes.

**Captured:** 2026-04-21, immediately after `sprint-v0.6-agentic-plug-complete`.

## Setup

Three adapters in the registry after v0.6 Phase A install:

```text
┌───────────────┬─────────────────────────────────────────────┬─────────────────────────────────┐
│ name          │ transport                                   │ source                           │
├───────────────┼─────────────────────────────────────────────┼─────────────────────────────────┤
│ pi            │ subprocess ["pi","-p","--no-session",…]     │ pi-mono → switchAILocal → LLM    │
│ switchailocal │ openai-compatible @ 127.0.0.1:18080/v1      │ direct to MiniMax M2.7 gateway   │
│ tytus-cli     │ mcp-stdio ["tytus-mcp"]                     │ MCP fan-out to 7 Tytus tools     │
└───────────────┴─────────────────────────────────────────────┴─────────────────────────────────┘
```

Three transport kinds: `subprocess`, `openai-compatible`, `mcp-stdio`.
All driven by the same `makakoo adapter call <name>` CLI. All reachable
from lope as validator tier 4 (auto-resolved by name).

## The question

Not synthetic — this is the actual next-sprint decision posed right
after v0.6 shipped:

> Makakoo OS v0.6 just shipped (adapter bus + peer HTTP transport).
> For v0.7, pick ONE: (A) tool-use forwarding so delegate adapters
> can call Harvey MCP tools back (closes multi-agent collaboration
> loop), or (B) flip the makakoo-os repo public (unlocks marketing
> + external contributors). Answer: one letter, then two sentences
> why.

## The calls

Three independent invocations. Same prompt (or envelope), three
different backends, one CLI shape:

```bash
makakoo adapter call switchailocal --prompt "$QUESTION"
makakoo adapter call pi            --prompt "$QUESTION"
makakoo adapter call tytus-cli     --prompt '{"tool":"tytus_status","arguments":{}}'
```

### switchailocal (MiniMax M2.7 via the local gateway)

```text
A. Tool-use forwarding lets delegate adapters call Harvey MCP tools,
closing the multi-agent collaboration loop and unlocking richer,
coordinated workflows. Keeping the repo private now preserves focus
on core functionality before opening up the codebase for broader
contribution.
```

### pi (Claude Sonnet via pi-mono → switchAILocal)

```text
**A** — tool-use forwarding is the architectural missing link that
makes the adapter bus a true peer network rather than a star topology,
and it directly compounds the v0.6 transport work without needing a
social/marketing event to validate the code.
```

### tytus-cli (asking a different kind of question — state, not opinion)

Routed via the v0.6 JSON-envelope convention — one adapter, N tools:

```text
Not logged in. User needs to run: tytus login
```

This isn't an opinion — `tytus_status` returns system state. The
demo deliberately mixes adapter kinds to show the bus carries both
conversational and structured-tool responses through the same
`adapter call` interface.

## Verdict

Both LLM adapters voted **A**. Rationales converged on "tool-use
forwarding compounds v0.6's transport work rather than needing a
marketing moment to validate the code." Tytus-cli contributed
infrastructure state (user not logged in, so tytus-chat adapter
unavailable) — the kind of context a real swarm-orchestrator layer
would use to prune unreachable validators.

That's the v0.6 adapter-bus promise in one session: heterogeneous
backends, one CLI, one output schema (`ValidatorResult`), real answers
to real questions on real infrastructure.

## What this proves

1. **Three transport kinds, zero glue.** Subprocess, HTTP, MCP-stdio
   all worked through the same `adapter call` call path. Nothing
   was special-cased per adapter.
2. **Envelope routing does its job.** `tytus-cli` is a fan-out
   adapter — one `adapter.toml`, 7 underlying tools. The
   `{"tool":"…","arguments":{…}}` envelope picks the tool at call
   time. No per-tool manifest required.
3. **Real responses, not mocks.** Sub-5-second round trips against
   live services. The bridge isn't speculative infrastructure; it
   carries actual workloads today.

## What this motivates for v0.7

The two LLM adapters independently chose **A** (tool-use forwarding)
over **B** (public-repo flip). The rationale both gave — that v0.6's
peer transport is half-useful until delegates can call back — is
the load-bearing observation. Deferred from v0.6 per SPRINT.md §12.
v0.7 now has a clear theme.

## Reproducing this session

```bash
# 1. Install the three v0.6 bundled adapters into the local registry.
makakoo adapter install pi            --bundled --allow-unsigned --skip-health-check
makakoo adapter install switchailocal --bundled --allow-unsigned --skip-health-check
makakoo adapter install tytus-cli     --bundled --allow-unsigned --skip-health-check

# 2. Confirm they're wired.
makakoo adapter list

# 3. Pose a question. Anything.
export Q="Your question here."
makakoo adapter call switchailocal --prompt "$Q"
makakoo adapter call pi            --prompt "$Q"
makakoo adapter call tytus-cli     --prompt '{"tool":"tytus_status","arguments":{}}'
```

Prerequisites:
- `pi` binary on PATH (`npm i -g @mariozechner/pi-ai`).
- `tytus-mcp` binary on PATH (ships with tytus v0.4+).
- SwitchAILocal gateway running on 127.0.0.1:18080 with `AIL_API_KEY`
  set.

## Next

- [bring-your-own-agent.md](bring-your-own-agent.md) — how to add
  your own adapter to this registry in under 60 seconds.
- [consuming-makakoo-externally.md](consuming-makakoo-externally.md)
  — the reverse direction: external agentic apps (LangChain, OpenAI
  Assistants, Cursor) consuming Makakoo skills.
