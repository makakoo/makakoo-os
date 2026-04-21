---
name: pi
version: 0.1.0
description: |
  Drive badlogic/pi-mono (the pi CLI) as a first-class subagent —
  one-turn prompts, multi-turn sessions with fork/rewind/label,
  mid-session model swaps, and live steering. Six MCP tools; pi is a
  separate binary the user installs on PATH. Makakoo never vendors pi.
allowed-tools:
  - pi_run
  - pi_session_fork
  - pi_session_label
  - pi_session_export
  - pi_set_model
  - pi_steer
category: ai-ml
tags:
  - subagent
  - multi-turn
  - cli-agnostic
  - mcp-tool
  - external-binary
---

# pi — multi-turn AI sessions with fork, rewind, steer

pi is badlogic/pi-mono, a standalone CLI that stores every turn of
every session as JSONL under `~/.pi/`. It exposes an RPC protocol
over `pi --rpc` that Makakoo wraps as six MCP tools, letting any
MCP-connected agent route code-task-shaped requests to pi as a
subagent while retaining full session sophistication (fork,
label a message, rewind, hot-swap models, inject guidance mid-turn).

The pi binary is **not bundled** with Makakoo. It's an external CLI
the user installs themselves. Every tool fails with a clear "pi not on
PATH" error if the binary is missing. Run
`makakoo plugin health agent-pi` to verify.

## When to reach for pi

Use pi when the user is in an **iterative code loop** that benefits
from session features Makakoo's native LLM client doesn't have:

| Situation | Route through pi? | Why |
|---|---|---|
| One-shot question with no history | ❌ — just use your local LLM | pi setup cost not worth it |
| Multi-turn code refactor / debugging session | ✅ `pi_run` with stable `session_id` | pi stores the turn-by-turn history, can rewind |
| Need to try two approaches without losing the first | ✅ `pi_session_fork` | branches the session, both kept |
| Want to anchor "this was the good state" | ✅ `pi_session_label` | labels survive rewind |
| Model started going off the rails mid-session | ✅ `pi_steer` — inject "stop and reconsider X" | no need to restart |
| Want to swap from cheap Haiku to Opus partway | ✅ `pi_set_model` | takes effect next turn |
| Need a human-readable export of the conversation | ✅ `pi_session_export` | html or markdown |

## `pi_run` — one turn

```json
{
  "tool": "pi_run",
  "arguments": {
    "prompt": "Refactor this function to use async/await",
    "session_id": "refactor-auth-middleware",
    "model": "switchai:ail-compound",
    "timeout_s": 300
  }
}
```

- `prompt` (required) — the user message for this turn.
- `session_id` — stable id. Omit to start a fresh session;
  pi assigns one and returns it.
- `model` — provider-prefixed id (e.g. `switchai:ail-compound`,
  `anthropic:claude-sonnet-4-6`). Defaults to pi's configured default.
- `timeout_s` — per-turn cap, clamped to 1800s.

Returns `{text, usage, frames, session_id}`. `frames` is the raw
pi RPC frame count for debugging; `text` is the assistant reply.

## `pi_session_fork` — non-destructive branch

```json
{
  "tool": "pi_session_fork",
  "arguments": {
    "session_id": "refactor-auth-middleware",
    "from_msg_id": "msg_2026-04-21T13:05:12"
  }
}
```

Creates a new session that shares history up to `from_msg_id` and
diverges from there. The parent session keeps every message intact.
Use when you want to try "what if we did X instead" without losing
the current trajectory.

## `pi_session_label` — anchor a checkpoint

```json
{
  "tool": "pi_session_label",
  "arguments": {
    "session_id": "refactor-auth-middleware",
    "msg_id": "msg_2026-04-21T13:08:45",
    "label": "tests-green"
  }
}
```

Labels are human-readable markers on messages. pi's `rewind to label`
workflow uses these to return to a known-good checkpoint without
counting message ids.

## `pi_session_export` — rendered transcript

```json
{
  "tool": "pi_session_export",
  "arguments": {
    "session_id": "refactor-auth-middleware",
    "format": "md"
  }
}
```

`format` is `html` or `md`. Returns the rendered body and, if pi wrote
a file on disk, the path.

## `pi_set_model` — mid-session swap

```json
{
  "tool": "pi_set_model",
  "arguments": {
    "session_id": "refactor-auth-middleware",
    "provider": "anthropic",
    "model_id": "claude-opus-4-7"
  }
}
```

Takes effect on the next turn. No context is lost — pi re-hydrates the
same history for the new model.

## `pi_steer` — inject mid-turn guidance

```json
{
  "tool": "pi_steer",
  "arguments": {
    "session_id": "refactor-auth-middleware",
    "message": "Stop writing tests. Focus on the actual bug first."
  }
}
```

Useful when the orchestrator (you) spots the subagent going off-rails
and wants to redirect without ending the turn and restarting.

## Prereqs for the agent runtime

1. Install pi. Upstream: [badlogic/pi-mono](https://github.com/badlogic/pi-mono). Typical: `cargo install pi-mono` or a prebuilt binary on PATH.
2. `makakoo plugin install --core agent-pi` (or your framework's equivalent).
3. `makakoo plugin health agent-pi` should report green.
4. Fresh sessions auto-hydrate — no DB setup beyond pi's own `~/.pi/`.

## Portable integration (external agentic apps)

The MCP handlers live in Rust at `makakoo-mcp/src/handlers/tier_b/pi.rs`.
All six tools shell out to `pi --rpc` with JSONL frames over stdin/stdout.
External agent runtimes have two paths:

1. **Connect to `makakoo-mcp`** and call the tools as-is via the MCP
   adapter your framework ships (LangChain, OpenAI Assistants, Cursor
   rules, etc). Zero code change.
2. **Shell out to `pi --rpc` directly** from your own runtime and mirror
   the JSON frame format (see the RPC docstrings in the source for the
   exact shape). The tool-shape above stays identical.

The MCP route is preferred — it inherits Makakoo's timeout clamping
(1800s max), sanitized env, and `pi_steer` safety check that other
runtimes would need to re-implement.

## Don't confuse pi with your own LLM client

pi is a **subagent router**, not a model. When you call `pi_run`, you're
asking another CLI to run a turn and come back with the answer — that
CLI's session, that CLI's tool use, that CLI's cost line. Don't use pi
for stateless one-off completions you could do faster with a direct
provider call. Use it when the *multi-turn history + rewind/fork
features* justify the overhead.

## Attribution

pi-mono is authored and maintained by [badlogic](https://github.com/badlogic) —
not by Makakoo. The `agent-pi` plugin is a thin integration wrapper.
