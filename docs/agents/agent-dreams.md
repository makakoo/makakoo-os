# `agent-dreams`

**Summary:** Memory consolidation engine — runs nightly to strengthen the Brain knowledge graph.
**Kind:** SANCHO task (plugin) · **Language:** Python · **Source:** `plugins-core/agent-dreams/`

> **Naming caveat:** `agent-dreams` lives in the `agent-*` plugin namespace but its `plugin.toml` declares `kind = "sancho-task"`, not `kind = "agent"`. Functionally it is a scheduled consolidation pass, not a long-lived daemon. Kept in the `agent-*` namespace for historical reasons and because it complements the other Brain agents.

## When to use

Left alone. `agent-dreams` fires on its own SANCHO schedule — typically nightly in low-activity hours — and runs the consolidation pass that:

- Promotes frequently-recalled facts into auto-memory.
- Prunes dead wikilinks.
- Strengthens the knowledge graph edges the retrieval layer uses.

You invoke it manually only when you want to force a consolidation pass before that evening's usual run:

```sh
makakoo dream
```

Expected output (abbreviated):

```text
dream: consolidating ~/MAKAKOO/data/Brain/ ...
  pages scanned:       868
  journals scanned:    46
  promoter candidates: 3
  graph edges added:   12
✓ done in 2.3s
```

## Start / stop

Not a daemon — fires from SANCHO. Toggle via:

```sh
makakoo plugin disable agent-dreams
makakoo plugin enable agent-dreams
```

Disable if you want to run your own consolidation schedule manually.

## Where it writes

- **Journal breadcrumb:** one `- [[Dreams]] ...` line per consolidation pass in today's Brain journal.
- **Graph updates:** `~/MAKAKOO/data/makakoo.db` (shared with `makakoo sync`).
- **Logs:** captured in SANCHO output, viewable with `makakoo daemon logs`.

## Health signals

- `makakoo sancho status` shows the task registered.
- `grep "\[\[Dreams\]\]" ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md` — at least one line if dreams ran today.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `makakoo dream` errors with `no brain found` | `$MAKAKOO_HOME/data/Brain/` doesn't exist yet | Run `makakoo install` first, then add at least one journal entry (walkthrough 02). |
| Dreams runs but journal shows no breadcrumb | Plugin bundled stale code (journal write skipped) | Reinstall: `makakoo plugin install --core agent-dreams`. |

## Capability surface

- `fs/read` + `fs/write` — Brain dir.
- `llm/chat` — summarization of grouped entries (optional; degrades gracefully without).

## Remove permanently

```sh
makakoo plugin uninstall agent-dreams --purge
```
