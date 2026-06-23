# `makakoo setup` — interactive wizard

`makakoo setup` walks you through every one-time configuration step: naming
the assistant, choosing auto/manual Makakoo OS updates, registering brain
sources, bootstrapping the blessed CLI agent (pi), terminal (Ghostty on macOS),
and Lope validator ensemble, picking a primary LLM adapter, and infecting local
CLI hosts with the Makakoo bootstrap block.

Fresh installs are offered the wizard automatically at the end of
`makakoo install`. You can also run it at any time — or re-run a single
section when something changes.

## Commands

```bash
makakoo setup                 # walk every section in order
makakoo setup <section>       # run one section
makakoo setup --only A,B      # run only named sections
makakoo setup --skip A,B      # run every section except the named ones
makakoo setup --non-interactive
                              # print current state and exit 0; no prompts
makakoo setup --reset         # wipe state file; re-ask every section
makakoo setup --force         # re-run the persona section even when a
                              # persona.json already exists (other
                              # sections are idempotent by default and
                              # ignore this flag)
```

When stdin is not a TTY (CI, pipes, redirected input), the wizard
automatically prints current state and exits 0 without prompting. The
same effect can be forced with `--non-interactive`.

## Sections

| Section          | What it does                                                                 | How status is decided                            |
|------------------|-----------------------------------------------------------------------------|--------------------------------------------------|
| `persona`        | Names your assistant + optional user name + pronoun + voice default; seeds persona registry/context when installed. | `config/persona.json` exists → already-satisfied |
| `updates`        | Chooses Makakoo OS update mode. `auto` runs `makakoo update --reinfect` from SANCHO every 24h; `manual` only checks when the user runs it. | `config/updates.toml` has `mode = "auto"` or `mode = "manual"` |
| `brain`          | Seeds the canonical Makakoo Brain at `$MAKAKOO_HOME/data/Brain`, checks whether Obsidian is installed, offers to install it when supported, and treats separate Obsidian/plain-markdown vaults as optional enrichment sources. | `config/brain_sources.json` has a valid canonical Brain source |
| `cli-agent`      | Installs pi (`@mariozechner/pi-coding-agent`) via `npm install -g`.          | `pi` is on `$PATH`                                |
| `terminal`       | Installs Ghostty via `brew install --cask ghostty`. **macOS only.**           | `brew list --cask ghostty` exits 0                |
| `lope`           | Offers Makakoo's optional in-house Lope validator ensemble. It clones `traylinx/lope` to `~/.lope` or `$LOPE_HOME`, runs Lope's installer, and registers its skills/commands into detected AI CLI hosts. Defaults to No because it runs a remote installer. | `lope` is on `$PATH` or `~/.lope/lope/cli.py` exists |
| `model-provider` | Writes `~/.makakoo/primary_adapter.toml` naming the default routing adapter. If no adapters are registered yet, it installs the bundled `switchailocal` adapter and selects it as the fresh-install default. | file exists and points to a registered adapter    |
| `infect`         | Thin wrapper over `makakoo infect` — writes the bootstrap block to every    | `makakoo infect --verify` exits 0                 |
|                  | detected CLI host config (NOT your shell dotfiles).                         |                                                  |

Each section is idempotent. The wizard detects the current state before
every run; if the desired state is already satisfied it prints a one-line
status and moves on. Only the `persona` section's `--force` flag triggers
re-prompting when the state is already present.

Every `Y/n/s` prompt accepts:

- `y` / `yes` / Enter on a Y-default prompt → proceed
- `n` / `no` → decline this run (re-ask on the next `setup`)
- `s` / `skip` → record as skipped so the wizard doesn't re-ask

Non-yes/no prompts also show their default in brackets. Pressing Enter accepts
that default, for example persona name `[1]`, pronoun `[they]`, voice `[1]`,
or model-provider `[skip]`.


### Brain and Obsidian

The Brain section always seeds the canonical Makakoo Brain at
`$MAKAKOO_HOME/data/Brain/`. If you use Obsidian, there are two paths:

- Open the canonical Brain folder as an Obsidian vault. No Brain replacement, no
  source-of-truth change.
- Register a separate Obsidian vault so Makakoo can index it as labeled
  enrichment context.

