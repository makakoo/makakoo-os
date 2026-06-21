# SANCHO — proactive task engine

SANCHO is Makakoo's background scheduler. It runs inside the Makakoo daemon,
loads native tasks plus plugin-declared tasks, applies cadence gates, executes
eligible work, and writes durable results to logs and the Brain journal.

Use SANCHO for maintenance work that should happen without a human prompt:
Brain sync, memory consolidation, plugin update checks, Makakoo auto-update,
watchdogs, mascot/GYM loops, and any plugin task declared in `plugin.toml`.

## CLI surface

Current public commands are deliberately small:

| Command | What it does |
|---|---|
| `makakoo sancho status` | Show registered tasks, cadence, last run, and result state. |
| `makakoo sancho tick` | Run every task that is eligible right now. Tasks still respect their cadence gates. |

There is no `makakoo sancho run <task>` or `makakoo sancho history` command in
the current kernel. Check `makakoo daemon logs` and today's Brain journal when
you need run evidence.

## Quick checks

```bash
makakoo daemon status      # daemon host is up
makakoo sancho status      # task registry + last-run state
makakoo sancho tick        # force one due-task pass now
makakoo daemon logs -l 100 # traceback/details if a task failed
```

## How tasks are registered

Plugins register tasks in `plugin.toml`:

```toml
[[sancho_tasks]]
name  = "my_plugin_tick"
fn    = "my_plugin.tasks:run_tick"
every = "30m"
```

The daemon reads plugin manifests on startup. After adding or updating a plugin,
restart the daemon so the registry refreshes:

```bash
makakoo daemon restart
makakoo sancho status
```

## Cadence semantics

`sancho tick` is not a bypass. A task runs only when:

- the task is registered and enabled,
- the last successful/attempted run is older than its cadence, or it has never run,
- the task's own gates pass, if it declares any.

A fresh tick may therefore show many skipped tasks. That is normal.

## Update tasks

Fresh setup can write `$MAKAKOO_HOME/config/updates.toml` with:

```toml
mode = "auto"
```

When this exists and is `auto`, the `sancho-task-makakoo-update` plugin runs
`makakoo update --reinfect` on a 24h cadence. If the file is missing, the task
stays idle so old installs do not silently opt into unattended updates. If the
mode is `manual`, SANCHO can remind you, but you run the update yourself.

## Troubleshooting

**A task is missing from `sancho status`.**
Restart the daemon after installing or updating the plugin:

```bash
makakoo daemon restart
makakoo sancho status
```

**A task is failed.**
Read daemon logs and search today's journal:

```bash
makakoo daemon logs -l 200
cat "$MAKAKOO_HOME/data/Brain/journals/$(date +%Y_%m_%d).md"
```

**I need the Brain index rebuilt now.**
Use the Brain sync command, not a SANCHO task name:

```bash
makakoo sync --force
```

## Related docs

- [`makakoo sancho`](../user-manual/makakoo-sancho.md) — command reference
- [`makakoo daemon`](../user-manual/makakoo-daemon.md) — daemon lifecycle
- [`makakoo update`](../user-manual/makakoo-update.md) — self-update command
- [Plugin guide](../plugins/) — task declaration in plugin manifests
