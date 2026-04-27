# Acknowledgements

`skill-meta-caveman-voice` is a Harvey-native adaptation of upstream open-source work:

- **[JuliusBrussee/caveman](https://github.com/JuliusBrussee/caveman)** (MIT License, Copyright (c) 2026 Julius Brussee) — the original "smart caveman" prompt skill. The core terseness rules, the `lite`/`full`/`ultra` intensity level definitions, the Auto-Clarity bypass concept (security warnings, destructive confirmations), the pattern `[thing] [action] [reason]. [next step].`, and the React re-render + DB pooling examples all come from upstream `caveman/SKILL.md`.

## What Harvey's adaptation adds

- **CLI-agnostic placement.** Upstream caveman is packaged as a Claude Code plugin and installs into `~/.claude/skills/`. Harvey is CLI-agnostic — may host under Claude Code today, opencode tomorrow, qwen the day after. This plugin ships as a `bootstrap-fragment` kind so the Makakoo kernel distributes the prompt to every infected CLI host via `makakoo infect --global`, independent of CLI skill registries.
- **Explicit HARD-GATE bypass for external writing.** Upstream caveman has a brief "Auto-Clarity" section covering security warnings and destructive confirmations, but it does not enumerate external-writing bypass contexts. Harvey's adaptation adds a structured HARD-GATE block listing every skill, intent keyword, output destination, and content type where caveman voice must be disabled — because Sebastian produces real polished writing (papers, LinkedIn posts, blog content) that must not be caveman-voiced under any circumstances.
- **Token-math section tied to Harvey's actual workload.** Upstream reports a 65–75% output reduction claim. Harvey's adaptation translates that into ~63% expected aggregate savings based on ~90% internal / ~10% external output mix, and explicitly calls out the drift-over-turns failure mode.
- **`wenyan-*` classical Chinese levels dropped.** Upstream includes `wenyan-lite`/`wenyan-full`/`wenyan-ultra` modes. Harvey's workload doesn't benefit from them.

## License

Upstream caveman ships under MIT. This adaptation stays MIT-compatible. The caveman-derived portions remain under upstream MIT terms per the license.
