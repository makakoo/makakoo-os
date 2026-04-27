---
name: wiki
version: 0.1.0
description: |
  Turn freeform markdown into valid Logseq-outliner format, lint
  existing wiki pages, and save content atomically with fs2 locking.
  Three MCP tools; any external doc tool that wants to emit Brain-
  compatible pages (Logseq bullets with [[wikilinks]]) can call these
  without bundling any Makakoo code.
allowed-tools:
  - wiki_compile
  - wiki_lint
  - wiki_save
category: productivity
tags:
  - logseq
  - wiki
  - outliner
  - cli-agnostic
  - mcp-tool
---

# wiki — Logseq-compatible doc pipeline

Makakoo's Brain is a Logseq vault — every page is an outliner where
every line starts with `- ` and `[[WikiLinks]]` cross-reference
entities. Three MCP tools expose the compiler, linter, and atomic
write primitives, so any external doc tool can produce + validate +
persist Brain-compatible pages without bundling Makakoo itself.

## When to reach for wiki tools

| Situation | Tool |
|---|---|
| *"Turn this plain markdown into a Brain page"* | `wiki_compile` |
| *"Check if this page is valid Logseq outline"* | `wiki_lint` |
| *"Write this content to disk atomically"* | `wiki_save` |

Typical external pipeline:

1. Generate content in your own system.
2. `wiki_compile` → normalized bullet tree.
3. `wiki_lint` → catch hierarchy or wikilink issues.
4. `wiki_save` → atomic persist under `$MAKAKOO_HOME/data/Brain/`.

## `wiki_compile` — freeform → outliner

```json
{
  "tool": "wiki_compile",
  "arguments": {
    "source_path": "/path/to/notes.md",
    "title": "My Notes",
    "collapse_blanks": false
  }
}
```

- `source_path` (required) — absolute path or relative to
  `$MAKAKOO_HOME`. Must be a readable markdown file.
- `title` — optional page title; prefixed at the top of the compiled
  output if given.
- `collapse_blanks` — `true` squashes multiple blank lines to one;
  default `false` preserves spacing.

Returns:
```json
{
  "content": "- Harvey runs on switchAILocal\n- Hermes too\n",
  "lines_rewritten": 2,
  "lines_total": 2
}
```

Read-only; `wiki_compile` never writes anything. Combine with
`wiki_save` when you want to persist.

## `wiki_lint` — validation pass

```json
{
  "tool": "wiki_lint",
  "arguments": {
    "page_path": "data/Brain/pages/Harvey.md"
  }
}
```

Two call shapes:
- `page_path` — absolute or relative to `$MAKAKOO_HOME`. Lints the
  file on disk.
- `content` — inline string (CLI-friendly). Lints without touching
  disk.

Exactly one of `page_path` / `content` must be provided. Returns a
report with an `issues` array — each issue carries a line number,
category (hierarchy, wikilink, whitespace), and human-readable
message.

Clean pages return `{issues: []}`. A missing wikilink target shows up
as `{line: 42, category: "wikilink", message: "wikilink target [[Foo]] not found"}`.

## `wiki_save` — atomic persist

```json
{
  "tool": "wiki_save",
  "arguments": {
    "path": "/absolute/path/to/page.md",
    "content": "- Harvey runs on switchAILocal\n"
  }
}
```

- `path` — absolute filesystem path.
- `content` — the full file body. `wiki_save` does NOT append;
  callers are responsible for round-tripping existing content if they
  want a merge.

Writes through `makakoo_core::wiki::save`, which uses an fs2 exclusive
lock + atomic-rename pattern so concurrent writers can't produce a
torn file. Creates parent directories as needed. Overwrites existing
files in place.

Returns `{ok: true, bytes: <N>}`.

## Portable integration (external agentic apps)

The handlers live in Rust:
- Tier-A: `makakoo-mcp/src/handlers/tier_a/wiki.rs` (`wiki_lint`).
- Tier-B: `makakoo-mcp/src/handlers/tier_b/wiki.rs` (`wiki_compile`,
  `wiki_save`).

Underlying logic is in `makakoo_core::wiki::{compiler, lint, save}`.

External runtime options:

1. **Connect to `makakoo-mcp`** and call the three tools over MCP
   stdio. This is the recommended path for LangChain / OpenAI
   Assistants / Cursor / any MCP-aware framework.
2. **Reimplement the Logseq contract**: bullet-prefix every line
   (`- `), use `[[WikiLinks]]` for cross-refs, indent by two spaces
   to nest children. You'll miss the deep validation `wiki_lint`
   provides but the *output* is portable — Logseq will accept it.

## Don't invent your own lint rules

`wiki_lint` already ships every rule Logseq itself enforces plus a
few Makakoo-specific ones (wikilink target existence, journal-page
date format). If your external tool starts checking for "bullets must
start with `- `" locally, you'll diverge from the canonical rule set
and see different reports between local vs. Makakoo. Use `wiki_lint`
as the source of truth; render the issues your way.

## Edge cases

- **Empty file or one-line input**: `wiki_compile` returns the line
  prefixed with `- ` and `lines_rewritten: 1`.
- **Mixed indent styles (tab vs 2-space)**: `wiki_compile`
  normalises to two-space. Round-tripping a Logseq export is safe.
- **Content with no `- ` prefix**: compiler adds them. `wiki_lint`
  flags them on a raw file but `wiki_compile` fixes them.
- **Concurrent writers**: `wiki_save` serialises via fs2. Worst case
  is one caller waits for the other's rename; no torn files.
