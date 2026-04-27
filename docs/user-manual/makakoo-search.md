# `makakoo search` — CLI reference

`makakoo search` runs a raw FTS5 full-text query over the Brain
(`superbrain.db`) and returns matched snippets with surrounding context.
It is the fastest way to find an exact phrase, a person's name, a project
keyword, or a URL buried somewhere in your journals or pages — no LLM call,
no network, instant results.

For an interpreted, synthesized answer use
[`makakoo query`](makakoo-query.md) instead.

## Flag reference

| Flag | Default | Meaning |
|---|---|---|
| `<QUERY>` | *(required)* | Query text. Use quotes to group phrases. FTS5 operators `AND`, `OR`, `NOT`, and `"..."` phrases are all supported. |
| `-l` / `--limit <N>` | `10` | Maximum hits to return. |

## Key use patterns

### Find a phrase across all journals and pages

```sh
# exact phrase search
makakoo search "polymarket arbitrage"

# limit to 5 results
makakoo search "Traylinx" -l 5
```

### Combine terms with FTS5 operators

```sh
# pages containing both "adapter" and "openai-compat"
makakoo search "adapter AND openai-compat"

# pages mentioning a project but not a noisy term
makakoo search "lope NOT codex"
```

### Grep for a person or entity across all journals

```sh
# find every journal line where a person is mentioned
makakoo search "Igor Varzin"

# find all wikilink references to a project
makakoo search "[[arbitrage-agent]]"
```

## What it searches

- Brain journal files (`data/Brain/journals/`)
- Brain page files (`data/Brain/pages/`)
- Auto-memory (`data/auto-memory/`)

All three are indexed into a single FTS5 table in `superbrain.db`.
The index is updated by `makakoo sync` (runs every 30 minutes via SANCHO,
or immediately when you run `makakoo sync` manually).

## Related commands

- [`makakoo-query.md`](makakoo-query.md) — LLM-synthesized answer from the same index
- [`makakoo-sancho.md`](makakoo-sancho.md) — SANCHO keeps the index fresh via `brain_sync_tick`
- [`../brain/index.md`](../brain/) — what lives in the Brain

## Common gotcha

**`makakoo search` returns zero hits for something you know is in the Brain.**
The FTS5 index is a snapshot that is updated by `makakoo sync`. If you wrote
a journal entry recently (manually or via a tool), it may not be indexed yet.
Run `makakoo sync` to flush new files into FTS5, then retry. If you still get
zero hits, check that `superbrain.db` exists at `$MAKAKOO_HOME/data/superbrain.db`
and is not empty — a missing `$MAKAKOO_HOME` env var causes the binary to look
in the wrong location.
