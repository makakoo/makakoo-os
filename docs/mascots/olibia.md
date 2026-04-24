# Olibia · Weekly Home Digest

**Species:** owl (guardian) · **Mission:** Weekly home digest — aggregates the week's activity into one Markdown report.
**Cadence:** Sundays 09:00 local · **Writer:** `plugins-core/lib-harvey-core/src/core/mascots/missions.py::olibia_weekly_digest`
**Mission wiring:** `mascot-gym` plugin (SANCHO task `mascot_olibia_weekly`)

## What Olibia does

Every Sunday morning, Olibia writes a weekly digest of:

- **Commits to makakoo-os** in the past 7 days — count + top 10 subjects.
- **SANCHO health** — ok / failed / skipped tallies, list of tasks still firing failures, historical backlog of retired task failures.
- **Plugins installed.**
- **Prospect pipeline** (if `agent-career-manager` is present) — counts per stage (new / contacted / negotiation / hired / rejected).
- **GYM improvement queue** — pending / approved / rejected candidate counts.
- **Brain activity** — pages modified this week.

The report is a plain Markdown file you can read in 30 seconds.

## Where Olibia writes

- **Digest:** `~/MAKAKOO/data/mascots/olibia/weekly/<YYYY-W-NN>.md` (ISO week number).
- **Journal breadcrumb:** `- [[Olibia]] weekly digest ready → <path>`.

## How to read this week's digest

```sh
ls -t ~/MAKAKOO/data/mascots/olibia/weekly/ | head -1 | xargs -I {} cat ~/MAKAKOO/data/mascots/olibia/weekly/{}
```

## Filter rules (2026-04-24)

Olibia separates **active failures** (task fired a failure in the past 24h) from **dead-manifest backlog** (tasks whose failures are historical and that are no longer registered). This distinction was added after an earlier digest reported `freelance_*_tick: 661 failures` for tasks that had been retired two days prior.

## When Olibia is healthy

- A new file in `~/MAKAKOO/data/mascots/olibia/weekly/` every Sunday.
- Numbers in the digest match ground truth (cross-check commit count with `git log --since='1 week ago' --oneline | wc -l`).

## Fire manually (off-cadence)

```python
from core.mascots.missions import olibia_weekly_digest
olibia_weekly_digest()
```

This writes a digest regardless of the Sunday cadence — useful mid-week when you want a checkpoint.

## Disable / re-enable

Shipped by `mascot-gym`:

```sh
makakoo plugin disable mascot-gym
makakoo plugin enable mascot-gym
```

## Related

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- The other four mascots: [Pixel](./pixel.md), [Cinder](./cinder.md), [Ziggy](./ziggy.md), [Glimmer](./glimmer.md).
