# Pixel · SANCHO Doctor

**Species:** kraken · **Mission:** SANCHO health doctor — watches for repeat task failures.
**Cadence:** every 2 hours · **Writer (mission source):** `plugins-core/lib-harvey-core/src/core/mascots/missions.py::pixel_sancho_doctor`
**Mission wiring:** `mascot-gym` plugin (SANCHO task `mascot_pixel_doctor`)

## What Pixel does

Every two hours, Pixel walks the last two days of Brain journals looking for `[[SANCHO]] task: FAILED/ok/skipped` lines. She flags any task with ≥3 **consecutive** failures as sick.

Pixel is read-only. She only writes her findings — she never tries to fix the failing task.

## How to see Pixel's latest report

```sh
grep "\[\[Pixel\]\]" ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md | tail -5
```

Expected output (wraps — one journal line per run):

```text
- [[Pixel]] SANCHO Doctor — 2 tasks still firing failures: inbox_pipeline (295), gym_hypothesize (60).
```

A week-over-week view:

```sh
ls -la ~/MAKAKOO/data/mascots/pixel/ 2>/dev/null
```

## When Pixel is healthy

- `makakoo sancho status` shows the `mascot_pixel_doctor` task registered and recently run.
- A `[[Pixel]]` journal line exists within the last 3 hours.
- Pixel's findings match reality (cross-check with `makakoo sancho status` — the tasks she names really are failing).

## When Pixel disagrees with reality

If Pixel claims a task is still firing failures but `makakoo sancho status` shows it passing:
- Most likely: the task transitioned to healthy after Pixel's last run. Wait 2 hours.
- If still mismatched after two cadences: the mission's journal-scan window or the status parser drifted. File an issue; check the mission source.

## Fire manually

```sh
makakoo sancho tick
```

Look for the `ok mascot_pixel_doctor` row. If the task is present but never fires, check `makakoo plugin info mascot-gym` and `makakoo plugin enable mascot-gym`.

## Disable / re-enable

Pixel is a SANCHO task shipped by `mascot-gym`:

```sh
makakoo plugin disable mascot-gym   # disables ALL mascots
makakoo plugin enable mascot-gym
```

No per-mascot toggle today — `mascot-gym` is the atom.

## Related

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- [`mascots/index.md`](./index.md) — the other four mascots.
