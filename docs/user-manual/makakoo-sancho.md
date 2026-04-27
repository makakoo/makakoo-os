# `makakoo sancho` — CLI reference

SANCHO is the proactive task engine that runs inside the Makakoo daemon.
Plugins register tasks (Python callables with a cadence) into SANCHO at
startup; SANCHO fires each task on its schedule and writes a journal line
per run. Mascots (Pixel, Cinder, Ziggy, Glimmer, Olibia) are SANCHO tasks.
Memory consolidation, Brain sync, perms expiry cleanup, and GYM hypotheses
are all SANCHO tasks.

`makakoo sancho` exposes two controls: check what tasks are registered and
how they last ran, or force a tick immediately instead of waiting for the
next scheduled window.

## Subcommand overview

| Subcommand | Purpose |
|---|---|
| `sancho status` | Table of every registered task: name, cadence, last-run timestamp, last result (ok / failed / skipped). |
| `sancho tick` | Run every eligible task exactly once right now. Returns when all tasks complete. |

## Key use patterns

### Check which tasks are registered and when they last ran

```sh
makakoo sancho status

# example output:
#   TASK                          CADENCE   LAST RUN         RESULT
#   perms_purge_tick              15m       2m ago           ok
#   mascot_pixel_doctor           2h        1h 47m ago       ok
#   mascot_cinder_sentinel        4h        3h 12m ago       ok
#   mascot_ziggy_doctor           daily     8h 03m ago       ok
#   mascot_glimmer_garden         daily     1h 41m ago       ok
#   mascot_olibia_weekly          weekly    5d ago           ok
#   brain_dream_consolidate       6h        4h 22m ago       ok
```

### Force a tick (e.g. right after adding a new plugin)

```sh
# runs all eligible tasks now; useful when you can't wait for the next cadence
makakoo sancho tick
```

## Task eligibility

A task is eligible for a tick when its last-run timestamp is older than its
cadence, or when it has never run. Tasks that have already run within their
cadence window are skipped (logged as `skipped`). `sancho tick` does not
bypass cadence guards — it just runs the eligible set immediately rather
than waiting for the daemon timer.

## Adding tasks (plugin authors)

Register a task in `plugin.toml`:

```toml
[[sancho_tasks]]
name   = "my_plugin_tick"
fn     = "my_plugin.tasks:run_tick"
every  = "30m"
```

The daemon picks up new task registrations on the next restart. Running
`makakoo daemon uninstall && makakoo daemon install` applies the change
without a full reinstall.

## Related commands

- [`makakoo-daemon.md`](makakoo-daemon.md) — the daemon that hosts SANCHO
- [`makakoo-plugin.md`](makakoo-plugin.md) — plugins register SANCHO tasks
- [`../mascots/index.md`](../mascots/) — mascots that run as SANCHO tasks
- [`../concepts/sancho.md`](../concepts/sancho.md) — SANCHO architecture deep-dive

## Common gotcha

**A task shows `failed` in `sancho status` but no error is visible.**
SANCHO captures each task's exception and writes it to the daemon log, not
to stdout. Run `makakoo daemon logs -l 100` and search for the task name to
find the traceback. The most common cause is a missing Python dependency that
was removed when a plugin was updated — `makakoo plugin info <plugin>` will
show the recorded entrypoint path and let you verify the file exists.
