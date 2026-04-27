# Walkthrough 02 — Your first Brain entry

## What you'll do

Write a single line of memory into Makakoo's **Brain**, sync it into the searchable index, and find it back with `makakoo search`. No AI model, no keys, no network — just files and full-text search.

**Time:** about 3 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md) completed (`~/MAKAKOO/` exists, `makakoo --version` works).

## What is the Brain?

The Brain is a folder on your computer at `~/MAKAKOO/data/Brain/`. Every day has a "journal" file there, named with today's date. Every meaningful person, project, or topic can have its own "page" file.

Makakoo reads and writes this folder. Every AI CLI you've infected reads it too. So anything you put in the Brain becomes memory that every AI you use can see.

The format is plain Markdown with one quirk: **every line starts with `- `** (a dash and a space). That's it. That's the whole format.

## Steps

### 1. Open today's journal

The file name is today's date, with underscores. Today is `2026-04-24`, so the file is `~/MAKAKOO/data/Brain/journals/2026_04_24.md`.

> **On your machine, today will be different.** Wherever the walkthrough shows `2026_04_24`, substitute today's date in `YYYY_MM_DD` form. Check today's date with `date +%Y_%m_%d`.

Open it in a simple editor. On macOS, TextEdit is fine:

```sh
open -a TextEdit ~/MAKAKOO/data/Brain/journals/2026_04_24.md
```

If the file doesn't exist yet, create it first:

```sh
mkdir -p ~/MAKAKOO/data/Brain/journals
touch ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md
```

### 2. Write one line of memory

Add this line to the file (or your own variation). The line **must** start with `- ` (dash, space):

```text
- Tried Makakoo for the first time. [[Harvey]] is my assistant. The Brain is at `~/MAKAKOO/data/Brain/`.
```

Save and close TextEdit.

The `[[Harvey]]` part is a **wikilink** — any text wrapped in `[[ ]]` becomes a cross-reference. If a page called `Harvey.md` exists, that's the link target; if not, Makakoo notes the mention and will build the page on first write.

### 3. Sync the Brain into the search index

Makakoo keeps a fast search index (SQLite + FTS5) separate from the Markdown files. You just wrote to a file, so the index doesn't know yet. Tell it:

```sh
makakoo sync
```

Expected output (numbers will vary based on how many files already exist):

```text
sync complete: 1 pages, 1 journals, 0 memories, 0 skipped, 0 removed, 0 errors, 0 vectors (3 graph nodes / 2 edges)
```

On an already-populated Brain the numbers will be much larger, e.g.:

```text
sync complete: 868 pages, 46 journals, 425 memories, 5 skipped, 0 removed, 0 errors, 0 vectors (2197 graph nodes / 4433 edges)
```

Either shape is healthy as long as `0 errors` and `0 removed` are present. `0 vectors` is expected — vector embeddings are opt-in (`makakoo sync --embed`) and aren't needed for plain search.

### 4. Search for what you wrote

```sh
makakoo search "Brain is at"
```

Expected output (truncated — scores and exact paths will differ):

```text
┌───────────────────────────────────────────────────┬─────────┬───────┬─────────────────────────────────────────────┐
│ doc_id                                            │ type    │ score │ snippet                                     │
├───────────────────────────────────────────────────┼─────────┼───────┼─────────────────────────────────────────────┤
│ /Users/you/MAKAKOO/data/Brain/journals/2026_04_24 │ journal │ 9.1   │ - Tried Makakoo for the first time. [[Harv… │
└───────────────────────────────────────────────────┴─────────┴───────┴─────────────────────────────────────────────┘
```

The line you just wrote is the top hit. That's your first Brain round-trip.

### 5. Search for the wikilink target

```sh
makakoo search "Harvey"
```

You'll probably see many results (every journal entry that mentions `[[Harvey]]`). Your new line should be among the top hits because it was just touched.

## What just happened?

- **The Brain** is just a folder of Markdown files at `~/MAKAKOO/data/Brain/`. No database, no cloud. You can read it in Finder, edit it in any text editor, back it up with `rsync` or Time Machine, delete it entirely with `rm -rf`.
- **`makakoo sync`** walks that folder, notices what changed, and updates a small SQLite index at `~/MAKAKOO/data/makakoo.db`. This index is what makes search fast — the Markdown files are the source of truth.
- **`makakoo search`** queries that index. The search is full-text (FTS5): fuzzy word matching, BM25 scoring, snippet extraction. It does NOT use any AI model, so it works offline and costs nothing to run.
- **Wikilinks (`[[name]]`)** are not magic — Makakoo just indexes them as regular words. Their value is consistency: if you always link `[[Harvey]]` the same way, you can find every mention of Harvey with one search.

The richer cousin of `makakoo search` is `makakoo query` — same retrieval, but the top hits get fed to an LLM which writes a synthesized answer. That needs a configured model provider, which walkthrough [05 — ask Harvey](./05-ask-harvey.md) covers.

## If something went wrong

| Symptom | Fix |
|---|---|
| `open -a TextEdit: command not found` | Not on macOS? Use whatever editor you have: `nano ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md` works on every Unix system. On Windows: `notepad %USERPROFILE%\MAKAKOO\data\Brain\journals\2026_04_24.md`. |
| Line starts with something other than `- ` | Makakoo still indexes the file, but the line won't render as a Logseq-style bullet. Put `- ` at the start. The only hard requirement. |
| `makakoo sync` reports `indexed 0 new` after you added a line | The file wasn't saved. Re-open it in the editor, confirm the line is there, save again, rerun `makakoo sync`. |
| `makakoo search` returns no results | Run `makakoo sync --force` to rebuild the whole index from scratch. If still empty, check the file actually contains your line: `grep "Brain is at" ~/MAKAKOO/data/Brain/journals/*.md` |
| Any other error | See [`docs/troubleshooting/index.md`](../troubleshooting/index.md). |

## Next

- [Walkthrough 03 — Install a plugin](./03-install-plugin.md) — browse the catalog and install your first plugin.
- [Walkthrough 04 — Writing to the Brain organically](./04-write-brain-journal.md) — how SANCHO + infected CLIs populate the Brain without you touching a file.
- [Walkthrough 05 — Ask Harvey](./05-ask-harvey.md) — wire up an LLM and query the Brain with a real question.
