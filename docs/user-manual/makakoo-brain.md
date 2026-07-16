# `makakoo brain` - sources and OKF interchange

`makakoo brain` manages the knowledge sources indexed by Superbrain and imports or exports [Open Knowledge Format](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md) v0.1 bundles.

The canonical Brain never moves. It remains at `$MAKAKOO_HOME/data/Brain`. Obsidian, plain-Markdown, Logseq, and OKF paths are enrichment sources.

## List sources

```bash
makakoo brain list
makakoo brain list --json
```

## Register an enrichment source

```bash
makakoo brain add personal obsidian ~/Documents/MyVault --read-only
makakoo brain add notes plain ~/Documents/notes --read-only
makakoo brain add catalog okf ~/knowledge/catalog
# Explicit opt-in when Makakoo should write into an enrichment source
makakoo brain add working-notes plain ~/Documents/working-notes --writable
```

Supported types are `logseq`, `obsidian`, `plain`, and `okf`. Enrichment sources default to read-only; writes require `--writable`. An OKF source must validate before registration and is always read-only.

The canonical source name `default` is fixed. The registry keeps one entry per source name, source roots may not overlap, and registering an existing name updates that registry entry without deleting either directory.

Unregistering does not delete source files:

```bash
makakoo brain remove catalog
```

Run `makakoo sync` after source changes to refresh FTS5 and the entity graph.

## Import and index an OKF bundle

Import means registering an existing bundle as read-only enrichment. Makakoo does not copy the bundle into the canonical Brain.

```bash
# Inspect precise diagnostics first
makakoo brain validate ~/knowledge/partner-catalog --json

# Registration validates again and refuses structural errors
makakoo brain add partner-catalog okf ~/knowledge/partner-catalog

# Index concepts, metadata, and Markdown relationships
makakoo sync

# Confirm retrieval from the labeled source
makakoo search "known catalog term"
```

Reserved `index.md` and `log.md` files support progressive disclosure and history but are not indexed as concept documents. Concept frontmatter `type` values become `#okf-type/<type>` search metadata. Local Markdown links become qualified graph relationships, so identical filenames in different bundles do not collapse into one concept.

If an imported bundle changes on disk, run `makakoo sync` again. Invalid or unreadable concepts are skipped and stale indexed copies are removed instead of being returned as current knowledge.

## Export OKF v0.1

The default export contains canonical Brain pages and durable auto-memory. Daily journals are excluded.

```bash
makakoo brain export --format okf --source default --out ~/exports/makakoo-okf
```

Optional controls:

```bash
# Include daily journals explicitly
makakoo brain export --out ~/exports/full-okf --include-journals

# Exclude auto-memory
makakoo brain export --out ~/exports/pages-only --no-auto-memory

# Replace an existing non-empty destination
makakoo brain export --out ~/exports/makakoo-okf --force

# Machine-readable result
makakoo brain export --out ~/exports/makakoo-okf --json
```

Export is local-only. It builds in a locked sibling staging directory and uses owned recovery markers plus a verified backup when `--force` replaces an existing bundle. Unrelated files that collide with an internal recovery name are never deleted automatically. It never uploads or publishes anything. The exporter:

- synthesizes required `type` metadata when a Brain page has none;
- preserves existing YAML metadata;
- writes `title`, `description`, `resource`, `tags`, and `timestamp`;
- converts Logseq `[[wikilinks]]` into bundle-relative Markdown links;
- creates root and directory `index.md` files;
- pins `okf_version: "0.1"` in the root index.

### Public-safe mode

`--public` is an allowlist, not a redactor. Only documents explicitly carrying `visibility: public` in YAML frontmatter or `visibility:: public` as a Logseq property are exported. Credential-shaped tokens, private-key blocks, and credential assignments make the whole export fail closed.

```bash
makakoo brain export --out ~/exports/public-okf --public
```

The command still does not publish the result. Review the bundle before sharing it.

If no document qualifies as public, export fails instead of producing a misleading empty bundle.

## Ingest files or folders into a bundle

`brain ingest` builds an OKF v0.1 bundle from arbitrary local Markdown — loose files, folders, or a mix. Use it to turn any documentation set into portable knowledge that can then be validated, registered as enrichment, or shared.

```bash
# One folder becomes one bundle
makakoo brain ingest ~/notes/project-docs --out ~/exports/project-okf

# Mix loose files and folders; name the knowledge source
makakoo brain ingest README.md docs/ --out ~/exports/repo-okf --name my-repo

# Replace an existing non-empty destination / machine-readable result
makakoo brain ingest docs/ --out ~/exports/repo-okf --force --json
```

Rules and behavior:

- Only `.md` files are ingested; other file types are skipped. If the inputs contain no Markdown at all, the command fails instead of writing an empty bundle.
- A single folder input is walked in place. Loose files or multiple inputs are staged first; name collisions get a `-N` suffix.
- Hidden directories, `node_modules`, `bak`, and symlinks are skipped.
- The output goes through the same staged, crash-recoverable writer as `brain export`, and the resulting bundle passes `brain validate` by construction.
- Ingest builds a bundle only — it does not register or index anything. To make the knowledge searchable, follow with `brain add <name> okf <bundle>` and `makakoo sync`.

## Validate a bundle

```bash
makakoo brain validate ~/exports/makakoo-okf
makakoo brain validate ~/exports/makakoo-okf --json
```

Validation checks UTF-8 Markdown, YAML frontmatter, required non-empty `type`, and the exact case-sensitive reserved names `index.md` and `log.md`. Index entries must be Markdown list links grouped beneath section headings. Log dates must be real `YYYY-MM-DD` dates in newest-first order, with at least one flat list entry per date; indented prose continuations are allowed.

An empty directory is conformant with warnings. Broken internal links, untraversed symlinks, and unknown declared OKF versions are also warnings because OKF v0.1 requires permissive, best-effort consumption. Structural violations return exit code 1.

## Machine-readable output and exit status

`brain list --json`, `brain export --json`, `brain ingest --json`, and `brain validate --json` write JSON to standard output.

- Export JSON contains `version`, `source`, `output`, `concepts`, `pages`, `memories`, `journals`, and `skipped_private`.
- Validation JSON contains `version`, `bundle`, `concepts`, `indexes`, `logs`, `errors`, and `warnings`. Every diagnostic has `path` and `message`.
- `brain validate` returns `0` when there are warnings but no errors, and `1` for a non-conformant bundle. Registry, filesystem, collision, or export failures also return a nonzero status.

## Deliberate boundaries

- OKF is an interchange format, not Makakoo's canonical store.
- No Google Cloud account, SDK, service, or model is required.
- OKF imports are read-only enrichment.
- ACLs, encryption, remote synchronization, and conflict resolution are outside OKF v0.1 and outside this command.
