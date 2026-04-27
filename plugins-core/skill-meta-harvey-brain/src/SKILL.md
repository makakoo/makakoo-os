---
name: harvey-brain
description: Use when the user asks about their own past notes, decisions, history, or anything that should already be in their persistent memory. Trigger phrases include "do I have notes on X", "what did I decide about Y", "check my brain for Z", "remember when I worked on W", "what do I know about V", "what did I write last week about U". Routes the question through Harvey's Brain (Logseq journals + pages) via mcp__harvey__harvey_brain_search for raw hits or mcp__harvey__harvey_superbrain_query for synthesized answers with sources. Also use this skill to LOG something the user explicitly asks to remember (mcp__harvey__harvey_journal_entry).
---

# Harvey Brain — Persistent Memory

Harvey has a persistent Logseq-backed memory at `~/MAKAKOO/data/Brain/`.
Every meaningful past conversation, decision, person, project, and
discovery is in there. Before saying "I don't know" about anything
the user has touched before, search the brain. Before answering from
your training data when the user is asking about *their* world,
search the brain.

## Trigger phrases

**Memory queries** (read):
- "do I have notes on X"
- "what did I decide about Y"
- "check my brain for Z"
- "remember when…"
- "what did I work on last week"
- "what do I know about <person/project/topic>"
- "did I write anything about X"
- "search my notes for X"

**Memory writes** (the user wants to remember something):
- "remember that X"
- "log this: …"
- "save to my brain: …"
- "make a note that X"
- "for the record, X"

## Read path — three tools, picked by depth

### `mcp__harvey__harvey_brain_search` — fast keyword scan

Use when the user asks for raw hits or you just need to see if a topic
exists. Returns top-k matches with snippets, no LLM synthesis. Cheap.

```
mcp__harvey__harvey_brain_search({
  query: "<topic>",
  top_k: 5
})
```

### `mcp__harvey__harvey_superbrain_query` — synthesized answer

Use when the user asks an actual question they want an *answer* to,
not a list of hits. Combines FTS + vector search + entity graph + LLM
synthesis. Returns a written answer with sources.

```
mcp__harvey__harvey_superbrain_query({
  question: "<the user's exact question>"
})
```

This is the right call for *"what did I decide about the auth
middleware?"* — you want an answer, not 8 raw journal hits.

### `mcp__harvey__harvey_superbrain_vector_search` — semantic only

Use when keyword search misses but you suspect there's a semantically
related note. Slower than `harvey_brain_search` but catches paraphrases.

```
mcp__harvey__harvey_superbrain_vector_search({
  query: "<topic>",
  k: 5
})
```

## Decision tree

1. **User wants a written answer to a question?** → `harvey_superbrain_query`
2. **User wants to see raw notes/hits?** → `harvey_brain_search`
3. **Keyword search returned nothing but you think the topic exists?** → `harvey_superbrain_vector_search`
4. **User said "remember…" or "log…"?** → `harvey_journal_entry` (skip to write path below)

## Write path

When the user explicitly asks to remember something, log it to today's
journal with entity tags so future searches find it:

```
mcp__harvey__harvey_journal_entry({
  summary: "<one-line summary of what to remember>",
  tags: ["<EntityName>", "<Topic>"]
})
```

**Tag rules:**
- Always include any people, projects, companies, technologies named
  in the summary as tags. Tags become `[[wikilinks]]` in Logseq, which
  is how the knowledge graph builds relationships.
- Use TitleCase for entity tags (`HarveyOS`, `DiffusionTransformers`),
  not lowercase or with spaces.
- 2-4 tags is the sweet spot. More than 6 dilutes the graph signal.

## Important behaviors

- **Never claim ignorance about the user's own past without searching
  first.** If they say "what did I decide about X" and you haven't
  called a brain tool yet, you're guessing.
- **Quote the source** when you answer from brain hits. The user
  wants to know which journal/page it came from.
- **Don't invent journal entries.** If `harvey_brain_search` returns
  nothing, say "no hits in the Brain for that — want me to log it
  now?" — don't make up a memory.

## When NOT to use this skill

- The user asked about a public fact unrelated to their world
  ("when did WW2 end") — answer directly.
- The user wants new research, not a memory query — use the
  `harvey-swarm` skill instead.
- The user gave a precise tool-call instruction
  ("use harvey_brain_search for X") — honor it directly.
