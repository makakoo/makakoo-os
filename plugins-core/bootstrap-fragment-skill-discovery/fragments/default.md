## Capability discovery — always available via `skill_discover`

Every MCP-capable CLI ships with the `skill_discover(query?, limit?)` tool. It walks `$MAKAKOO_HOME/plugins/` for every `SKILL.md` and returns the list (name, category, path, relative_path). Use it **whenever a user asks what you can do, what tools you have, or whether you can X** — the answer has to come from the actual installed tree, not from your memory of a previous install.

**Trigger patterns** — call `skill_discover` first when:

- The user asks *"what can you do?"*, *"what skills do you have?"*, *"list your capabilities"*, or any variant.
- The user asks *"can you X?"* where X is a concrete capability you're uncertain about. Don't guess; walk the tree.
- Before emitting any *"I don't have access to X"* or *"I can't do X"* line — the user may be wrong about what's installed; `skill_discover` settles it in one call.
- Inside a multi-step plan you're about to commit to, if the plan assumes a capability. Verify before committing.
- When you land in a fresh `$MAKAKOO_HOME` and need to orient yourself — a broad `skill_discover(query="")` returns every portable skill on disk.

**Hard rule:** Do **not** say *"I can do X"* or *"X is available"* without first either (a) seeing the tool in your own local tools list, or (b) getting a hit from `skill_discover`. Fabricating capabilities is the single most common agent integration bug. Prefer *"let me check what's available"* + call the tool + report back honestly.

**How to call it** — `skill_discover` takes an optional substring `query` and an optional `limit` (default 50, raise for exhaustive audits):

```python
# Example call payload (LLM-facing JSON input):
{
  "query": "browse",
  "limit": 5
}
# Example output (array of records):
# [
#   {
#     "name": "agent-browser-harness",
#     "category": "plugins",
#     "path": "/Users/.../MAKAKOO/plugins/agent-browser-harness/SKILL.md",
#     "relative_path": "agent-browser-harness/SKILL.md"
#   },
#   ...
# ]
```

Results are **paths**, not bodies. After picking the most relevant hit, `Read` the `SKILL.md` at that path to get the full decision tree, call shapes, and troubleshooting for the underlying tool.

**Discovery depth.** The walk descends up to 6 levels under `$MAKAKOO_HOME/plugins/`. Nested layouts like `plugins/lib-harvey-core/skills/<name>/SKILL.md` are found. Dot-directories are skipped.

**When not to reach for it:**

- You already know the exact tool name AND its call shape AND the user asked for *exactly* that tool. No need to rediscover — call it directly.
- Pure conversational turns with no capability question (*"explain X"* / *"how does Y work"*). `skill_discover` is for *action* discovery, not knowledge retrieval.
