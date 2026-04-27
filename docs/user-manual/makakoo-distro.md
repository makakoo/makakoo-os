# `makakoo distro` — CLI reference

A distro is a named, pinned bundle of plugins. Instead of installing
plugins one by one, you pick a distro and get a coherent set with
blake3-pinned versions. This is also how you reproduce an exact working
environment on a second machine: `distro save` serializes what you have;
`distro install --from` replays it.

Shipped distros: `minimal` (kernel only), `core` (default, general-purpose),
`sebastian` (the owner's personal stack), `creator` (writing + research),
`trader` (arbitrage + Polymarket).

## Subcommand overview

| Subcommand | Purpose |
|---|---|
| `distro list` | List every distro file shipped under `distros/` plus the currently active distro. |
| `distro install [name] [--from file] [--dry-run] [--yes]` | Install a named distro or a local file. `--dry-run` prints the plan without installing. |
| `distro save <name> [--out file] [--force] [--include-disabled]` | Serialize the current live plugin set into a pinned distro TOML. |

## Key use patterns

### Switch to the creator distro

```sh
# preview the plugin list before committing
makakoo distro install creator --dry-run

# install (prompts for confirmation once)
makakoo distro install creator

# skip the prompt in CI
makakoo distro install creator --yes
```

### Snapshot your current setup and replay it elsewhere

```sh
# on machine A: save the live plugin set
makakoo distro save my-stack --out ~/my-stack.toml

# on machine B: replay it exactly (all versions pinned by blake3)
makakoo distro install --from ~/my-stack.toml --yes
```

## Distro file format

A distro TOML lists plugins by name and optional version/blake3 pin,
plus optional `[include]` references to other distros. Run
`makakoo distro install core --dry-run` to see a resolved flat list.

## Related commands

- [`makakoo-plugin.md`](makakoo-plugin.md) — install individual plugins not in any distro
- [`makakoo-daemon.md`](makakoo-daemon.md) — restart the daemon after a distro switch
- [`../plugins/index.md`](../plugins/) — plugin authoring and the plugin ecosystem
- [`setup-wizard.md`](setup-wizard.md) — the setup wizard uses `core` distro by default

## Common gotcha

**`distro install` hangs or fails mid-way through a large distro.**
Each plugin in the distro is installed serially. If one plugin's git fetch
times out (e.g. a private repo with an expired token), the whole install
stops at that entry. Check the partial output to find the failing plugin
name, fix the source or remove it from your distro file, and re-run —
`distro install` is resumable because already-installed plugins are
detected and skipped.
