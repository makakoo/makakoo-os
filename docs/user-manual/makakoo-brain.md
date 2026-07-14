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
```

Supported types are `logseq`, `obsidian`, `plain`, and `okf`. An OKF source must validate before registration and is always read-only.

Unregistering does not delete source files:

```bash
makakoo brain remove catalog
```

Run `makakoo sync` after source changes to refresh FTS5 and the entity graph.

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

Export is local-only and atomic. It never uploads or publishes anything. The exporter:

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

## Validate a bundle

```bash
makakoo brain validate ~/exports/makakoo-okf
makakoo brain validate ~/exports/makakoo-okf --json
```

Validation checks UTF-8 Markdown, YAML frontmatter, required non-empty `type`, reserved `index.md` and `log.md` structure, version declarations, symlinks, and internal links. Broken internal links are warnings because OKF v0.1 explicitly permits incomplete bundles. Structural violations return exit code 1.

## Deliberate boundaries

- OKF is an interchange format, not Makakoo's canonical store.
- No Google Cloud account, SDK, service, or model is required.
- OKF imports are read-only enrichment.
- ACLs, encryption, remote synchronization, and conflict resolution are outside OKF v0.1 and outside this command.
