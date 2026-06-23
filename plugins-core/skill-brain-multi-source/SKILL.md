---
name: brain-multi-source
description: Use this skill whenever the user asks to connect, register, add, or inspect an Obsidian vault, Logseq graph, or plain markdown folder as Makakoo Brain enrichment. Trigger phrases include "connect brain to obsidian", "add my obsidian vault", "use my logseq graph", "connect my notes", "register a vault", "where does harvey save notes", "list brain sources", "what vaults am I using". Routes through the bundled `brain_cli.py` helper (list / add / remove / sync / init) to edit `$MAKAKOO_HOME/config/brain_sources.json`. NEVER edit the config by hand; always go through the helper so validation and atomic writes apply. A future Rust `makakoo brain` wrapper may expose the same commands, but do not promise it until it exists in `makakoo --help`.
---

# Brain Multi-Source

The Makakoo Brain stays canonical at `$MAKAKOO_HOME/data/Brain`. This skill registers optional enrichment sources — Obsidian vaults, Logseq graphs, and plain markdown folders — so Superbrain can index them with source labels, extracted tags/aliases, and Obsidian Canvas graph hints. It does not replace the Brain or move normal journal writes.

## When to use this skill

Trigger phrases (match any):

- "connect brain to obsidian" / "connect my obsidian vault"
- "add my obsidian vault at <path>"
- "use my logseq graph" / "point harvey at my logseq"
- "connect my notes folder"
- "register a vault"
- "where does harvey save notes" / "what vault is default"
- "list brain sources" / "what vaults am I using"
- "remove the obsidian vault"
- "setup my brain" (trigger the first-run picker)

## How to run

The canonical CLI is at:

```
python3 ~/makakoo-os/plugins-core/skill-brain-multi-source/src/brain_cli.py <subcommand>
```

Or the runtime-installed copy (same contents):

```
python3 ~/MAKAKOO/plugins/skill-brain-multi-source/src/brain_cli.py <subcommand>
```

### Subcommands

```bash
# List every registered source + the canonical Brain
python3 .../brain_cli.py list
python3 .../brain_cli.py list --json    # machine-readable

# Register a new source
python3 .../brain_cli.py add <name> <type> <path> [--writable|--read-only]
#   types: logseq | obsidian | plain
#   examples:
python3 .../brain_cli.py add personal obsidian ~/Documents/MyVault          # enrichment, read-only default
python3 .../brain_cli.py add notes plain ~/scratch-notes --writable        # explicit writes

# Unregister (refuses the canonical source)
python3 .../brain_cli.py remove <name>

# Walk a source and report doc count + mtime range (dry, no DB writes)
python3 .../brain_cli.py sync --name <name>
python3 .../brain_cli.py sync              # all sources

# Interactive first-run wizard — asks about Obsidian, plain folder, default
python3 .../brain_cli.py init
```

### Common flows

**FIRST — always disambiguate before reaching for `add`:**

When a user says "connect brain to obsidian" / "use obsidian with harvey" / anything similar, there are **two completely different scenarios** and the right answer depends on which one they mean. Ask before acting:

> *"Two options — which one do you want?*
> *(A) Use Obsidian as a UI on top of Harvey's existing Brain. No config change, no separate vault. You just 'Open folder as vault' in Obsidian and point at `$MAKAKOO_HOME/data/Brain/`. Same files, Obsidian UX.*
> *(B) You already have a separate Obsidian vault (personal notes, work stuff, etc.) and want Harvey to ALSO read from it alongside the Brain. This registers the vault as a labeled enrichment source. The canonical Brain stays `$MAKAKOO_HOME/data/Brain`."*

**Scenario A is almost always what the user wants** when they own a single machine and just want a nicer editor over their existing notes. No CLI call needed — they open Obsidian, choose "Open folder as vault", point at `$MAKAKOO_HOME/data/Brain/`. Done. Tell them that and stop.

Caveat to mention for Scenario A: the existing Brain uses Logseq outliner format (every line starts with `- `). Obsidian renders it fine, but new notes they type in Obsidian will be flat markdown. Mixed format in the same dir. Either accept the mix, or run a one-time migration to flatten the journals.

**Only if they explicitly say "separate vault" / name a different path → Scenario B:**

1. Ask for the vault path (or offer to auto-detect common locations: `~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`).
2. Run: `python3 .../brain_cli.py add <chosen-name> obsidian <path>` (default name: `obsidian` or `personal`).
3. Confirm with `list` and show the user the new registry state.
4. Keep it as enrichment. Do not make a separate vault the Brain.

**User says "setup my brain" or "first-run":**

1. Run the picker interactively: `python3 .../brain_cli.py init`.
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

- **Never edit `$MAKAKOO_HOME/config/brain_sources.json` directly.** Always route through the CLI — it does atomic writes and enforces the default-source guard.
- **Never delete a registered path on disk** just because the user unregisters a source. Removing a source from the registry = stop indexing. Files stay where they are.
- **Always `list` before destructive subcommands** so the user can see state.
- **First-run picker is optional**, not a blocker. If the user says "skip" or "no", leave them with the canonical Makakoo Brain source only.
- **NEVER call `add obsidian <path>` without first disambiguating Scenario A vs B** (see "Common flows" above). Assuming "connect obsidian" always means a separate vault is the single most common miss — the Brain directory is already plain markdown and opens in Obsidian with zero config. Ask which scenario before acting.
- **Never say a separate Obsidian vault replaces the Brain.** It is enrichment context, read-only by default, and normal journal writes stay canonical.

## Underlying plugin

This skill is the user-facing documentation for `plugins-core/skill-brain-multi-source/`. The plugin ships:

- `brain_source.py` — adapter classes (LogseqSource / ObsidianSource / PlainMarkdownSource)
- `config.py` — JSON config loader + atomic writer
- `brain_cli.py` — what this skill drives
- `picker.py` — interactive `init` wizard
- `sancho_ingest.py` — 30-min SANCHO task that walks every registered source

The enrichment sprint that hardened the canonical-vs-enrichment contract: `development/sprints/queued/SPRINT-BRAIN-OBSIDIAN-ENRICHMENT-2026-06-23/SPRINT.md`.

## Known gaps (don't promise these)

- Obsidian Canvas enrichment currently records document-to-canvas-target graph hints, not a full node-to-node visual graph.
- Cross-source wikilinks (`[[vault:page]]` syntax) are not yet resolved — wikilinks work within-source only.
- The UserPromptSubmit memory recall hook is grep-only (keyword match on MEMORY.md). Semantic / vector recall is queued.
