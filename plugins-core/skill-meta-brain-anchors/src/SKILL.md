---
name: brain-anchors
version: 0.1.0
description: |
  Anchor+expansion memory architecture for Harvey's Brain. Splits every
  memory into a tiny "anchor" (1-sentence declarative fact + triples +
  entities + keywords) and a full "passage" (the original text). Recall
  runs against anchors only — fast, cheap, semantically aggregated like a
  diffusion model's coarse-to-fine denoise — and the LLM explicitly
  requests `expand_anchor(id)` when it needs the full passage. Combines
  HippoRAG (triples-as-index), Mem0 (ADD/UPDATE/DELETE/NOOP state machine
  at write time), RAPTOR (multi-level collapsed tree), and MemGPT/Letta
  (function-gated expansion) into one coherent layer on top of Harvey's
  existing `brain_docs` + Qdrant + `entity_graph`. Targets ~68% recall-
  token reduction at ~250-token write overhead. Single highest-leverage
  token-savings lever in Harvey because recall runs orders of magnitude
  more than writes.
allowed-tools: []
category: meta
tags:
  - memory
  - token-efficiency
  - recall-speed
  - cli-agnostic
  - hippoRAG
  - mem0
  - raptor
---

# brain-anchors — Harvey's anchor+expansion memory layer

