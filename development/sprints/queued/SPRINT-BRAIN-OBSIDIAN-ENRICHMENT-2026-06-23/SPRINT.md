# SPRINT-BRAIN-OBSIDIAN-ENRICHMENT-2026-06-23

## Goal

Improve the Makakoo Brain with Obsidian as an **optional enrichment layer**.

This sprint does **not** replace the Makakoo Brain, does **not** migrate Brain ownership to Obsidian, and does **not** make a separate Obsidian vault the source of truth.

The win condition is simple:

- Makakoo keeps `$MAKAKOO_HOME/data/Brain` as canonical memory.
- Obsidian improves human editing, metadata, and graph signals when installed.
- Separate Obsidian vaults can be indexed as labeled extra context without becoming the Brain.
- Superbrain gets richer source labels, frontmatter/tags/aliases, and canvas/graph-derived relationships.

## Current verdict

**Build, but with strict scope.** Obsidian is useful as UI and metadata/graph enrichment. It is not useful as the Brain engine.

The previous risk was wording and architecture drift toward “replace Logseq format with Obsidian.” That is rejected. The correct plan is **canonical Brain plus optional Obsidian enrichment**.

## Implementation status — 2026-06-23

Status: **implemented on branch `fix/brain-setup-defaults`, awaiting review/commit/release flow.**

Implemented:

- Canonical Brain remains pinned to `$MAKAKOO_HOME/data/Brain`.
- External Obsidian/plain Markdown folders are enrichment sources, read-only by default.
- Old `brain_sources.json` shape still loads; new shape writes explicit `canonical` + source `role`.
- Setup/picker can create safe `.obsidian/` UI defaults in the canonical Brain without changing Brain ownership.
- Rust Superbrain ingest reads registered sources and writes source metadata:
  - `source_name`
  - `source_type`
  - `source_role`
  - `relative_path`
- Search hits and CLI search output include source labels.
- Per-source/per-doc-type pruning prevents enrichment churn from deleting canonical rows.
- Missing enrichment roots are skipped without pruning old enrichment rows.
- Missing canonical subdirs do not wipe the canonical index.
- Enrichment roots overlapping canonical Brain are skipped to avoid retagging canonical files.
- Obsidian-compatible graph signals now include:
  - `[[wikilinks]]`
  - inline tags
  - YAML frontmatter `tags`
  - YAML frontmatter `aliases`
  - `.canvas` files as bounded searchable graph hints with sanitized `canvas_*` predicates.
- Internal Canvas edge markers are kept out of FTS search text.
- Normal journal writes remain canonical only.
- SANCHO brain-source task now triggers `makakoo sync` instead of only logging counts, with pytest guard.
- Docs/FAQ/use-case wording now says enrichment, not replacement.

Claude validation:

- `lope ask --validators claude` returned **NO_BLOCKER** for canonical prune safety.
- `lope ask --validators claude` returned **NO_BLOCKER** for Canvas enrichment design.
- Claude noted one non-blocking limitation: Canvas graph edges currently model `canvas file -> target`, not true node-to-node Canvas edges. Accepted for this sprint because it fits the existing document-to-entity graph model.

Verification:

```bash
python3 -m pytest plugins-core/skill-brain-multi-source/tests -q
# 52 passed

cargo test -p makakoo-core superbrain::ingest -- --nocapture
# 18 passed in module

cargo test -p makakoo-core superbrain::store -- --nocapture
# 24 passed in module

cargo test --workspace
# passed
```

## Non-negotiable locked decisions

### LD-1 — Canonical Brain stays fixed

`$MAKAKOO_HOME/data/Brain` remains the canonical Makakoo Brain. Existing journals, pages, Superbrain ingest, MCP tools, and agent prompts continue to treat it as source of truth.

### LD-2 — Obsidian is enrichment, never replacement

Obsidian can add UI, metadata, tags, aliases, backlinks, canvas, and external-vault context. It does not own Brain persistence.

### LD-3 — No forced journal migration

Existing `journals/YYYY_MM_DD.md` files stay valid. Existing `- ` outliner lines stay valid. Obsidian can render them as Markdown. Do not rename old journals to `YYYY-MM-DD.md`.

### LD-4 — Default writes stay canonical

Normal Makakoo journal/page writes keep going to `$MAKAKOO_HOME/data/Brain`. External vault writes require explicit source-targeted command and user-granted writable source. No hidden redirect into Obsidian.

