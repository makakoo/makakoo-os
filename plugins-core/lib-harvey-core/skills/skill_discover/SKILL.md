---
name: skill_discover
version: 0.1.0
description: |
  Meta-tool — walk `$MAKAKOO_HOME/plugins/` for every `SKILL.md` file
  and return the list. The one tool every external agent should call
  before claiming a capability. Self-referential: this SKILL.md lives
  in the tree it describes.
allowed-tools:
  - skill_discover
category: meta
tags:
  - meta
  - capability-discovery
  - self-documenting
  - cli-agnostic
  - mcp-tool
---

# skill_discover — find capabilities before claiming them

Any MCP-connected agent sees a long list of tool names via `tools/list`,
but the name alone rarely explains **when** to reach for the tool. Every
tool family with external value ships a `SKILL.md` alongside its
implementation describing the decision tree, call shapes, and edge
cases. `skill_discover` walks the tree and returns every `SKILL.md` it
finds, so the agent can `Read` the top hit before calling the
underlying tool.

## When to reach for skill_discover

**Every time** the user asks:

- *"what can you do?"*
- *"what tools do you have?"*
- *"list your skills"*
- *"can you X?"* (to check without guessing)

And before **any** response shaped like *"I don't have access to X"* —
the user may be wrong, and `skill_discover` settles it in one call.

## Hard rule

Do **not** say *"I can do X"* without first having discovered X via
`skill_discover` or having the capability in your own local tool list.
Fabricating capabilities is the single most common agent integration
bug. Prefer *"let me check what's available"* + call `skill_discover`
+ report back honestly.

## Call shape

```json
{
  "tool": "skill_discover",
  "arguments": {
    "query": "browse",
    "limit": 50
  }
}
```

- `query` — optional substring filter. Matches against the skill
  name, parent-dir category, and relative path (all lowercased). Omit
  for the full list.
- `limit` — max results, default 50. Raise for exhaustive audits.

Returns an array of records:

```json
[
  {
    "name": "browse",
    "category": "agent-browser-harness",
    "path": "/Users/you/MAKAKOO/plugins/agent-browser-harness/SKILL.md",
    "relative_path": "agent-browser-harness/SKILL.md"
  },
  ...
]
```

The caller should then `Read` the `path` of the most relevant hit to
get the full SKILL.md body (decision tree, call shapes, troubleshooting).
Returning *just* the paths — not the bodies — keeps the response
lightweight; the caller picks the one that matters and reads deeper
only when needed.

## How the walk works

`skill_discover`:

1. Roots at `$MAKAKOO_HOME/plugins/`.
2. Walks up to depth 6 (plenty for nested `skills/<name>/SKILL.md`
   layouts inside library plugins like `lib-harvey-core`).
3. Collects every file named `SKILL.md` (case-insensitive).
4. Applies the query filter across `relative_path`, parent dir name
   (becomes `name`), and grandparent dir name (becomes `category`).
5. Skips dot-directories.

The walk is filesystem-only — no DB, no subsystems, no network.
~100ms on a fresh install; a few hundred ms with the full skill tree.

## Portable integration (external agentic apps)

The handler lives in Rust at
`makakoo-mcp/src/handlers/tier_a/skill.rs`. Zero Python deps; walks
the tree directly. External runtimes have two paths:

1. **Connect to `makakoo-mcp`** and call `skill_discover` the same as
   any other MCP tool. Decision tree: discover → read → decide →
   call.
2. **Walk the filesystem yourself**: find every `SKILL.md` under your
   own plugin root, parse the YAML frontmatter, build your own
   registry. Makakoo's v0.1 `SKILL.md` format is
   self-describing enough that external tools can re-implement the
   walk in ~30 lines. See `docs/agents/consuming-makakoo-externally.md`
   for a LangChain example.

## Don't skip the discovery step

An agent that assumes capabilities from a cached tools/list is a
confabulation risk — tools can be added, removed, or replaced between
sessions. `skill_discover` is cheap enough (filesystem walk, no LLM)
that running it on every "what can you do" question is free in
practice. Running it also **teaches** the external agent about the
universe of Makakoo-flavored capability docs, which it can then
include in its own system prompt via `Read`.

## Self-referential hook

This `SKILL.md` lives at
`plugins-core/lib-harvey-core/skills/skill_discover/SKILL.md`. Its
parent dir name is `skill_discover`, so `skill_discover(query="skill_discover")`
returns this exact document — proving the walk reaches nested library
locations. Try it.
