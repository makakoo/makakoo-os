# Walkthrough 04 — Watch the Brain grow by itself

## What you'll do

See the list of proactive tasks Makakoo registered for you, trigger them once manually, and observe what the memory layer now knows. No manual edits this time — the Brain populates itself.

**Time:** about 4 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md) completed; at least one invocation of `makakoo sync` or `makakoo search` has run (so the index exists).

## What is SANCHO?

SANCHO is Makakoo's **proactive task engine**. It's a registry of small jobs that run on their own schedules (every 30 min, every hour, nightly, etc.) while you're working. They read from the Brain, maybe call an LLM, write results back into the Brain.

Examples of what native SANCHO tasks do:
- **`journal_compactor`** — takes yesterday's long journal and compresses it into a summary page.
- **`brain_resurface`** — surfaces old journal entries that are still relevant to what you're doing now.
- **`memory_promoter`** — promotes frequently-recalled facts into the curated auto-memory layer.
- **`dream_consolidate`** — runs a nightly memory-consolidation pass.

You don't have to touch any of this. It just runs. But for this walkthrough we'll look at it up close.

## Steps

### 1. See the tasks that are registered

```sh
makakoo sancho status
```

Expected output (the 32 number reflects the `core` distro; other distros differ):

```text
sancho: 32 registered task(s) (11 native + 21 manifest)
(no task has run yet this process)
```

If you've already been using Makakoo for a while, you'll see actual `last_run` timestamps instead of "no task has run yet":

```text
sancho: 32 registered task(s) (11 native + 21 manifest)
journal_compactor        last_run=2026-04-24T09:12:05+02:00  interval=3600s
brain_resurface          last_run=2026-04-24T13:47:22+02:00  interval=1800s
memory_promoter          last_run=2026-04-24T13:47:01+02:00  interval=1800s
dream_consolidate        last_run=2026-04-24T03:00:01+02:00  interval=86400s
...
```

**What to notice:**
- **`native`** tasks are baked into the `makakoo` binary.
- **`manifest`** tasks come from installed plugins that declared `[[sancho.tasks]]` entries in their `plugin.toml`.
- **`interval=1800s`** = every 30 minutes. `86400s` = daily.

### 2. Fire every eligible task exactly once

Normally the daemon does this for you, on schedule. You can also run it manually:

```sh
makakoo sancho tick
```

Expected output (truncated — each task prints a status line):

```text
SANCHO tick — 32 eligible tasks
ok   journal_compactor        0.24s
ok   brain_resurface          1.80s
ok   memory_promoter          0.41s
skip watchdog_infect          (active_hours=[9..22])
fail gym_hypothesize          ModuleNotFoundError: ...
...
```

Each row shows: `<status> <task> <duration> <detail>`:
- **`ok`** — the task ran and exited 0.
- **`skip`** — task was registered but not eligible (outside active hours, or didn't hit its interval since last run).
- **`fail`** — task ran and exited non-zero. Captured in the journal and visible via `makakoo sancho status` on later runs.

> **Don't panic at fails.** A fresh install of a large distro might have a few tasks that fail on first tick because they expect state that doesn't exist yet (no career agent state, no freelance-office config, …). These will go silent as you configure the plugins you actually use, or you can disable them with `makakoo plugin disable <plugin-name>`.

### 3. Look at what the memory layer knows now

```sh
makakoo memory stats
```

Expected output (on an install with some history):

```text
Recall log:
  total entries:        705
  today:                12
  last 7d:              67
  by source:
    anchor_search                    280
    mcp:brain_query                  2
    mcp:brain_search                 43
    mcp:harvey_brain_search          2
    search                           127
    vector                           250

Recall stats (promoter input):
  total content_hashes:              397
  distinct facts surfaced:           201
  last promoter run:                 2026-04-24T13:47:01+02:00

Memory promotions:
  candidates awaiting review:        0
  promoted to auto-memory this week: 3
```

**What each section means:**
- **Recall log** — every time anything (you, a SANCHO task, an infected CLI) pulled something out of the Brain, one row gets appended here. The log is raw signal.
- **Recall stats** — deduplicated signal: per-fact counts used to decide what's worth promoting.
- **Memory promotions** — facts that the `memory_promoter` task thinks should be upgraded from "random journal line" to "durable auto-memory". Reviewed by you (or automatically) and promoted to `~/MAKAKOO/data/auto-memory/`.

On a brand-new install, every number will be 0 or near-zero. That's healthy — there's nothing to remember yet.

### 4. Check what's queued for promotion

```sh
makakoo promotions
```

Expected output when nothing is pending:

```text
(no promotion candidates)
```

When the promoter has candidates ready for you to review, this command lists them with a short summary and a confidence score. Reviewed candidates can be accepted into auto-memory with a separate command (covered in the user-manual page for `makakoo memory`).

### 5. Sync once more so everything the tasks wrote gets indexed

```sh
makakoo sync
```

Expected output similar to walkthrough 02 — some small counts if tasks wrote journal entries.

## What just happened?

- **SANCHO is always running in the background** (via the LaunchAgent you registered in walkthrough 01). This walkthrough let you see its internals by forcing one round manually.
- **Every search, question, and CLI roundtrip is recorded** in the recall log at `~/MAKAKOO/data/makakoo.db`. The promoter reads that log every 30 minutes and nominates facts that are getting recalled a lot — those are the ones worth keeping long-term.
- **The Brain is not a static folder** — it's a folder + an index + a recall log + a promotion pipeline. Files are the source of truth; the index is the fast lookup; the log + promoter make the system notice what matters without you telling it.
- **You cannot break this by running `makakoo sync` too often.** The sync is idempotent — it uses content hashes to avoid reindexing unchanged files.

## If something went wrong

| Symptom | Fix |
|---|---|
| `makakoo sancho status` reports `sancho: 0 registered task(s)` | The daemon didn't load plugin manifests. Run `makakoo daemon restart` then check again. |
| A specific task shows `fail` on every tick | Run `makakoo plugin info <plugin-name>` and look for Python / shell errors; if the plugin isn't one you use, `makakoo plugin disable <plugin-name>` silences it. |
| `makakoo promotions` errors instead of printing "(no promotion candidates)" | `makakoo memory purge-legacy` one-time fix if you migrated from harvey-os paths. Otherwise check `makakoo memory stats` for schema errors. |
| All counts in `memory stats` are 0 after heavy use | The recall log writer is broken — check `makakoo daemon status` for daemon crashes, restart with `makakoo daemon restart`. |

## Next

- [Walkthrough 05 — Ask Harvey](./05-ask-harvey.md) — now that the Brain has content, ask it a real question via the LLM path.
- [Walkthrough 10 — Mascot mission](./10-mascot-mission.md) — mascots live inside SANCHO; watch one fire a scheduled mission.
