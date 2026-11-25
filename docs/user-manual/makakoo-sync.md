# `makakoo sync` — index the on-disk Brain

`makakoo sync` walks the local Brain files and updates the SQLite FTS5 search
index and entity knowledge graph. Use it after you or an agent writes Markdown
directly under journals, pages, auto-memory, or registered enrichment sources.

## Synopsis

```bash
makakoo sync [--force] [--embed] [--no-auto-memory] [--embed-limit N] [--file PATH]
```

## Common use

```bash
# Normal full walk (stat fast-skip; zero writes when nothing changed)
makakoo sync

# Re-index one journal after a manual append
makakoo sync --file ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md

# Re-index everything, ignoring cached file stats and content hashes
makakoo sync --force

# Sync + embed pending documents (holds a scoped idle-sleep guard while embedding)
makakoo sync --embed --embed-limit 200
```

## Performance & crash safety (v0.4.2+)

- **O(stat) fast-skip:** `makakoo sync` preloads indexed metadata in one query
  and skips unchanged files by `(file_mtime_ns, file_size)` plus source
  metadata without opening, hashing, or regex-parsing the file. On the first
  sync after upgrading from schema v1, missing file stats are backfilled once
  without rewriting FTS5 or the graph.
- **Zero-write no-op sync:** When no documents changed and the graph is already
  populated, `makakoo sync` skips rebuilding `entity_graph` and
  `brain_graph_*`, producing zero WAL churn.
- **Atomic graph rebuild:** When files change, the knowledge graph rebuild runs
  inside a single `BEGIN IMMEDIATE` transaction so a crash can never leave
  half-empty graph tables.

## Embeddings & wake locks

`--embed` fills missing vector embeddings when the embedding gateway is
reachable (best-effort, capped by `--embed-limit`). While `--embed` is actively
running, Makakoo acquires a scoped OS idle-sleep guard (`caffeinate -i` on
macOS, `systemd-inhibit` on Linux, `SetThreadExecutionState` on Windows) so a
long embedding batch does not stall if your laptop idles. The display may still
sleep, and the lock releases immediately when embedding finishes or exits.
Disable wake locks globally with `MAKAKOO_NO_WAKE_LOCK=1`.
