# `makakoo query` — CLI reference

`makakoo query` is the high-level Brain interface: it runs full-text + vector
search over your Brain (journals, pages, auto-memory), assembles the top hits
into context, and asks the configured LLM to synthesize a short answer with
citations. Use it when you want an interpreted answer rather than raw snippets
— "what did I decide about X?" rather than "show me lines containing X".

For raw hits without an LLM call, use [`makakoo search`](makakoo-search.md).

## Flag reference

| Flag | Default | Meaning |
|---|---|---|
| `<QUESTION>` | *(required)* | Natural-language question. |
| `--top-k <N>` | `5` | Number of retrieved hits to pass to the LLM. Increase for broader context; higher values cost more tokens. |
| `--model <name>` | `ail-compound` | Override the LLM model name for this call. |
| `-v` / `--show-memory` | off | Print the assembled L0+L1+L2 memory block before the LLM answer. Useful when debugging why an answer is missing context. |

## Key use patterns

### Recall a past decision

```sh
makakoo query "what did I decide about the database migration?"
# returns a synthesized answer citing the exact journal entries
```

### Debug missing context

```sh
# --show-memory prints the full context block the LLM receives
# check whether your journal entries are actually being retrieved
makakoo query "Polymarket arbitrage strategy" --show-memory
```

### Use a different model for a complex synthesis

```sh
# route to a more capable model when the default answer is too shallow
makakoo query "summarize all career decisions from last month" \
  --model ail-compound \
  --top-k 10
```

## How it works

1. FTS5 keyword search over `superbrain.db` (journals + pages + auto-memory).
2. Vector similarity search (when embeddings are present) with the same query.
3. Top-k hits merged, deduplicated, and ranked.
4. LLM prompt assembled with L0 (persona) + L1 (session context) + L2 (hits).
5. LLM response streamed to stdout, followed by a source citations block.

## Related commands

- [`makakoo-search.md`](makakoo-search.md) — raw FTS5 hits without an LLM call
- [`makakoo-sancho.md`](makakoo-sancho.md) — SANCHO runs periodic Brain sync tasks
- [`../brain/index.md`](../brain/) — the Brain structure that query reads from
- [`../concepts/architecture.md`](../concepts/architecture.md) — memory layers (L0/L1/L2)

## Common gotcha

**`makakoo query` returns "I don't have information about that" even though
you wrote it to the Brain recently.**
The FTS5 index is updated by `makakoo sync`, which runs on a SANCHO schedule
(every 30 minutes by default) or immediately after writing via the
`--file` flag. If you just wrote a journal entry manually, run
`makakoo sync` once to flush it into the index, then retry the query.