The picker checks for the Obsidian app before separate-vault prompts. If the
app is missing, it offers to install Obsidian with the available package
manager: Homebrew on macOS, Flatpak on Linux, or winget on Windows. The
default answer is No. Declining install skips Obsidian setup for this run; you
can install later and open `$MAKAKOO_HOME/data/Brain/` directly. Normal
Makakoo writes still go to the canonical Brain.

### Lope

Lope is optional, but it is one of the best things to install on a Makakoo
machine. It is the in-house validator ensemble: one CLI drafts, the rest
review independently. Use it for `review`, `vote`, `compare`, negotiated
plans, and validator-in-the-loop sprints. It is especially useful before a
PR, a migration, a release, or any decision where one model should not be
the only judge.

The setup section defaults to No because it clones `https://github.com/traylinx/lope`
and executes Lope's installer. Say Yes if you want the full multi-CLI
review loop. Say Skip if you want Makakoo to stop asking until `--reset`.

### Headroom

Headroom is not a wizard section. It ships in the default core distro as
`tool-headroom`. Fresh installs try `headroom-ai[mcp]` first, then fall
back to the Docker-native wrapper when Python wheels are unavailable. Use
`headroom mcp status` after install if you want to confirm the MCP server
was registered for your host.

## State file

`$MAKAKOO_HOME/state/makakoo-setup/completed.json` records per-section
status. Atomically written (tmp + rename). Corruption is tolerated:
unreadable or version-mismatched state reverts to "all not-started" on
load; nothing crashes.

Sample contents after a successful full run:

```json
{
  "version": 1,
  "sections": {
    "persona":        { "status": "Completed", "at": "2026-04-23T17:30:00Z" },
    "updates":        { "status": "Completed", "at": "2026-04-23T17:30:30Z" },
    "brain":          { "status": "Skipped",   "at": "2026-04-23T17:31:12Z" },
    "cli-agent":      { "status": "Completed", "at": "2026-04-23T17:32:48Z" },
    "terminal":       { "status": "Completed", "at": "2026-04-23T17:33:19Z" },
    "lope":           { "status": "Completed", "at": "2026-04-23T17:33:45Z" },
    "model-provider": { "status": "Completed", "at": "2026-04-23T17:34:02Z" },
    "infect":         { "status": "Completed", "at": "2026-04-23T17:34:55Z" }
  }
}
```

## Install hand-off

`makakoo install` ends with: `"Run the setup wizard now? [Y/n]"`. Answering
yes invokes `makakoo setup` in the same process. Pass `--no-setup` to skip
the prompt entirely (e.g. in CI). Non-TTY installs never prompt regardless
of the flag.

## Design notes

- **Baseline vs wizard split.** Both `makakoo install` and `makakoo setup`
  exist on purpose: install is non-interactive and declarative (distro
  + daemon + infect with no prompts), setup is interactive and walks the
  one-time configuration surface. You can re-run setup at any time.
- **Never auto-install without consent.** `npm install -g` and `brew
  install --cask` are only invoked after an explicit `Y`. If the required
  tool is missing (no npm, no brew), the section fails with a clear hint
  rather than silently doing nothing.
- **Sancho tasks preserve the 24h update cadence.** `sancho-task-makakoo-update`
  keeps Makakoo OS current when update mode is explicitly `auto` (the fresh
  setup default). Missing config stays idle so existing installs do not silently
  opt into unattended self-updates.
  The `sancho-task-cli-pi` and `sancho-task-cli-ghostty` plugins keep installed
  tools current; they nag (via the Brain journal) but never install missing
  tools without the wizard.
- **Lope is recommended, not mandatory.** The Lope section explains the value
  of multi-CLI validator review and installs only after explicit consent.
- **Persona is preserved across refactors.** The old `makakoo setup`
  was a one-shot persona picker; it now lives as the `persona` section
  inside this dispatcher. Fresh core installs also seed
  `config/persona_registry.json` and `config/persona_context.md` so
  every infected CLI remembers user, companion, and channel names.
- **Fresh installs get a model route.** The `model-provider` section
  bootstraps the bundled `switchailocal` adapter when the registry is
  empty, then writes it as the primary adapter. Users still need to set
  any required upstream key, for example `makakoo secret set AIL_API_KEY`,
  before LLM calls can succeed.
