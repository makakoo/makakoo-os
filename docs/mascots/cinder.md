# Cinder · Entrypoint Sentinel

**Species:** panther · **Mission:** Plugin entrypoint sentinel — compile-checks every plugin's entrypoint script.
**Cadence:** every 4 hours · **Writer:** `plugins-core/lib-harvey-core/src/core/mascots/missions.py::cinder_entrypoint_sentinel`
**Mission wiring:** `mascot-gym` plugin (SANCHO task `mascot_cinder_sentinel`)

## What Cinder does

Walks every installed plugin's `plugin.toml`, parses the `[entrypoint].start` script path, runs `py_compile` (for Python entrypoints) against it, and flags any plugin whose entrypoint fails to compile — before the daemon tries to spawn it and crashes.

Read-only. She reports; she doesn't fix.

## How to see Cinder's latest report

```sh
grep "\[\[Cinder\]\]" ~/MAKAKOO/data/Brain/journals/$(date +%Y_%m_%d).md | tail -3
```

Expected output on a healthy install:

```text
- [[Cinder]] Entrypoint Sentinel — 0 broken entrypoints across 214 plugins.
```

On a drifting install after a big refactor:

```text
- [[Cinder]] Entrypoint Sentinel — 3 broken entrypoints: skill-X (ModuleNotFoundError), agent-Y (SyntaxError line 42), skill-Z (entrypoint file missing).
```

## When Cinder is healthy

- `makakoo sancho status` lists `mascot_cinder_sentinel`.
- Recent `[[Cinder]]` journal line.
- Count she reports for "broken entrypoints" matches what the daemon logs on startup.

## Fire manually

```sh
makakoo sancho tick
```

Or invoke the underlying Python function directly from a REPL:

```python
from core.mascots.missions import cinder_entrypoint_sentinel
cinder_entrypoint_sentinel()
```

## Disable / re-enable

Shipped by `mascot-gym`:

```sh
makakoo plugin disable mascot-gym
makakoo plugin enable mascot-gym
```

## Related

- [Walkthrough 10 — Meet the mascots](../walkthroughs/10-mascot-mission.md)
- DOGFOOD-FINDINGS F-006 (invalid `language = "markdown"` in a plugin manifest) — Cinder would surface this kind of manifest drift today.
