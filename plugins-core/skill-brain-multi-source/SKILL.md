---
name: brain-multi-source
description: Use this skill whenever the user asks to connect, register, add, or change a knowledge vault (Logseq, Obsidian, or a plain markdown folder) that Harvey should read from or write into. Trigger phrases include "connect brain to obsidian", "add my obsidian vault", "use my logseq graph", "connect my notes", "register a vault", "switch default brain", "where does harvey save notes", "list brain sources", "what vaults am I using". Routes through the `makakoo brain` CLI (list / add / remove / set-default / sync / init) to edit `$MAKAKOO_HOME/config/brain_sources.json`. NEVER edit the config by hand; always go through the CLI so the picker's validation and atomic writes apply.
---

# Brain Multi-Source

Harvey's brain can span multiple knowledge substrates at once — Logseq graph, Obsidian vault, plain markdown folder. This skill manages which substrates are registered and which is the write default.

## When to use this skill

Trigger phrases (match any):

- "connect brain to obsidian" / "connect my obsidian vault"
- "add my obsidian vault at <path>"
- "use my logseq graph" / "point harvey at my logseq"
- "connect my notes folder"
- "register a vault"
- "switch default brain to X" / "make X the default"
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
# List every registered source + the current default
python3 .../brain_cli.py list
python3 .../brain_cli.py list --json    # machine-readable

# Register a new source
python3 .../brain_cli.py add <name> <type> <path> [--read-only]
#   types: logseq | obsidian | plain
#   examples:
python3 .../brain_cli.py add personal obsidian ~/Documents/MyVault
python3 .../brain_cli.py add notes plain ~/scratch-notes --read-only

# Unregister (refuses the default source; change default first)
python3 .../brain_cli.py remove <name>

# Change write default
python3 .../brain_cli.py set-default <name>

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
> *(B) You already have a separate Obsidian vault (personal notes, work stuff, etc.) and want Harvey to ALSO read from it alongside the Brain. Two substrates merged — this registers the vault in the config."*

**Scenario A is almost always what the user wants** when they own a single machine and just want a nicer editor over their existing notes. No CLI call needed — they open Obsidian, choose "Open folder as vault", point at `$MAKAKOO_HOME/data/Brain/`. Done. Tell them that and stop.

Caveat to mention for Scenario A: the existing Brain uses Logseq outliner format (every line starts with `- `). Obsidian renders it fine, but new notes they type in Obsidian will be flat markdown. Mixed format in the same dir. Either accept the mix, or run a one-time migration to flatten the journals.

**Only if they explicitly say "separate vault" / name a different path → Scenario B:**

1. Ask for the vault path (or offer to auto-detect common locations: `~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`).
2. Run: `python3 .../brain_cli.py add <chosen-name> obsidian <path>` (default name: `obsidian` or `personal`).
3. Confirm with `list` and show the user the new registry state.
4. Ask whether they want this to be the write default. If yes, `set-default`.

**User says "setup my brain" or "first-run":**

1. Run the picker interactively: `python3 .../brain_cli.py init`.
2. What the picker does, in order:
   - Prints a banner noting that if the user just wants Obsidian as a UI over the existing Brain, no registration is needed (Scenario A — open the Brain dir as a vault).
   - Seeds the default Logseq source if missing (baseline guarantee, outside the batched flow).
   - Asks about a separate Obsidian vault (auto-detects common paths: `~/Documents/Obsidian Vault`, `~/Documents/obsidian`, `~/Obsidian`).
   - Asks about any other plain markdown folder (name + writable toggle).
   - If >1 source will exist, asks whether to change the write-default.
   - **Shows a "Pending changes" summary** listing every registration + default change.
   - Asks "Save these changes? [Y/n]" — nothing is persisted until this is confirmed.
   - On confirmation, commits each add + the default change, then **dry-walks each new source and prints doc counts** so the user sees the registration took.
3. It's optional throughout. Empty answers skip a prompt. Ctrl-C at any point leaves the config file untouched (other than the baseline default seed, which runs first).
4. `--non-interactive` flag skips all prompts and leaves the user with the default Logseq source only — use this in CI or install automation.

**User says "where does harvey save notes":**

1. Run `list` and report the default source + its path.

**User says "remove my obsidian":**

1. Run `list` first to show what would change.
2. If the target IS the default, refuse and prompt to `set-default` another source first.
3. Then `remove`.

## Critical rules

- **Never edit `$MAKAKOO_HOME/config/brain_sources.json` directly.** Always route through the CLI — it does atomic writes and enforces the default-source guard.
- **Never delete a registered path on disk** just because the user unregisters a source. Removing a source from the registry = stop indexing. Files stay where they are.
- **Always `list` before destructive subcommands** so the user can see state.
- **First-run picker is optional**, not a blocker. If the user says "skip" or "no", leave them with the default Logseq source only.
- **NEVER call `add obsidian <path>` without first disambiguating Scenario A vs B** (see "Common flows" above). Assuming "connect obsidian" always means a separate vault is the single most common miss — the Brain directory is already plain markdown and opens in Obsidian with zero config. Ask which scenario before acting.

## Underlying plugin

This skill is the user-facing documentation for `plugins-core/skill-brain-multi-source/`. The plugin ships:

- `brain_source.py` — adapter classes (LogseqSource / ObsidianSource / PlainMarkdownSource)
- `config.py` — JSON config loader + atomic writer
- `brain_cli.py` — what this skill drives
- `picker.py` — interactive `init` wizard
- `sancho_ingest.py` — 30-min SANCHO task that walks every registered source

The sprint that shipped it: `development/sprints/SPRINT-BRAIN-MEMORY-UNIFIED/SPRINT.md`.

## Known gaps (don't promise these)

- Registering a non-default source currently marks it in config and the SANCHO task walks it, but the Rust daemon's file-watcher only auto-embeds the default Logseq source today. Full SQLite ingest for non-default sources is queued for the v0.2 Phase C sprint.
- Cross-source wikilinks (`[[vault:page]]` syntax) are not yet resolved — wikilinks work within-source only.
- The UserPromptSubmit memory recall hook is grep-only (keyword match on MEMORY.md). Semantic / vector recall is queued.
