---
name: brain-multi-source
description: Use this skill whenever the user asks to connect, register, add, inspect, import, export, or validate a Brain source or Open Knowledge Format bundle. Supports Logseq, Obsidian, plain Markdown, and read-only OKF v0.1 enrichment through `makakoo brain`. Never edit `brain_sources.json` directly and never publish an export without explicit approval.
---

# Brain Multi-Source

The Makakoo Brain stays canonical at `$MAKAKOO_HOME/data/Brain`. This skill registers optional enrichment sources and produces portable OKF v0.1 bundles. OKF is an interchange boundary, never a second canonical Brain.

## When to use this skill

Trigger phrases (match any):

- "connect brain to obsidian" / "connect my obsidian vault"
- "add my obsidian vault at <path>"
- "use my logseq graph" / "point harvey at my logseq"
- "connect my notes folder"
- "register a vault"
- "import this OKF bundle" / "add this knowledge bundle"
- "export the Brain" / "export as OKF" / "validate this OKF bundle"
- "where does harvey save notes" / "what vault is default"
- "list brain sources" / "what vaults am I using"
- "remove the obsidian vault"
- "setup my brain" (trigger the first-run picker)

## How to run

Use the native CLI. The Python helper remains a compatibility surface for the setup picker and SANCHO task.

### Subcommands

```bash
# List every registered source + the canonical Brain
makakoo brain list
makakoo brain list --json

# Register a new source
makakoo brain add <name> <type> <path> [--read-only|--writable]
#   types: logseq | obsidian | plain | okf
#   examples:
makakoo brain add personal obsidian ~/Documents/MyVault --read-only
makakoo brain add catalog okf ~/knowledge/catalog
makakoo brain add working-notes plain ~/Documents/working-notes --writable

# Unregister (refuses the canonical source)
makakoo brain remove <name>

# Export pages + durable auto-memory. Journals are opt-in.
makakoo brain export --format okf --source default --out ~/exports/makakoo-okf
makakoo brain export --out ~/exports/public-okf --public
makakoo brain export --out ~/exports/full-okf --include-journals

# Validate before registering or sharing a bundle.
makakoo brain validate ~/exports/makakoo-okf
makakoo brain validate ~/exports/makakoo-okf --json

# Interactive source setup remains in the setup wizard.
makakoo setup brain
```

### Common flows

#### Import an OKF bundle

1. Run `makakoo brain validate <bundle> --json` and inspect both errors and warnings.
2. If validation is conformant, run `makakoo brain add <name> okf <bundle>`.
3. Run `makakoo sync`, then confirm retrieval with `makakoo search <known-term>`.
4. Keep the source read-only. Never copy imported concepts into the canonical Brain unless the user explicitly asks for a curated migration.

#### Export or share an OKF bundle

1. Confirm which registered source to export with `makakoo brain list`.
2. Export to a local directory. Canonical pages and durable auto-memory are included by default; journals require explicit `--include-journals`.
3. Validate the output with `makakoo brain validate <output>`.
4. For an intended public bundle, use `--public`, report how many private documents were skipped, and inspect the result.
5. Never upload, publish, email, or otherwise share the directory without explicit approval.

**FIRST — always disambiguate before reaching for `add`:**

When a user says "connect brain to obsidian" / "use obsidian with harvey" / anything similar, there are **two completely different scenarios** and the right answer depends on which one they mean. Ask before acting:

> *"Two options — which one do you want?*
> *(A) Use Obsidian as a UI on top of Harvey's existing Brain. No config change, no separate vault. You just 'Open folder as vault' in Obsidian and point at `$MAKAKOO_HOME/data/Brain/`. Same files, Obsidian UX.*
> *(B) You already have a separate Obsidian vault (personal notes, work stuff, etc.) and want Harvey to ALSO read from it alongside the Brain. This registers the vault as a labeled enrichment source. The canonical Brain stays `$MAKAKOO_HOME/data/Brain`."*

**Scenario A is almost always what the user wants** when they own a single machine and just want a nicer editor over their existing notes. No CLI call needed — they open Obsidian, choose "Open folder as vault", point at `$MAKAKOO_HOME/data/Brain/`. Done. Tell them that and stop.

Caveat to mention for Scenario A: the existing Brain uses Logseq outliner format (every line starts with `- `). Obsidian renders it fine, but new notes they type in Obsidian will be flat markdown. Mixed format in the same dir. Either accept the mix, or run a one-time migration to flatten the journals.

