# Glimmer · Brain Gardener

**Species:** owl (junior) · **Mission:** Brain gardener — archives stale `Lead -*` and `Inbox -*` pages older than 14 days.
**Cadence:** daily, 22:00 local · **Writer:** `plugins-core/lib-harvey-core/src/core/mascots/missions.py::glimmer_brain_gardener`
**Mission wiring:** `mascot-gym` plugin (SANCHO task `mascot_glimmer_garden`)

## What Glimmer does

Glimmer is the **only writer mascot**. She moves Brain pages that match `Lead -*.md` or `Inbox -*.md` and haven't been modified in 14 days into `~/MAKAKOO/data/Brain/archive/<YYYY>/<MM>/`. Everything else she leaves alone.

Her goal: keep the top-level `Brain/pages/` dir scannable by humans, not a graveyard.

## Where Glimmer writes

- **Archived pages:** `~/MAKAKOO/data/Brain/archive/<YYYY>/<MM>/<original-filename>.md`
- **Journal breadcrumb:** one `- [[Glimmer]] ...` line per run summarizing how many pages she moved.

## Dry-run first

Glimmer respects a dry-run mode. Run it before you trust her with your Brain:

```python
from core.mascots.missions import glimmer_brain_gardener
glimmer_brain_gardener(dry_run=True)
```

Prints the list of candidate pages without moving anything.

## How to see Glimmer's latest report

```sh
grep "\[\[Glimmer\]\]" ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md | tail -3
```

Expected output on a normal day:

```text
- [[Glimmer]] Brain Gardener — archived 0 pages (7 candidates were modified within 14d).
```

On a day with archivable cruft:

```text
- [[Glimmer]] Brain Gardener — archived 4 stale pages → Brain/archive/2026/04/.
```

## When Glimmer is healthy

- Daily journal line tagged `[[Glimmer]]`.
- The archive tree grows with files, not copies of the active pages.
- A spot-check: pick one archived page, confirm it's gone from `Brain/pages/` and present in `Brain/archive/`.

## Undo an archive

```sh
mv ~/MAKAKOO/data/Brain/archive/2026/04/<page>.md ~/MAKAKOO/data/Brain/pages/
makakoo sync
```

## Disable / re-enable

Shipped by `mascot-gym`. Glimmer is the only mascot that **writes** — if you're nervous, disable just her by editing `~/MAKAKOO/plugins/mascot-gym/plugin.toml` and commenting out the `mascot_glimmer_garden` SANCHO task entry. Or disable the whole mascot-gym:

```sh
makakoo plugin disable mascot-gym
```

## Related

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- [`brain/index.md`](../brain/) — the Brain convention Glimmer curates.