### LD-5 — External vaults are read-only by default

A separate Obsidian vault registered during setup is an enrichment source by default. Writable mode must be explicit and must still not become canonical Brain.

### LD-6 — Graph truth comes from files, not Obsidian view state

Obsidian Graph View is a visual UI derived from notes, links, and tags. The `graph.json` file stores display settings, not the actual graph database. Makakoo should parse the underlying markdown/frontmatter/canvas data and build its own graph.

### LD-7 — Documentation must not overclaim

Docs may say “open the Brain folder in Obsidian” today. Docs may not say “single unified search layer” for separate vaults until Rust Superbrain indexes those sources.

## Source evidence from audit

### Existing code that already helps

```text
plugins-core/skill-brain-multi-source/src/brain_source.py
plugins-core/skill-brain-multi-source/src/config.py
plugins-core/skill-brain-multi-source/src/brain_cli.py
plugins-core/skill-brain-multi-source/src/picker.py
plugins-core/skill-brain-multi-source/src/sancho_ingest.py
plugins-core/skill-productivity-obsidian/src/__main__.py
makakoo-core/src/superbrain/ingest.rs
makakoo-core/src/capability/service/brain.rs
makakoo-mcp/src/handlers/tier_a/brain.rs
```

What exists:

- `BrainSource` adapter abstraction supports `LogseqSource`, `ObsidianSource`, and `PlainMarkdownSource`.
- `ObsidianSource` reads `.obsidian/daily-notes.json`, walks markdown, skips hidden dirs, and can format flat journal lines.
- `brain_sources.json` registry exists.
- Setup picker can detect/install Obsidian and register separate vaults.
- Default Brain path already opens fine as an Obsidian vault because it is plain Markdown.

### Existing code that blocks full value

- `makakoo-core/src/superbrain/ingest.rs` hardcodes `home/data/Brain/pages`, `home/data/Brain/journals`, and optional `home/data/auto-memory`.
- Rust ingest does not read `config/brain_sources.json`.
- `sancho_ingest.py` is currently a count/log stub, not real ingestion.
- `BrainHandler::write_journal` writes only to canonical `journals/YYYY_MM_DD.md` and always prepends `- `.
- MCP Brain tools search only `SuperbrainStore`, so they only see what Rust indexed.
- `brain_docs` schema has no source metadata columns.

### Runtime evidence on Sebastian machine

Current runtime config includes:

```json
{
  "default": "default",
  "sources": [
    {"name": "default", "type": "logseq", "path": "$MAKAKOO_HOME/data/Brain", "writable": true},
    {"name": "obsidian", "type": "obsidian", "path": "~/Documents/Obsidian Vault", "writable": true}
  ]
}
```

Dry walk result observed during audit:

```text
default: 1253 docs
obsidian: 1907 docs
```

Superbrain DB observed during audit:

```text
brain_docs total: 1665
paths under /Users/sebastian/Documents/%: 0
paths containing Obsidian Vault: 0
```

Meaning: separate Obsidian vault is registered/countable but not actually indexed into Superbrain today.

## Target architecture

```text
                       +-----------------------------+
                       | Human UI                    |
                       | - Obsidian app              |
                       | - Logseq / editor / CLI     |
                       +--------------+--------------+
                                      |
                                      v
+---------------------------+   +---------------------------+
| Canonical Brain           |   | Enrichment sources        |
| $MAKAKOO_HOME/data/Brain  |   | - Obsidian vaults         |
| - journals/               |   | - plain markdown folders  |
| - pages/                  |   | - future sources          |
+-------------+-------------+   +-------------+-------------+
              |                               |
              v                               v
        +---------------------------------------------+
        | Source registry + source adapters           |
        | canonical source is fixed to Makakoo Brain   |
        | enrichment sources are labeled extras        |
        +----------------------+----------------------+
                               |
                               v
        +---------------------------------------------+
        | Rust Superbrain ingest                       |
        | - source metadata                            |
        | - markdown/frontmatter parser                |
        | - per-source prune                           |
        | - graph triple extraction                    |
        +----------------------+----------------------+
                               |
                               v
        +---------------------------------------------+
        | Superbrain                                  |
        | - FTS5                                      |
        | - vectors                                   |
        | - entity graph                              |
        | - relation graph                            |
        +----------------------+----------------------+
                               |
                               v
        +---------------------------------------------+
        | Agents / MCP / MemoryStack / HarveyChat      |
        | Searches canonical + enrichment, labeled.    |
        | Writes stay canonical unless explicit.       |
        +---------------------------------------------+
```