**Only if they explicitly say "separate vault" / name a different path → Scenario B:**

1. Ask for the vault path (or offer to auto-detect common locations: `~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`).
2. Run: `makakoo brain add <chosen-name> obsidian <path> --read-only` (default name: `obsidian` or `personal`).
3. Confirm with `list` and show the user the new registry state.
4. Keep it as enrichment. Do not make a separate vault the Brain.

**User says "setup my brain" or "first-run":**

1. Run the picker interactively: `makakoo setup brain`.
2. What the picker does, in order:
   - Prints the canonical default Brain folder: `$MAKAKOO_HOME/data/Brain/`.
   - Detects the Obsidian app first. If missing, offers to install it through Homebrew, Flatpak, or winget when available. The default is No; declining install skips Obsidian setup for this run.
   - If Obsidian is installed (or was installed successfully), confirms that the default Brain folder is the Obsidian editor/vault folder. No extra registration is needed for this path.
   - Optionally asks about an additional separate Obsidian vault (auto-detects common paths: `~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`).
   - Asks about any other plain markdown folder (name + writable toggle).
   - **Shows a "Pending changes" summary** listing every enrichment registration.
   - Asks "Save these changes? [Y/n]" — nothing is persisted until this is confirmed.
   - On confirmation, commits each add, then **dry-walks each new source and prints doc counts** so the user sees the registration took.
3. It's optional throughout. Empty answers accept the visible default. Ctrl-C at any point leaves pending source registrations untouched.
4. `--non-interactive` flag skips all prompts and writes the default Makakoo Brain source only — use this in CI or install automation.

**User says "where does harvey save notes":**

1. Run `list` and report the canonical Brain plus enrichment sources.

**User says "remove my obsidian":**

1. Run `list` first to show what would change.
2. If the target is the canonical Brain, refuse.
3. Then `remove`.

## Critical rules

- **Never edit `$MAKAKOO_HOME/config/brain_sources.json` directly.** Always route through the CLI. Rust and Python share one cross-process lock and owned crash-recovery protocol; unowned temp/backup collisions fail closed.
- **Never delete a registered path on disk** just because the user unregisters a source. Removing a source from the registry = stop indexing. Files stay where they are.
- **Always `list` before destructive subcommands** so the user can see state.
- **First-run picker is optional**, not a blocker. If the user says "skip" or "no", leave them with the canonical Makakoo Brain source only.
- **NEVER call `add obsidian <path>` without first disambiguating Scenario A vs B** (see "Common flows" above). Assuming "connect obsidian" always means a separate vault is the single most common miss — the Brain directory is already plain markdown and opens in Obsidian with zero config. Ask which scenario before acting.
- **Never say a separate Obsidian vault replaces the Brain.** It is enrichment context, read-only by default, and normal journal writes stay canonical.
- **OKF sources are always read-only enrichment.** Validate them before registration; `makakoo brain add ... okf ...` enforces both rules.
- **Never publish automatically.** Export writes only to the requested local directory. `--public` requires `visibility: public` and refuses credential-shaped content, but publication still needs explicit approval.
- **Journals are excluded by default.** Only pass `--include-journals` when the user explicitly wants chronological history in the bundle.

## Underlying plugin

This skill is the user-facing documentation for `plugins-core/skill-brain-multi-source/`. The plugin ships:

- `brain_source.py` — adapter classes (LogseqSource / ObsidianSource / PlainMarkdownSource / OkfSource)
- `config.py` — JSON config loader + cross-process locked, crash-recoverable writer
- `brain_cli.py` — compatibility helper used by setup and SANCHO; native user operations go through `makakoo brain`
- `picker.py` — interactive `init` wizard
- `sancho_ingest.py` — 30-min SANCHO task that walks every registered source

The enrichment sprint that hardened the canonical-vs-enrichment contract: `development/sprints/queued/SPRINT-BRAIN-OBSIDIAN-ENRICHMENT-2026-06-23/SPRINT.md`.

## Known gaps (don't promise these)

- Obsidian Canvas enrichment currently records document-to-canvas-target graph hints, not a full node-to-node visual graph.
- Cross-source wikilinks (`[[vault:page]]` syntax) are not yet resolved — wikilinks work within-source only.
- OKF v0.1 does not define ACLs, encryption, conflict resolution, or typed link predicates. Makakoo does not invent them in the interchange layer.
- The UserPromptSubmit memory recall hook is grep-only (keyword match on MEMORY.md). Semantic / vector recall is queued.
