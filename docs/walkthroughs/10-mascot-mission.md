# Walkthrough 10 — Meet the mascots and fire one mission

## What you'll do

See the nursery of mascots registered on your install, inspect one mission-capable mascot, and manually trigger a mascot's scheduled mission so you can observe the journal entry it produces. Mascots are specialized SANCHO-driven agents whose job is to keep your Makakoo home healthy and your Brain tidy.

**Time:** about 4 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md), [Walkthrough 04](./04-write-brain-journal.md) for SANCHO vocabulary.

## What are mascots?

**Mascots** are characters — part persona, part maintenance worker — that fire periodic missions. Each has a name, a species (kraken, panther, fox, owl, …), and a job (SANCHO doctor, entrypoint sentinel, SKILL.md linter, Brain gardener, weekly digest writer).

Mechanically, a mascot mission is a **SANCHO task that writes a journal line** tagged with the mascot's name when it finishes. If the journal starts accumulating `[[Pixel]] SANCHO Doctor: 3 consecutive failures detected on inbox_pipeline` lines, that's Pixel (the SANCHO doctor mascot) doing her job.

The **buddy** is your currently-active foreground mascot — the one whose ASCII face might show up in `makakoo buddy status` or in a tray UI. Most installs don't have an active buddy yet (cosmetic, not functional).

## Steps

### 1. Check if you have an active buddy

```sh
makakoo buddy status
```

On a fresh install:

```text
[no active buddy — `harvey nursery adopt <name>` to pick one]
```

> **Note:** the `harvey nursery adopt` hint is a broken suggestion (DOGFOOD-FINDINGS F-009) — neither `adopt` subcommand exists today. For now, ignore the hint; buddy selection is cosmetic and not required for mission-firing.

### 2. List the mascots registered in the nursery

```sh
makakoo nursery list
```

Expected output on a fresh install:

```text
┌─────────┬─────────┬────────┬────────────┬─────┐
│ name    │ species │ status │ maintainer │ job │
├─────────┼─────────┼────────┼────────────┼─────┤
│ Cricket │ kraken  │ Active │            │     │
│ Doodle  │ panther │ Active │            │     │
│ Pickle  │ fox     │ Active │            │     │
└─────────┴─────────┴────────┴────────────┴─────┘
```

The nursery starts with three placeholder mascots. You can hatch more (assign them specific jobs) — skip this walkthrough or see the Hatching appendix below.

### 3. Register a working mascot (optional — only if you want to create one)

For example, hatch a new mascot that guards SANCHO health:

```sh
makakoo nursery hatch Pixel \
  --species kraken \
  --maintainer "@you" \
  --job "SANCHO health doctor — watches for repeat task failures"
```

Expected output:

```text
hatched: Pixel (kraken) — job logged; mission wiring is separate (see docs/mascots/)
```

A `hatch` call only registers the mascot in the nursery; the **mission wiring** (which SANCHO task fires periodically and writes the `[[Pixel]] ...` journal lines) is a separate plugin — typically shipped as `mascot-gym` in the core distro. `makakoo sancho status` in step 4 shows whether those wired missions are present on your install.

### 4. See if any mascot missions are registered

```sh
makakoo sancho status
```

Expected output on a fresh install:

```text
sancho: 32 registered task(s) (11 native + 21 manifest)
(no task has run yet this process)
```

If your install has `mascot-gym` (the mascot-missions plugin) installed, the 21 manifest tasks include entries like `mascot_pixel_doctor`, `mascot_cinder_sentinel`, `mascot_ziggy_doctor`, `mascot_glimmer_garden`, `mascot_olibia_weekly`. (The exact set is distro-dependent.)

### 5. Fire every eligible mission exactly once

```sh
makakoo sancho tick
```

Expected output (truncated — each task prints a status line):

```text
SANCHO tick — 32 eligible tasks
ok   journal_compactor        0.24s
ok   brain_resurface          1.80s
ok   memory_promoter          0.41s
ok   mascot_pixel_doctor      0.30s  (SANCHO health: 2 tasks still firing failures)
ok   mascot_ziggy_doctor      0.95s  (SKILL.md: 377 / 430 files have gaps)
skip mascot_olibia_weekly     (interval=604800s; ran 3h 12m ago)
...
```

The `mascot_*` rows are mascot missions firing. Each one writes a `[[Mascot-Name]] ...` line to today's Brain journal.

### 6. Read the mascot's journal entry

```sh
grep "^- \[\[" ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md | grep -iE "(pixel|cinder|ziggy|glimmer|olibia)" | tail -5
```

Expected output (truncated):

```text
- [[Pixel]] SANCHO Doctor — 2 tasks still firing failures: inbox_pipeline (295), gym_hypothesize (60).
- [[Ziggy]] SKILL.md Doctor — 377 of 430 SKILL.md files missing a description or a usage section.
```

Every line is a real finding. The mascots are, quite literally, surfacing bugs — this walkthrough is itself the mascot system working correctly.

### 7. See the mascot's data directory

Mascots with mission wiring keep state under `~/MAKAKOO/data/mascots/`:

```sh
ls ~/MAKAKOO/data/mascots/ 2>/dev/null
```

Expected output (depends on which mascots have run):

```text
olibia
pixel
ziggy
```

Each directory holds that mascot's artifacts — e.g. `olibia/weekly/2026-W17.md` is Olibia's most recent weekly home digest.

## What just happened?

- **Mascots are a persona layer over SANCHO.** The hard work is in SANCHO tasks; mascots add identity + a consistent wikilink tag so findings are discoverable by mascot name.
- **The nursery is a registry, not the runtime.** `makakoo nursery list` shows who is registered. The running-mission surface is SANCHO.
- **Every mascot mission must write a journal line** to be visible. A mascot that "runs" but doesn't leave a `[[Name]] ...` breadcrumb is indistinguishable from noise — unusable. The `mascot-gym` plugin enforces this pattern for shipped mascots.
- **The buddy system is decorative.** As of v0.1.0, `makakoo buddy status` only tracks one "active" mascot for display in a future tray UI. Missions don't require a buddy.

## If something went wrong

| Symptom | Fix |
|---|---|
| `makakoo nursery list` returns only 3 placeholder mascots (Cricket / Doodle / Pickle) | That's the initial registry seed. Hatch the real mascots with `makakoo nursery hatch <name> --species <kind> --job "..."`. Or install `mascot-gym` which seeds the canonical 5. |
| `makakoo sancho tick` runs but no `mascot_*` rows appear | `mascot-gym` isn't installed. Check: `makakoo plugin list \| grep mascot`. Install with `makakoo plugin install --core mascot-gym` (from a repo checkout). |
| Mascot missions fire but no journal lines appear | Filesystem permission issue. Check `ls -la ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md` — should be writable by you. |
| `makakoo buddy status` suggests `harvey nursery adopt` | DOGFOOD-FINDINGS F-009 — broken hint string. Ignore; buddy selection isn't required. |

## Next

- [Walkthrough 11 — Connect Tytus](./11-connect-tytus.md) — extend the Brain to a remote pod you can access from anywhere.
- [Walkthrough 12 — Octopus federation](./12-octopus-federation.md) (stub — pending Octopus-generalize sprint Phase 1 merge).