## Scope boundaries

### In scope

- Make default Makakoo Brain folder Obsidian-friendly when user has Obsidian.
- Clarify source registry semantics: canonical Brain vs enrichment sources.
- Real Superbrain indexing for registered external Obsidian/plain-markdown sources.
- Add source metadata to indexed docs and search hits.
- Parse Obsidian-compatible metadata: frontmatter, tags, aliases, date properties, wikilinks in properties.
- Parse Obsidian Canvas `.canvas` files as extra relationship maps when present.
- Keep normal Brain writes canonical.
- Fix docs and setup wording to remove replacement ambiguity.
- Tests proving no Brain replacement and no silent external writes.

### Out of scope

- Replacing `$MAKAKOO_HOME/data/Brain`.
- Migrating journal filenames from `YYYY_MM_DD.md` to `YYYY-MM-DD.md`.
- Rewriting all Brain pages into Obsidian-native style.
- Depending on Obsidian desktop app for core memory.
- Depending on Obsidian CLI for core memory.
- Obsidian Sync integration.
- Obsidian Publish integration.
- Installing community plugins by default.
- Full bidirectional sync or conflict resolution between canonical Brain and external vaults.
- Cloud vault sync.
- Deleting or modifying user vault files during ingest.

## Phase 0 — Plan and docs correction

**Goal:** Prevent architecture drift and user confusion before implementation.

### Deliverables

- Update docs to use these terms:
  - `Canonical Brain`: `$MAKAKOO_HOME/data/Brain`
  - `Enrichment source`: external Obsidian/plain folder indexed as extra context
  - `Obsidian UI`: opening canonical Brain in the Obsidian desktop app
- Remove or qualify “single unified search layer” claims until Phase 3 lands.
- Update setup docs to say external vaults are read-only enrichment by default.
- Update `skill-brain-multi-source/SKILL.md` to forbid saying “make Obsidian the Brain.”

### Files likely touched

```text
docs/user-manual/setup-wizard.md
docs/use-cases.md
docs/faq.md
docs/plugins/index.md
plugins-core/skill-brain-multi-source/SKILL.md
plugins-core/skill-brain-multi-source/plugin.toml
```

### Success checks

- `rg -n "replace.*Brain|single unified search layer|write-default" docs plugins-core/skill-brain-multi-source` shows no misleading wording.
- Docs distinguish current shipped behavior from queued implementation.

## Phase 1 — Obsidian-friendly profile for canonical Brain

**Goal:** If the user has Obsidian, make the existing canonical Brain pleasant to open in Obsidian without changing Brain ownership or data format.

### Deliverables

- Add helper to create safe `.obsidian/` defaults under `$MAKAKOO_HOME/data/Brain` only when:
  - Obsidian is installed or user opted into Obsidian setup.
  - No existing conflicting `.obsidian` config exists.
  - User accepts the wizard prompt.
- Suggested default files:
  - `.obsidian/core-plugins.json`: graph, backlinks, outgoing links, tags, daily notes, templates, file recovery enabled.
  - `.obsidian/daily-notes.json`: folder `journals`, format `YYYY_MM_DD`.
  - `.obsidian/graph.json`: display config only, no graph-data assumptions.
- Do not overwrite user config without backup/confirmation.
- Do not create separate source registration for the canonical Brain. It is already canonical.

### Files likely touched

```text
plugins-core/skill-brain-multi-source/src/picker.py
plugins-core/skill-brain-multi-source/tests/test_picker.py
makakoo/src/commands/setup/brain.rs
```

### Success checks

- Fresh temp home, Obsidian accepted: `.obsidian/daily-notes.json` created with `journals` + `YYYY_MM_DD`.
- Existing `.obsidian` config: no overwrite.
- Obsidian skipped: no `.obsidian` files created.
- Default `brain_sources.json` remains canonical Brain only unless user registers additional source.

## Phase 2 — Source registry semantics: canonical vs enrichment

**Goal:** Fix the dangerous ambiguity where a separate vault can appear to become the Brain default.

### Deliverables