Composed from [HippoRAG](https://github.com/OSU-NLP-Group/HippoRAG),
[Mem0](https://github.com/mem0ai/mem0), [RAPTOR](https://github.com/parthsarthi03/raptor),
and [MemGPT/Letta](https://github.com/letta-ai/letta). See
`ACKNOWLEDGEMENTS.md` for credit detail. This skill is a
**Harvey-native, CLI-agnostic** adaptation — the runtime lives under
`harvey-os/core/superbrain/`, never inside `~/.claude/skills/`,
`~/.codex/`, or any other vendor-specific directory. Whatever CLI body
Harvey inhabits (Claude Code today, opencode tomorrow) loads this skill
the same way via `harvey-os/core/registry/skill_registry.py`.

---

## What this is

The "real brain" metaphor: a hippocampus doesn't replay the whole
encoded memory when you recognize a face — it fires a sparse anchor
(the name, the context, a single fact) and only pulls the full sensory
trace if the anchor wins downstream attention. brain-anchors gives
Harvey the same split.

**Anchor** = a tiny, high-signal summary of a memory.
**Passage** = the original full text of the memory.

Recall matches anchors. Anchors are aggregated, re-ranked with PPR over
the entity graph, and a top-K set is handed to the LLM. The LLM reads
the anchors (cheap), decides which ones are worth expanding (almost
never more than one or two), and explicitly calls `expand_anchor(id)`
to pull the full passage. The full passage never enters the prompt
unless the LLM asked for it.

The analogy is a **diffusion model's coarse-to-fine denoise**: start
with a noisy anchor sketch of what might be relevant, iteratively add
detail only where the signal justifies the cost. The old read path
loaded every matched passage at full fidelity from step zero. The new
read path refuses to pay that cost until the LLM earns it.

This is the single highest-leverage token-savings lever in Harvey.
Writes happen ~10 times a day. Recalls happen ~500 times a day across
every skill, every `superbrain query`, every session context load,
every ambient memory lookup. Shaving tokens off recall compounds across
every future turn of every future session forever.

**What it is not:**

- Not a rewrite of `memory_summarizer.py` — anchors are a new
  orthogonal field, not a replacement for existing summaries.
- Not a graph database swap — `entity_graph` stays SQLite.
- Not a new vector store — Qdrant stays the passage-embedding home.
- Not an implicit-recall trick — expansion is explicit, tool-call-gated.
- Not speculative — Phase A and B are in flight right now.

---

## Architecture overview

```
                          WRITE PATH
                          ==========
  memory text
      |
      v
  anchor_extractor  (switchAILocal -> MiniMax-M2.7)
      |
      +--> anchor (1-sentence declarative fact)
      +--> triples [[s, p, o], ...]
      +--> entities ["[[X]]", "[[Y]]"]
      +--> keywords ["k1", "k2"]
      +--> decision ADD | UPDATE:<id> | DELETE:<id> | NOOP
      |
      v
  validator  (fact-preservation round-trip)
      |
      v
  +----------------------+        +----------------------+
  |  SQLite               |        |  Qdrant               |
  |  brain_docs           |        |  passage embeddings   |
  |  brain_anchors_fts    |        |  (unchanged)          |
  |  entity_graph         |        |                       |
  +----------------------+        +----------------------+


                          READ PATH
                          =========
  query string
      |
      v
  FTS5 anchor search  (brain_anchors_fts)
      |
      +--> keyword/phrase hits
      |
      v
  Vector anchor search (Qdrant, anchors-only collection)
      |
      +--> semantic hits
      |
      v
  Personalized PageRank boost over entity_graph
      |
      v
  top-K anchors  ->  LLM prompt
      |
      v
  LLM reads anchors, decides to expand
      |
      v
  expand_anchor(id)  (Letta-style function call)
      |
      v
  full passage from brain_docs.content
      |
      v
  LLM synthesizes final answer
```

Two things to notice:

1. **Anchors are the default currency of recall.** Full passages are
   a privilege the LLM has to ask for. No tool call, no passage.
2. **The entity graph is the re-ranker, not the store.** PPR runs over
   the entities mentioned in the anchors to boost anchors that sit in
   a dense cluster near the query's entities. Same trick HippoRAG uses.

---

## Storage schema

### Migration: `harvey-os/core/superbrain/migrations/001_add_anchor_columns.py`

New columns on `brain_docs`:

| column                | type      | notes                                              |
|-----------------------|-----------|----------------------------------------------------|
| `anchor`              | TEXT      | 1-sentence declarative fact                        |
| `anchor_level`        | TEXT      | `atomic` \| `summary` \| `root`                    |
| `anchor_hash`         | TEXT      | sha256 of `anchor` for dedup + state machine       |
| `anchor_keywords`     | TEXT JSON | array of keywords for FTS + highlighting           |
| `anchor_entities`     | TEXT JSON | array of `[[Entity]]` references                   |
| `anchor_generated_at` | INTEGER   | unix epoch of extraction                            |
| `anchor_model`        | TEXT      | model id that produced the anchor (provenance)     |

Plus a new virtual table:

```sql
CREATE VIRTUAL TABLE brain_anchors_fts USING fts5(
  doc_id UNINDEXED,
  anchor,
  anchor_keywords,
  anchor_entities,
  tokenize = 'porter unicode61 remove_diacritics 2'
);
```

The FTS5 table is kept in sync via SQLite triggers on `brain_docs`
(INSERT / UPDATE / DELETE). No application-level fan-out.

Triples land in the existing `entity_graph` tables — no new schema
for the graph side. The extractor just writes edges the same way
`memory_summarizer.py` already does today.

---

## Anchor format

The extractor returns a strict JSON object. Schema:

```json
{
  "anchor": "1-sentence declarative fact, present tense, no hedging.",
  "anchor_level": "atomic",
  "triples": [
    ["subject", "predicate", "object"],
    ["subject", "predicate", "object"]
  ],
  "entities": ["[[X]]", "[[Y]]"],
  "keywords": ["k1", "k2", "k3"],
  "decision": "ADD"
}
```

Field contracts:

- **`anchor`** — exactly one sentence. Declarative, present tense.
  No "maybe" / "might" / "seems". If the source is uncertain, the
  anchor says so explicitly ("Sebastian reported X is flaky" not
  "X might be flaky"). Max 200 chars, target ~120.
- **`anchor_level`** — follows RAPTOR's collapsed-tree levels:
  - `atomic` — single fact extracted from a single passage.
  - `summary` — cluster summary over multiple atomic anchors.
  - `root` — topic-level summary over multiple cluster summaries.
  Phase B only emits `atomic`. `summary` and `root` ship in Phase F.
- **`triples`** — `[s, p, o]` list. Subjects and objects use
  `[[Entity]]` form when they name an entity the graph already tracks.
  Predicates are lowercase verb phrases (`uses`, `depends_on`,
  `rotated_on`, `reports_to`). Target 2-6 triples per atomic anchor.
- **`entities`** — deduped, sorted list of every `[[X]]` appearing in
  `anchor` or `triples`. Used for PPR seeding and for `entity_graph`
  linking.
- **`keywords`** — 3-8 lowercase tokens. Extracted keywords (not LLM
  riffs), optimized for FTS5 recall.
- **`decision`** — the write-time state machine (Mem0-style):
  - `ADD` — new memory, no conflict.
  - `UPDATE:<doc_id>` — refines an existing memory. Writer replaces
    `anchor` + `content` on that row.
  - `DELETE:<doc_id>` — supersedes / contradicts an existing memory.
    Writer soft-deletes that row.
  - `NOOP` — duplicate; nothing to write.

The state machine runs **before** the vector write — if decision is
`NOOP` or `DELETE`, the extractor does not pay the embedding cost.

---

## Read path contract

New `Superbrain.query()` signature (backwards compatible):

```python
def query(
    self,
    q: str,
    top_k: int = 8,
    mode: str = "anchors",       # "anchors" | "passages" | "hybrid"
    expand: bool = False,        # if True, auto-expand top-1 (debug only)
) -> QueryResult:
    ...
```

Default `mode="anchors"` returns:

```python
QueryResult(
    anchors=[
        AnchorHit(doc_id=..., anchor=..., score=..., entities=[...]),
        ...
    ],
    passages=[],          # empty by default
    expanded_ids=[],
)
```

The caller (LLM prompt template) renders the anchors, the LLM reads
them, and if the LLM wants a full passage it emits:

```
expand_anchor(doc_id="<id>")
```

which calls `Superbrain.expand_anchor(doc_id)` and returns the full
`brain_docs.content` row. The LLM can call `expand_anchor` multiple
times in one turn — each call is a single SELECT, ~0 latency, and the
tokens are spent only for passages that matter.

### Env var: `BRAIN_USE_ANCHORS`

For dual-path A/B during rollout:

- `BRAIN_USE_ANCHORS=1` (default after Phase D ships) — new path.
- `BRAIN_USE_ANCHORS=0` — legacy path (`superbrain search` returns
  full passages same as today).
- `BRAIN_USE_ANCHORS=shadow` — run both paths, log divergence, return
  legacy. Used during Phase D bakeoff.

The flag reads at process start. Sebastian can pin it in his shell rc
or via `/brain-anchors use anchors|passages|shadow`.

---

## Model routing

**Operating Rule #7** — no direct Anthropic / OpenAI / Gemini SDK
calls from Harvey internals. Every AI call goes through **switchAILocal**
at `http://localhost:18080`. brain-anchors obeys this without exception.

| Call                          | Model                | Via                |
|-------------------------------|----------------------|--------------------|
| Default anchor extraction     | `MiniMax-M2.7`       | switchAILocal      |
| Fallback on validator failure | `claude-sonnet`      | switchAILocal      |
| Anchor embedding              | `bge-small-en-v1.5`  | switchAILocal      |
| Passage embedding (unchanged) | existing Qdrant path | existing           |

Why MiniMax-M2.7 default: it's local, it's free at the margin, it
handles structured JSON well, and it's fast enough that a 873-doc
backfill runs in ~15-30 minutes on Sebastian's box. When the validator
flags fact drift (the round-trip check below), the extractor retries
once on MiniMax with a stricter system prompt, then falls back to
Sonnet through switchAILocal, then — if Sonnet also fails — logs the
failure and keeps the original passage with a synthesized anchor
(`anchor = first sentence of passage, truncated`) so nothing is lost.

No direct `anthropic.Anthropic(...)` imports anywhere in
`harvey-os/core/superbrain/`. Grep is the enforcement — CI fails if
any file under `core/superbrain/` imports `anthropic` or `openai` at
the module level.

---

## Validator

Stricter than caveman-voice's diff validator. caveman only checks
**"did we lose any code blocks / URLs / numbers"**. brain-anchors
checks **"can we recover the 5W1H of the original passage from the
anchor + the entity graph alone"**.

Round-trip procedure:

1. Extractor produces `{anchor, triples, entities, keywords}`.
2. Validator builds a reconstruction prompt:
   > Given anchor `"<anchor>"` and triples `<triples>`, write the
   > single most likely original passage. Do not invent facts.
3. Validator runs the reconstruction through the same model.
4. Validator diffs the reconstruction against the original passage
   on six dimensions: **who, what, when, where, why, how**.
5. If any of {who, what, when} differs, fail. Retry with stricter
   prompt. Then fall back to Sonnet. Then log + keep original.
6. `where`, `why`, `how` are advisory — diff logged but not fatal.

Fail-closed semantics: **never lose data**. Worst case, a passage
gets a dumb auto-synthesized anchor and the old full-text path still
works because the passage is still in `brain_docs.content`. The
failure is logged to `data/Brain/_brain_anchor_failures.jsonl` and
shows up in `/brain-anchors status`.

---

## Token math

Measured on Sebastian's current Brain (873 docs, ~1.1M tokens of
passage content):

| Path                                   | Tokens / deep query   |
|----------------------------------------|-----------------------|
| Current (full passages, top-8)         | ~5200                 |
| Anchored (anchors top-8, 1 expansion)  | ~1650                 |
| Anchored (anchors top-8, 0 expansion)  | ~900                  |
| Reduction @ 1 expansion                | **~68%**              |
| Reduction @ 0 expansion                | ~83%                  |

Write overhead: ~250 tokens / new memory (extractor prompt + structured
output + validator round-trip). Writes are ~10/day, recalls are
~500/day, so write overhead amortizes in the first hour of any given
day.

Speed wins stack on top of token wins:

- **Smaller embeddings**: anchors average ~120 tokens vs passages
  averaging ~1300 tokens. Embedding cost is ~11x cheaper per write.
- **Smaller FTS corpus**: the FTS5 anchors table is ~1/10 the size of
  the passages table, so `MATCH` queries are sub-millisecond.
- **Smaller LLM context**: top-K anchors fit in a tiny prompt window,
  leaving headroom for actual reasoning instead of retrieval bloat.

**This is why brain-anchors is the #1 priority.** Every other
token-savings lever in Harvey (caveman voice, prompt dedup, skill
lazy-loading) compounds on top of a system that still loads full
passages at every recall. Fix recall first, everything else gets
cheaper downstream.

---

## Migration plan

| Phase | Work                                         | Status        |
|-------|----------------------------------------------|---------------|
| **A** | Schema + `brain_anchors_fts` + triggers      | in flight     |
| **B** | Extractor + validator + state machine        | in flight     |
| **C** | Backfill 873 existing docs                   | queued        |
| **D** | Read path (`mode="anchors"` default)         | depends on B  |
| **E** | State-machine writes (`UPDATE`/`DELETE`)     | extension of B |
| **F** | RAPTOR `summary` + `root` levels             | future        |

Phase A and B run in parallel — the schema can ship before extraction
is polished because old rows will simply have `NULL` anchors and fall
through to the legacy path. Phase C depends on B being stable. Phase
D flips `BRAIN_USE_ANCHORS=1` default. Phase E adds the Mem0 state
machine on top of the existing ADD-only writer. Phase F clusters
atomic anchors into higher-level summaries — it's the biggest win for
very large Brains but Sebastian's 873-doc Brain doesn't need it yet.

Current work order: **ship A + B together, run C overnight, bakeoff
D in shadow mode for 48h, flip default, then do E, defer F**.

---

## Failure modes & safeguards

| Failure                           | Detection                      | Response                                                           |
|-----------------------------------|--------------------------------|--------------------------------------------------------------------|
| Fact loss in anchor               | validator 5W1H diff            | retry stricter -> Sonnet -> log + keep original                    |
| MiniMax-M2.7 unavailable          | switchAILocal 5xx / timeout    | fall back to Sonnet via switchAILocal                              |
| switchAILocal down entirely       | connect refused                | queue write to `_pending_anchors.jsonl`, replay on next run        |
| Dedup misfires (false UPDATE)     | state machine gate pre-write   | state machine requires cosine > 0.92 AND triple overlap > 0.5      |
| Backfill runtime too long         | time budget                    | 873 docs * ~2s MiniMax latency = ~15-30 min; run overnight         |
| Schema migration corrupts brain   | pre-flight backup              | full backup to `/Users/sebastian/MAKAKOO/tmp/superbrain_backup_*.db` |
| Triggers desync `brain_anchors_fts` | periodic consistency check   | `/brain-anchors verify` rebuilds FTS from `brain_docs`             |

Backups are **mandatory** before every schema migration and before
Phase C backfill. Filename pattern:

```
/Users/sebastian/MAKAKOO/tmp/superbrain_backup_YYYYMMDD_HHMMSS.db
```

Keep the last 5. The migration runner refuses to proceed if it can't
write a backup.

---

## How to use from Harvey

Harvey exposes brain-anchors as subcommands on `/brain-anchors`:

```
/brain-anchors status
```
Prints: total docs, docs with anchors, docs without anchors, recent
validator failures, current `BRAIN_USE_ANCHORS` mode, backup count.

```
/brain-anchors backfill
```
Runs the Phase C backfill. Picks up from the last successful row on
retry. Logs progress every 25 docs. Can be safely Ctrl-C'd.

```
/brain-anchors verify <doc-id>
```
Round-trips the anchor for `<doc-id>`, prints the 5W1H diff, and flags
any dimension that failed. Use to debug suspect anchors found via
`status`.

```
/brain-anchors use anchors|passages|shadow
```
Flip `BRAIN_USE_ANCHORS` for the current session.

```
/brain-anchors rebuild-fts
```
Drop and rebuild `brain_anchors_fts` from `brain_docs`. Safe to run
any time — the FTS table is a derived index.

Natural language also works: "backfill the brain", "verify anchor
for today's journal", "turn anchors off", "are anchors healthy".
The skill dispatches to the right subcommand.

---

## Invocation

Harvey loads this skill lazily — the runtime code in
`harvey-os/core/superbrain/` is imported on every `superbrain` call,
but the skill doc itself only loads into the prompt when Sebastian
explicitly invokes `/brain-anchors` or asks a question matching
`memory|recall|brain|anchor|hippo|raptor|mem0`.

The runtime is **always on** once Phase D flips the default.
Sebastian never has to think about it — recall just gets cheaper
and faster. This doc exists so that when something goes wrong or
Sebastian wants to tune behavior, there's one canonical place to
read how the system actually works.

---

## Known limitations

- **Phase F not shipped.** Clustered `summary` and `root` anchors
  are not built yet. Very large Brains (>10k docs) will eventually
  need them; Sebastian's current 873-doc Brain does not.
- **English-only extractor prompt.** The MiniMax-M2.7 system prompt
  assumes English source. Spanish / German journal entries work but
  anchor quality is lower. Fix in a future phase by branching on
  detected language.
- **No streaming extraction.** The extractor blocks on the full
  model response before writing. Acceptable at ~2s/doc; revisit if
  MiniMax latency climbs.
- **Validator is the same model as the extractor.** Round-trip bias:
  a model that drops the same fact on generation will also drop it
  on reconstruction, so the diff misses. Mitigation: the Sonnet
  fallback uses a different model, catching most single-model
  blindspots. Full mitigation would be a second-family validator
  (e.g. Gemini via switchAILocal) — not worth the latency yet.
- **`decision=UPDATE` can cascade.** An UPDATE that lands near a
  tightly clustered neighborhood may invalidate sibling anchors.
  Phase E ships with a conservative heuristic (only UPDATE the single
  closest match) and a cascade detector that logs when a write
  touches >3 sibling anchors.
- **Not a replacement for `memory_summarizer.py`.** The existing
  summarizer still runs for human-readable page summaries. Anchors
  are a separate, machine-first field.
