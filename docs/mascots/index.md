# Mascots

Five mascots live inside Makakoo. They are a persona layer on top of SANCHO:
each one owns a scheduled mission that makes Makakoo healthier or your Brain
tidier, and each mission writes a wikilink-tagged journal line so findings
are discoverable by mascot name.

Read [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
first. This page is the reference catalog.

## The catalog

| Mascot | Species | Mission | Cadence | Docs |
|---|---|---|---|---|
| **Pixel** | kraken | SANCHO Doctor — flags tasks with ≥3 consecutive failures | every 2h | [manual](./pixel.md) |
| **Cinder** | panther | Entrypoint Sentinel — compile-checks every plugin's entrypoint | every 4h | [manual](./cinder.md) |
| **Ziggy** | fox | SKILL.md Doctor — lints frontmatter / description / usage sections | daily 08:00 | [manual](./ziggy.md) |
| **Glimmer** | owl (junior) | Brain Gardener — archives stale `Lead -*` / `Inbox -*` pages > 14d | daily 22:00 | [manual](./glimmer.md) |
| **Olibia** | owl (guardian) | Weekly Home Digest — aggregates the week's activity into one report | Sundays 09:00 | [manual](./olibia.md) |

## Types of mascots

Mascots fall into two functional families:

**Health watchers** catch drift before it becomes a crisis. They are
read-only — they observe, report, and tag; they never modify anything.

| Mascot | What it watches |
|---|---|
| Pixel | SANCHO task health (repeated failures → alert) |
| Cinder | Plugin entrypoint integrity (compile errors → alert) |
| Ziggy | SKILL.md quality (missing frontmatter / empty usage → report) |

**Curators** actively tidy things up on a schedule. They hold the only
write permissions in the mascot layer.

| Mascot | What it curates |
|---|---|
| Glimmer | Brain pages — archives stale `Lead-*` / `Inbox-*` files |
| Olibia | Weekly digest — aggregates commits, SANCHO health, Brain activity |

## How a mascot hatches

Mascots are not individually installed. They ship as a bundle via the
`mascot-gym` plugin. When `mascot-gym` is installed and enabled, the daemon
picks up five SANCHO task registrations on its next start:

```
mascot_pixel_doctor    → every 2h
mascot_cinder_sentinel → every 4h
mascot_ziggy_doctor    → daily 08:00
mascot_glimmer_garden  → daily 22:00
mascot_olibia_weekly   → Sundays 09:00
```

Each SANCHO task resolves to a Python callable in
`plugins-core/lib-harvey-core/src/core/mascots/missions.py`. On first tick,
the mascot writes its inaugural journal line — that is the "hatch" moment.

```sh
# check that all five mascots are registered
makakoo sancho status | grep mascot

# force a tick to hatch them immediately after install
makakoo sancho tick
```

## Mood and state model

Each mascot is stateless between ticks — it reads from the Brain or the
filesystem on every run and writes a fresh journal line. There is no
persisted "mood" field. Instead, the journal line itself carries the mood:

- **Healthy:** `... — 0 broken entrypoints across 214 plugins.`
- **Warning:** `... — 3 broken entrypoints: skill-X (SyntaxError), ...`
- **Silent:** if the task is marked `skipped` in SANCHO status, the mascot
  did not write a journal line this cycle (cadence guard, not a fault).

You can reconstruct a mascot's mood history by grepping their wikilink tag
across all journal files:

```sh
grep "\[\[Pixel\]\]" ~/MAKAKOO/data/Brain/journals/*.md | tail -20
grep "\[\[Cinder\]\]" ~/MAKAKOO/data/Brain/journals/*.md | tail -10
```

## Working example — reading what the mascots wrote today

```sh
TODAY=$(date +%Y_%m_%d)
JOURNAL=~/MAKAKOO/data/Brain/journals/$TODAY.md

# all mascot activity in one view
grep -E "\[\[(Pixel|Cinder|Ziggy|Glimmer|Olibia)\]\]" "$JOURNAL"
```

Expected output on a healthy install:

```text
- [[Pixel]] SANCHO Doctor — all tasks healthy (last 2d scan).
- [[Cinder]] Entrypoint Sentinel — 0 broken entrypoints across 214 plugins.
- [[Ziggy]] SKILL.md Doctor — 12 of 430 files have gaps. Top 3: skill-foo (no description), ...
- [[Glimmer]] Brain Gardener — archived 0 pages (3 candidates were modified within 14d).
```

## Why mascots at all

A SANCHO task with no personality is noise: the journal fills with
`- [[SANCHO]] <task-name> ok 0.24s` and nobody reads it. A task with a
mascot identity becomes a character you can **recognize in the journal**.
The wikilink tag makes findings discoverable by name.
`makakoo search "[[Pixel]]"` is a usable health-over-time view.

## The rules every mascot follows

1. **Idempotent within a SANCHO tick** — safe to re-run the same tick.
2. **Bounded output** — findings capped so one bad day cannot flood the journal.
3. **Read-only by default** — only Glimmer writes (and only to `Brain/archive/`).
4. **Python 3.9 compatible** — the missions run across every supported install.
5. **One journal line per run** — always tagged `[[<MascotName>]] ...`.

## Disable all mascots at once

Mascots are shipped by one plugin:

```sh
makakoo plugin disable mascot-gym
makakoo daemon uninstall && makakoo daemon install
```

No per-mascot toggle today. For granular control, fork `mascot-gym` and
comment out the relevant SANCHO task entries in `plugin.toml`.

## Individual mascot pages

- [Pixel](./pixel.md) — SANCHO Doctor (health watcher, every 2h)
- [Cinder](./cinder.md) — Entrypoint Sentinel (health watcher, every 4h)
- [Ziggy](./ziggy.md) — SKILL.md Doctor (health watcher, daily 08:00)
- [Glimmer](./glimmer.md) — Brain Gardener (curator, daily 22:00)
- [Olibia](./olibia.md) — Weekly Home Digest (curator, Sundays 09:00)

## Related docs

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- [Agents](../agents/) — the broader agent catalog mascots specialize.
- [Brain](../brain/) — the journal / pages conventions mascots use.
- [`user-manual/makakoo-sancho.md`](../user-manual/makakoo-sancho.md) — the SANCHO task engine.
- [`concepts/sancho.md`](../concepts/sancho.md) — SANCHO architecture.