- Extend `brain_sources.json` semantics while keeping backward compatibility:

```json
{
  "canonical": "default",
  "sources": [
    {"name": "default", "role": "canonical", "type": "logseq", "path": "$MAKAKOO_HOME/data/Brain", "writable": true},
    {"name": "personal", "role": "enrichment", "type": "obsidian", "path": "~/Documents/MyVault", "writable": false}
  ]
}
```

- Loader accepts old shape with `default` and maps it to `canonical` internally.
- Picker stops asking “change write-default” in normal flow.
- CLI `list` shows `canonical` and `enrichment` roles.
- CLI `set-default` either removed from docs or renamed to an explicit advanced `set-canonical` that refuses non-`$MAKAKOO_HOME/data/Brain` paths unless a hard override exists.
- External Obsidian vault registration default is read-only.

### Files likely touched

```text
plugins-core/skill-brain-multi-source/src/config.py
plugins-core/skill-brain-multi-source/src/brain_cli.py
plugins-core/skill-brain-multi-source/src/picker.py
plugins-core/skill-brain-multi-source/tests/test_picker.py
makakoo/src/commands/setup/brain.rs
```

### Success checks

- Old config still loads.
- New config writes `canonical` and `role` fields.
- Registering external Obsidian vault does not change canonical source.
- Normal Makakoo journal write path unchanged.

## Phase 3 — Real Superbrain ingest for enrichment sources

**Goal:** Registered Obsidian/plain folders become searchable Brain context with clear source labels.

### Deliverables

- Rust ingest reads `config/brain_sources.json`.
- Add DB schema migration for source metadata:
  - `source_name`
  - `source_type`
  - `source_role`
  - `relative_path`
  - maybe `source_root_hash` or `source_id` if path privacy matters.
- Upsert canonical docs and enrichment docs into `brain_docs`.
- `SearchHit` includes source metadata.
- Per-source prune:
  - Full sync of canonical source only prunes canonical rows.
  - Full sync of enrichment source only prunes that source's rows.
  - Missing external source does not delete old indexed docs unless explicit prune requested.
- Hidden dirs skipped:
  - `.obsidian/`
  - `.trash/`
  - `.git/`
  - `.logseq/`
  - `bak/`
- Source collision handling:
  - same filename in different sources remains separate docs.
  - search display includes source label.

### Files likely touched

```text
makakoo-core/src/db.rs
makakoo-core/src/superbrain/ingest.rs
makakoo-core/src/superbrain/store.rs
makakoo-core/src/superbrain/graph.rs
makakoo-mcp/src/handlers/tier_a/brain.rs
makakoo/src/commands/sync.rs
plugins-core/skill-brain-multi-source/src/sancho_ingest.py
```

### Success checks

- Temp test with canonical + external vault indexes both.
- External path appears in `brain_docs` with `source_role='enrichment'`.
- `brain_search` returns source labels.
- Missing external vault does not prune canonical docs and does not silently wipe external indexed rows.
- `cargo test -p makakoo-core superbrain::ingest` passes.
- `cargo test --workspace` passes.

## Phase 4 — Obsidian metadata and graph enrichment

**Goal:** Improve Makakoo graph quality using Obsidian-compatible metadata, not Obsidian visual state.

### Deliverables

Parse markdown/frontmatter into graph triples:

```text
note --links_to--> note
note --tagged--> tag
note --alias--> alias
alias --resolves_to--> note
note --has_date--> daily_note
note --has_property--> property_value
note --mentions_unresolved--> unresolved_concept
```

Parse these structures:

- `[[wikilinks]]`
- embeds `![[file]]`
- inline tags `#tag`
- YAML frontmatter `tags`
- YAML frontmatter `aliases`
- date/date-time properties
- internal links inside YAML property values
- unresolved links

Canvas support:

- Parse `.canvas` JSON files when present.
- Ingest canvas nodes and labeled edges as relation hints.
- Treat text-only canvas cards as content snippets only if safe and bounded.
- Do not require canvas files to exist.

### Files likely touched

```text
makakoo-core/src/superbrain/ingest.rs
makakoo-core/src/superbrain/graph.rs
makakoo-core/src/superbrain/store.rs
makakoo-core/src/db.rs
```

### Success checks

