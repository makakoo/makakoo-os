---
name: lazy-build
description: Use when implementing, debugging, refactoring, planning, or reviewing code where over-engineering, unnecessary dependencies, speculative abstractions, duplicated helpers, broad rewrites, or "build the simplest safe thing" are relevant; provides a YAGNI/minimality ladder, root-cause-first bugfix discipline, and over-engineering review rubric without weakening security, validation, accessibility, error handling, or explicit requirements
---

# Lazy Build

Build less. Understand more. Ship the smallest safe change that satisfies the real requirement.

This is a Makakoo-native adaptation of Ponytail-style minimality discipline. It governs **what you build**, not how terse you talk. Pair with `caveman-voice` only for output compression.

## Gate Function

Before writing or approving code:

1. **Understand the flow**: read the touched code and trace the real caller/user path.
2. **Classify the task**: implementation, bugfix, refactor, plan, diff review, or repo audit.
3. **Climb the ladder**: stop at the first rung that truly satisfies the requirement.
4. **Verify the boundary**: make sure the smaller solution did not cut correctness, security, validation, accessibility, data-loss handling, or explicit scope.

Skipping step 1 is not minimality. It is guessing with fewer lines.

## The Ladder

Stop at the first rung that works:

1. **Do not build it**: if the need is speculative, skip it and say what existing behavior covers.
2. **Reuse this codebase**: search for an existing helper, type, endpoint, component, command, fixture, or pattern before adding another.
3. **Use the standard library**: prefer maintained built-ins over owned code.
4. **Use native platform features**: HTML/CSS/browser/OS/DB/framework features before custom layers.
5. **Use already-installed dependencies**: never add a dependency for what the platform or a few clear lines can do.
6. **Shrink the implementation**: one clear line beats a helper; a helper beats a framework; deletion beats addition.
7. **Write minimum custom code**: only after the previous rungs fail.

If two options are equally small, pick the boring one that handles edge cases better.

## Bugfix Rule

A bug report names a symptom, not necessarily the failing layer.

For bugfixes:

1. Find the function/module you think needs changing.
2. Search every caller and sibling path.
3. Fix the shared root cause once if possible.
4. Add the smallest regression check that fails on the original bug.

Bad lazy: guard only the reported caller while sibling callers still break.
Good lazy: one guard in the shared path.

## Implementation Mode

When building:

- Prefer deleting, wiring, or reusing over new architecture.
- Keep file count low unless separation is already established in the project.
- Do not introduce interfaces, factories, config knobs, queues, caches, providers, or abstractions with one real implementation unless explicitly required.
- Mark deliberate shortcuts with `lazy-build:` comments only when the shortcut has a real ceiling:
  - Good: `// lazy-build: global lock is enough for local CLI; switch to per-project locks if concurrent writes exceed one process.`
  - Bad: `// lazy-build: TODO later`
- Non-trivial logic leaves one runnable check: test, assert-based smoke, or existing gate. No framework ceremony unless the repo already uses it.

## Review Mode

When reviewing a diff or repo for over-engineering, report only complexity findings. Use one line per finding:

`<path>:L<line>: <tag>: <what to cut>. <replacement>.`

Tags:

- `delete`: dead code, speculative feature, unused flag, unused adapter.
- `stdlib`: hand-rolled built-in behavior.
- `native`: custom dependency/code doing what platform/framework already does.
- `yagni`: abstraction/config/layer with one implementation or no current caller.
- `reuse`: duplicate helper/pattern already exists nearby.
- `shrink`: same behavior with fewer clearer lines.
- `wrong-layer`: small diff patches symptom, not shared cause.

End with `net: -<N> lines, -<M> deps possible.` If nothing real: `Lean already. Ship.`

## Hard Boundaries

Never simplify away:

- trust-boundary validation,
- authz/authn checks,
- security logging/audit trail,
- data-loss handling,
- accessibility basics,
- user-visible requirements explicitly requested,
- legal/compliance constraints,
- correctness on documented edge cases,
- deterministic verification before claiming success.

If a smaller implementation weakens any boundary, it is not the lazy solution. It is a bug.

## Output Contract

For implementation:

1. State the rung used.
2. State what was intentionally not built.
3. State the verification run or the missing gate.

For review:

1. Findings only.
2. No correctness/security/performance claims unless they are directly about over-engineering risk.
3. Do not apply fixes unless the user asks.

## Do Not Adopt Runtime Plumbing

Do not install third-party Ponytail hooks, CLI plugins, MCP servers, or global mode state as part of this skill. This skill is text/rubric only. Makakoo owns its agent lifecycle.

## Attribution

Adapted from the MIT-licensed Ponytail project by Dietrich Gebert (`https://github.com/DietrichGebert/ponytail`). License copy: `references/ponytail-license.txt`.
