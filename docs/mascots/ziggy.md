# Ziggy · SKILL.md Doctor

**Species:** fox · **Mission:** SKILL.md linter — checks every plugin's `SKILL.md` for missing frontmatter fields, empty usage sections, and stub bodies.
**Cadence:** daily, 08:00 local · **Writer:** `plugins-core/lib-harvey-core/src/core/mascots/missions.py::ziggy_skill_md_doctor`
**Mission wiring:** `mascot-gym` plugin (SANCHO task `mascot_ziggy_doctor`)

## What Ziggy does

Walks every `SKILL.md` under `plugins-core/` and the installed tree, parses the YAML frontmatter, and scores each on four checks:

1. `description` present and non-trivial.
2. Frontmatter required keys present (`name`, `description`, at least one of `allowed-tools` / `triggers`).
3. A non-empty "Usage" / "Call shape" section in the body.
4. Body exceeds a stub length threshold.

Ziggy reports the top-N failures each day.

## How to see Ziggy's latest report

```sh
grep "\[\[Ziggy\]\]" ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md | tail -3
```

Expected output on an install with drift:

```text
- [[Ziggy]] SKILL.md Doctor — 377 of 430 SKILL.md files have gaps. Top 5: skill-foo (no description), skill-bar (empty usage), skill-baz (stub body), skill-qux (missing allowed-tools), skill-quux (truncated frontmatter).
```

## Fixing what Ziggy surfaces

Pick one of the flagged `SKILL.md` files and fill in the missing section. If you maintain that plugin, the fix is usually a 5-line edit. If you don't, either disable the plugin or file an issue on its source repo.

## When Ziggy is healthy

- Daily journal line tagged `[[Ziggy]]`.
- The numbers in her report trend down over time as plugins get polished.

## Fire manually

```sh
makakoo sancho tick
```

The task is eligible once per day — if Ziggy already ran today, she'll be marked `skip`. Force via:

```python
from core.mascots.missions import ziggy_skill_md_doctor
ziggy_skill_md_doctor()
```

## Disable / re-enable

Shipped by `mascot-gym`:

```sh
makakoo plugin disable mascot-gym
makakoo plugin enable mascot-gym
```

## Related

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- [`plugins/writing.md`](../plugins/writing.md) — the `SKILL.md` authoring guide Ziggy enforces.