- Frontmatter alias test: searching alias finds canonical note.
- Tag test: `#project/foo` and YAML `tags` both become graph edges.
- Date property test: date links to matching daily note when possible.
- Canvas fixture test: labeled edge appears in graph relation table.
- Invalid YAML/canvas fails soft and indexes note content anyway.

## Phase 5 — MCP, memory, and UX surface

**Goal:** Make enriched Brain useful to agents without letting source ambiguity leak into writes.

### Deliverables

- `brain_search` and `brain_query` include source labels in result JSON.
- Optional search filters:
  - `source_name`
  - `source_type`
  - `source_role`
- `brain_context` can include top source labels when relevant.
- `brain_recent` can filter by source metadata.
- Normal `brain_write_journal` remains canonical only.
- Add explicit future-facing source write API only if needed:
  - `brain.write_source_note(source_name, path/title, content)`
  - must reject read-only sources
  - not required for sprint completion unless simple.
- Setup output clearly says: “Obsidian improves Brain context; Makakoo still writes its canonical journal.”

### Files likely touched

```text
makakoo-mcp/src/handlers/tier_a/brain.rs
makakoo-core/src/capability/service/brain.rs
makakoo-core/src/superbrain/memory_stack.rs
makakoo-core/src/superbrain/store.rs
```

### Success checks

- MCP `brain_search` returns external Obsidian hit with source metadata.
- MCP `brain_write_journal` still writes to `$MAKAKOO_HOME/data/Brain/journals/YYYY_MM_DD.md`.
- Attempted normal journal write never writes into external vault.

## Phase 6 — QA, migration safety, and release docs

**Goal:** Prove the enrichment layer works and cannot accidentally replace the Brain.

### Automated checks

```bash
python3 -m pytest plugins-core/skill-brain-multi-source/tests -q
cargo test -p makakoo-core superbrain::ingest -- --nocapture
cargo test -p makakoo-core superbrain::graph -- --nocapture
cargo test -p makakoo-mcp brain -- --nocapture
cargo test --workspace
```

Adapt exact test names to real modules after implementation.

### Manual checks

1. Fresh temp Makakoo home, no Obsidian: setup completes with canonical Brain only.
2. Fresh temp Makakoo home, Obsidian accepted: canonical Brain gets safe `.obsidian` profile, no external source created.
3. External Obsidian vault registered read-only: `sync` indexes docs with `source_role=enrichment`.
4. `brain_search "unique external term"` finds an external vault note and displays source label.
5. `harvey_brain_write` appends to canonical journal only.
6. Remove/move external vault temporarily, run canonical sync: canonical docs stay indexed; external rows are not silently pruned.
7. Invalid YAML frontmatter: note content still indexed, metadata parse warning is non-fatal.
8. Canvas fixture: labeled relation appears in graph query.

### Release docs

- Update `CHANGELOG.md`.
- Update setup wizard docs.
- Update FAQ/use cases.
- Add warning that external source indexing reads local markdown, no cloud sync.

## Failure modes and required handling

| Failure mode | Required behavior | Test required |
|---|---|---|
| External vault path missing | Warn, skip source, do not prune indexed rows by default | yes |
| External vault unreadable | Warn, continue canonical sync | yes |
| Invalid YAML frontmatter | Index content, skip metadata, record warning | yes |
| Invalid `.canvas` JSON | Skip canvas graph, continue sync | yes |
| Same relative path in two sources | Store as separate docs by source identity | yes |
| External source too large | Bound ingest, report counts, avoid memory blowup | yes |
| User marks external source writable | Still not canonical; normal journal writes stay canonical | yes |
| Old `brain_sources.json` shape | Loader migrates/normalizes without data loss | yes |
| Full sync prunes wrong source | Must not happen; per-source prune required | critical test |

## Data model sketch

Preferred minimum migration:

```sql
ALTER TABLE brain_docs ADD COLUMN source_name TEXT DEFAULT 'default';
ALTER TABLE brain_docs ADD COLUMN source_type TEXT DEFAULT 'logseq';
ALTER TABLE brain_docs ADD COLUMN source_role TEXT DEFAULT 'canonical';
ALTER TABLE brain_docs ADD COLUMN relative_path TEXT;
```

If uniqueness must change, use a staged migration:

```text
old unique: path
new unique: source_name + relative_path
compat: keep path as absolute display/debug path
```

Do not rush this. Unique-key migration is the part most likely to bite.

