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

## Frozen as of v0.1.0

Every file in this directory is part of the **v0.1 kernel contract**. Once v0.1.0 is tagged, these files do not change in a way that alters meaning until a `v0.2.0` (or later) release explicitly documents the migration:

- **Additive changes** (new optional field, new capability verb, new ABI member with default) → allowed within a minor version, documented in `CHANGELOG.md`.
- **Breaking changes** (field rename, removed verb, schema rework, tightened validation) → require a major version bump + a migration note in the release. Plugin manifest ABI versions (`[abi]` table) track this per-kind (skill, agent, sancho-task, mcp-tool, mascot, bootstrap-fragment).
- **Clarifications** (better wording, examples, typo fixes) → always allowed; the semantic content doesn't change.

Plugin authors: when you write `[abi].sancho-task = "^1.0"`, you're depending on the v0.1-frozen `ABI_SANCHO_TASK.md` contract. The kernel's `resolver.rs` enforces the check at load time — plugins that declare `^0.1` (which was the pre-freeze shape) fail to load with a clear error pointing at this rule.

This note is the provenance record. `git log --follow spec/<file>.md` shows every change after v0.1.0; mismatches between file content and the tag are bugs.
