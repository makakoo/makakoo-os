# `makakoo setup` — interactive wizard

`makakoo setup` walks you through every one-time configuration step: naming
the assistant, registering brain sources, bootstrapping the blessed CLI
agent (pi) and terminal (Ghostty on macOS), picking a primary LLM adapter,
and infecting local CLI hosts with the Makakoo bootstrap block.

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
| `persona`        | Names your assistant + pronoun + voice default.                              | `config/persona.json` exists → already-satisfied |
| `brain`          | Shells to the existing `skill-brain-multi-source` picker to register vaults.  | `config/brain_sources.json` has ≥1 non-default source |
| `cli-agent`      | Installs pi (`@mariozechner/pi-coding-agent`) via `npm install -g`.          | `pi` is on `$PATH`                                |
| `terminal`       | Installs Ghostty via `brew install --cask ghostty`. **macOS only.**           | `brew list --cask ghostty` exits 0                |
| `model-provider` | Writes `~/.makakoo/primary_adapter.toml` naming the default routing adapter. | file exists and points to a registered adapter    |
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
    "brain":          { "status": "Skipped",   "at": "2026-04-23T17:31:12Z" },
    "cli-agent":      { "status": "Completed", "at": "2026-04-23T17:32:48Z" },
    "terminal":       { "status": "Completed", "at": "2026-04-23T17:33:19Z" },
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
- **Sancho tasks preserve the 24h update cadence.** The `sancho-task-cli-pi`
  and `sancho-task-cli-ghostty` plugins keep installed tools current; they
  nag (via the Brain journal) but never install without the wizard.
- **Persona is preserved across refactors.** The old `makakoo setup`
  was a one-shot persona picker; it now lives as the `persona` section
  inside this dispatcher with bit-for-bit identical prompts.