## Worktree parallelization strategy

| Lane | Work | Modules touched | Depends on |
|---|---|---|---|
| A | Docs and setup wording | docs/, plugin SKILL/plugin.toml | none |
| B | Picker Obsidian profile + registry semantics | plugins-core/skill-brain-multi-source/, makakoo/src/commands/setup/ | none |
| C | Rust ingest + DB source metadata | makakoo-core/src/db.rs, makakoo-core/src/superbrain/ | B config shape finalized |
| D | MCP/search UX | makakoo-mcp/, makakoo-core/src/capability/ | C |
| E | QA/release docs | tests/docs/changelog | A+B+C+D |

Execution:

```text
Start A + B in parallel.
Merge B.
Run C.
Run D after C.
Run E last.
```

Conflict flags:

- B and C both depend on config shape. Decide schema before parallel coding.
- C is the risky lane. Keep it single-owner.
- A can safely run anytime but must be rechecked after C/D final behavior.

## Implementation prompt for the next agent

```text
You are in /Users/sebastian/makakoo-os on sprint development/sprints/queued/SPRINT-BRAIN-OBSIDIAN-ENRICHMENT-2026-06-23.

Implement Obsidian as optional Makakoo Brain enrichment, not replacement.

Hard rules:
- Canonical Brain remains $MAKAKOO_HOME/data/Brain.
- Normal journal writes stay canonical.
- External Obsidian vaults are enrichment sources, read-only by default.
- Do not migrate existing journals or pages.
- Do not depend on Obsidian app or CLI for core memory.
- Real value is source-labeled ingest plus metadata/graph enrichment.

Start with Phase 0 docs and Phase 2 config semantics before touching Rust ingest. Then implement Phase 3 source-labeled ingest with per-source pruning. Phase 4 metadata/canvas enrichment can follow after source metadata is stable.

Run the checks listed in SPRINT.md and update this sprint file with results.
```

## Done definition

Sprint is done when:

- Default Brain can be made Obsidian-friendly without migration.
- Separate Obsidian vaults are indexed as labeled enrichment sources.
- Search results show source metadata.
- Tags/aliases/frontmatter/canvas enrich Makakoo graph.
- Normal Brain writes still target canonical Brain only.
- Docs no longer imply Brain replacement or unsupported unified search.
- Tests prove old configs, missing vaults, invalid metadata, and per-source prune safety.

It is **not** done if the only change is setup wording or dry-walk counts. The external source must reach Superbrain search with labels, or it remains registry-only.

## Review summary

### Architecture review

1. The plan is correct only if canonical Brain remains fixed. Any “write-default external vault” behavior should be demoted or hidden behind an advanced explicit command.
2. The risky implementation is not Obsidian profile creation; it is DB uniqueness and prune semantics when multiple sources exist.
3. Obsidian Graph View itself is not data. Parse markdown/frontmatter/canvas and build Makakoo’s graph.
4. Obsidian CLI is useful later but must stay optional because core memory cannot depend on a running desktop app.

### Code quality review

- Reuse existing `BrainSource` adapters and registry. Do not invent a second registry.
- Move real ingest into Rust Superbrain, not Python SANCHO stub.
- Keep source metadata explicit and visible in search hits.
- Keep source writes explicit. Hidden routing is how the Brain gets accidentally replaced.

### Test review

Critical test gaps to close:

- per-source prune safety
- old config compatibility
- missing external vault behavior
- invalid YAML/canvas fail-soft behavior
- normal write path remains canonical
- source collision handling

### NOT in scope

- Replacing Brain with Obsidian: rejected.
- Obsidian Sync: separate paid/cloud product, not needed for Brain improvement.
- Community plugins: extra attack/support surface, not needed.
- Full bidirectional sync: conflict-heavy, not needed for this sprint.
- Journal migration: high risk, low value.

### What already exists

- `BrainSource` adapter layer: reuse and harden.
- Setup picker: reuse and reword.
- Superbrain ingest: extend in Rust.
- MCP brain handlers: extend response metadata.
- Legacy `skill-productivity-obsidian`: keep separate; do not treat as core Brain engine.

### Lake score

Completeness target: **9/10**.

We are not boiling the ocean. We are boiling the lake that matters: source-labeled ingest, metadata graph enrichment, and write safety. Full sync/conflict resolution is the ocean and stays out.
