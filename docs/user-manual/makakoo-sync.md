# `makakoo sync` — index the on-disk Brain

`makakoo sync` walks the local Brain files and updates the FTS5 search index.
Use it after you or an agent writes Markdown directly under journals, pages, or
auto-memory.

## Synopsis

```bash
makakoo sync [--force] [--embed] [--no-auto-memory] [--embed-limit N] [--file PATH]
```

## Common use

```bash
# Normal full walk
makakoo sync

# Re-index one journal after a manual append
makakoo sync --file ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md

# Re-index everything, ignoring cached content hashes
makakoo sync --force
```

`--embed` also fills missing vector embeddings when the embedding gateway is
reachable. It is best-effort and capped by `--embed-limit`.
