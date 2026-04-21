# Skills inventory — SKILL.md coverage for every MCP tool

This document is the audit table for every Makakoo MCP tool, whether
it ships a portable `SKILL.md`, and where that file lives. It's
regenerated manually during each sprint that ships or retires tools;
the contract test at `makakoo/tests/skill_manifests.rs` verifies every
claimed `SKILL.md` exists and parses.

Legend:

- **Portable**: agents in other runtimes (LangChain, OpenAI
  Assistants, Cursor rules, ChatGPT custom instructions) can read the
  `SKILL.md` and wire the tool into their own system. No Makakoo
  runtime required.
- **Internal**: the tool has meaning only inside a running Makakoo
  install (Brain, Olibia, SANCHO, HarveyChat, Nursery, Buddy,
  etc). External agents can't benefit without running Makakoo, so no
  portable `SKILL.md` ships.
- **Bootstrap fragment**: a companion plugin under
  `plugins-core/bootstrap-fragment-<name>/` weaves trigger-pattern
  guidance into every infected CLI's global bootstrap. Distinct from
  the `SKILL.md` which sits in the plugin's source tree and is
  surfaced on-demand via `skill_discover`.

Totals as of v0.5 (2026-04-21): 53 MCP tools, 6 portable tool families
documented in 6 `SKILL.md` files, 21 internal-only tools intentionally
skipped.

## Portable tools (6 families → 6 SKILL.md)

| Tool | Family | SKILL.md | Bootstrap fragment | Notes |
|---|---|---|---|---|
| `harvey_browse` | browse | `plugins-core/agent-browser-harness/SKILL.md` | `plugins-core/bootstrap-fragment-browser-harness/` | v0.4 flagship; real-Chrome CDP via upstream browser-use/browser-harness. |
| `harvey_describe_image` | multimodal | `plugins-core/agent-multimodal-knowledge/SKILL.md` | — | xiaomi-tp/mimo-v2-omni via switchAILocal. Stateless Q&A. |
| `harvey_describe_audio` | multimodal | same | — | Transcribe + summarise. |
| `harvey_describe_video` | multimodal | same | — | Adds `fps` + `media_resolution` knobs. |
| `harvey_generate_image` | multimodal | same | — | Routes to `ail-image`. |
| `harvey_knowledge_ingest` | multimodal | same | — | Persistent; chunks + embeds into multimodal Qdrant. |
| `pi_run` | pi | `plugins-core/agent-pi/SKILL.md` | — | One turn via `pi --rpc`. |
| `pi_session_fork` | pi | same | — | Non-destructive branch. |
| `pi_session_label` | pi | same | — | Anchor a checkpoint. |
| `pi_session_export` | pi | same | — | html or md. |
| `pi_set_model` | pi | same | — | Hot-swap mid-session. |
| `pi_steer` | pi | same | — | Mid-turn guidance injection. |
| `agent_list` | agents | `plugins-core/lib-harvey-core/skills/agents/SKILL.md` | — | Lists every `agents/<name>/agent.toml`. |
| `agent_info` | agents | same | — | Single-agent AgentSpec. |
| `agent_install` | agents | same | — | Imports from a src dir. |
| `agent_create` | agents | same | — | Scaffolds python\|rust\|shell stub. |
| `agent_uninstall` | agents | same | — | Refuses while fs2 lock held. |
| `wiki_compile` | wiki | `plugins-core/lib-harvey-core/skills/wiki/SKILL.md` | — | Freeform MD → Logseq bullet tree. |
| `wiki_lint` | wiki | same | — | Inline `content` or `page_path`. |
| `wiki_save` | wiki | same | — | Atomic fs2-locked write. |
| `skill_discover` | meta | `plugins-core/lib-harvey-core/skills/skill_discover/SKILL.md` | planned for v0.5 Phase E | Walks `$MAKAKOO_HOME/plugins/*/SKILL.md`; self-referential. |

## Internal-only tools (no portable SKILL.md ships)

These tools are meaningful only inside a running Makakoo install.
Shipping a `SKILL.md` would mislead external agents into calling
capabilities that don't apply to them. Per sprint v0.5 locked
decision D3, they stay undocumented for external consumption.

| Tool | Why internal |
|---|---|
| `brain_search`, `brain_query`, `brain_context`, `brain_entities`, `brain_recent`, `brain_write_journal` | Makakoo Brain (Logseq vault) is Sebastian-specific state. |
| `harvey_brain_search`, `harvey_brain_write`, `harvey_journal_entry` | Same — Brain write surface. |
| `harvey_superbrain_query`, `harvey_superbrain_vector_search` | FTS + vector retrieval over the Brain. |
| `harvey_telegram_send` | Routes to Sebastian's Telegram bot. |
| `harvey_olibia_speak` | Olibia mascot voice engine. |
| `harvey_infect_local` | CLI-global-slot bootstrap installer. |
| `harvey_swarm_run`, `harvey_swarm_status`, `swarm` | SANCHO delegation / swarm gateway. |
| `dream` | Memory consolidation. |
| `sancho_status`, `sancho_tick` | SANCHO engine management. |
| `chat_send`, `chat_history`, `chat_stats`, `chat_status` | HarveyChat conversational surface. |
| `nursery_hatch`, `nursery_status` | Nursery mascot lifecycle. |
| `buddy_status` | Active mascot pointer. |
| `outbound_draft` | Email/LinkedIn draft queue — draft-first, never auto-send. |
| `costs_summary` | Sebastian's LLM cost tracker. |
| `grant_write_access`, `revoke_write_access`, `list_write_grants` | v0.3 runtime capability grants — Sebastian-only authorisation. |

## How external agents consume this

1. Start at `skill_discover(query="")` — walks the tree, returns every
   `SKILL.md` path.
2. For each hit the agent cares about, `Read` the full body.
3. Implement the trigger patterns + call shapes in the host runtime.
4. For fully portable code examples (LangChain / OpenAI Assistants /
   Cursor rules), see `docs/agents/consuming-makakoo-externally.md`.

## Changelog

- **2026-04-21 (v0.5 Phase C)** — Added 4 new SKILL.md: `agent-pi`,
  `lib-harvey-core/skills/{agents,wiki,skill_discover}`. 2-of-6
  families before v0.5 → 6-of-6 after. Audit doc first landed.
- **2026-04-21 (v0.4.3)** — Consolidated multimodal to a single
  SKILL.md (5 tools, previously 0).
- **2026-04-21 (v0.4.2)** — First portable SKILL.md: browser-harness.
