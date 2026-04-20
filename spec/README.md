# `makakoo-os/spec/` — code contracts

This folder holds only the **machine-readable contracts** that the Rust kernel + plugin validators + capability enforcer compile against. Human-facing planning docs (sprint master, architecture narrative, parasite model, security posture, install matrix, reviews) moved to:

> `MAKAKOO/development/sprints/queued/MAKAKOO-OS-KERNEL/`

## What's here

| File | Purpose | Read by |
|---|---|---|
| `ABI_AGENT.md` | Agent plugin ABI | AgentRegistry / runner (Phase C–E) |
| `ABI_MCP_TOOL.md` | MCP tool plugin ABI | `makakoo-mcp` tool registry |
| `ABI_SKILL.md` | Skill plugin ABI | Skill dispatcher |
| `ABI_MASCOT.md` | Mascot plugin ABI | Nursery + GYM |
| `ABI_SANCHO_TASK.md` | Scheduled task plugin ABI | SANCHO runner |
| `ABI_BOOTSTRAP_FRAGMENT.md` | Bootstrap fragment plugin ABI | `infect --global` renderer |
| `PLUGIN_MANIFEST.md` | `plugin.toml` schema v0.1 LOCKED | Manifest parser |
| `DISTRO.md` | Distro `.toml` format + include semantics v0.1 LOCKED | Distro resolver |
| `CAPABILITIES.md` | Capability grants + enforcement model | Capability enforcer |

## Where the roadmap lives

- **Master plan:** `MAKAKOO/development/sprints/queued/MAKAKOO-OS-KERNEL/SPRINT.md`
- **Architecture narrative:** `…/specs/ARCHITECTURE.md`
- **Parasite model:** `…/specs/PARASITE.md`
- **Security posture:** `…/specs/SECURITY.md`
- **Install matrix:** `…/specs/INSTALL_MATRIX.md`
- **Phase trackers:** `…/phases/PHASE-{A,B,C,…}/`
- **Reviews:** `…/reviews/`

Rule: if the compiler (or the plugin validator, or the distro resolver, or the capability enforcer) reads it, it belongs here. Everything else belongs in `MAKAKOO/development/`.
