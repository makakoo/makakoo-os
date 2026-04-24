# Mascots

Five mascots live inside Makakoo. They are a persona layer on top of SANCHO: each one owns a scheduled mission that makes Makakoo healthier or your Brain tidier, and each mission writes a wikilink-tagged journal line so findings are discoverable by mascot name.

Read [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md) first. This page is the reference catalog.

## The catalog

| Mascot | Species | Job | Cadence | Docs |
|---|---|---|---|---|
| **Pixel** | kraken | SANCHO Doctor — flags tasks with ≥3 consecutive failures | every 2h | [manual](./pixel.md) |
| **Cinder** | panther | Entrypoint Sentinel — compile-checks every plugin's entrypoint | every 4h | [manual](./cinder.md) |
| **Ziggy** | fox | SKILL.md Doctor — lints frontmatter / description / usage sections | daily 08:00 | [manual](./ziggy.md) |
| **Glimmer** | owl (junior) | Brain Gardener — archives stale `Lead -*` / `Inbox -*` pages > 14d | daily 22:00 | [manual](./glimmer.md) |
| **Olibia** | owl (guardian) | Weekly Home Digest — aggregates the week's activity into one report | Sundays 09:00 | [manual](./olibia.md) |

## Why mascots at all

A SANCHO task with no personality is noise: the journal fills with `- [[SANCHO]] <task-name> ok 0.24s` and nobody reads it. A task with a mascot identity becomes a character you can **recognize in the journal**:

```text
- [[Pixel]] SANCHO Doctor — 2 tasks still firing failures: inbox_pipeline (295), gym_hypothesize (60).
```

The tag makes findings discoverable by mascot name. `grep "\[\[Pixel\]\]" ~/MAKAKOO/data/Brain/journals/*.md` is a usable health-over-time view.

## The rules every mascot follows

1. **Idempotent within a SANCHO tick** — safe to re-run the same tick.
2. **Bounded output** — findings capped so one bad day can't flood the journal.
3. **Read-only by default** — only Glimmer writes (and only to `Brain/archive/`).
4. **Python 3.9 compatible** — the missions run across every supported install.
5. **One journal line per run** — always tagged `[[<MascotName>]] ...`.

## Disable all mascots at once

Mascots are shipped by one plugin:

```sh
makakoo plugin disable mascot-gym
makakoo daemon restart
```

No per-mascot toggle today. If you want granular control, fork `mascot-gym` and comment out SANCHO task entries.

## Related docs

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- [Agents](../agents/) — the broader agent catalog mascots specialize.
- [Brain](../brain/) — the journal / pages conventions mascots use.
- [`concepts/sancho.md`](../concepts/sancho.md) — the task engine mascots run on.
