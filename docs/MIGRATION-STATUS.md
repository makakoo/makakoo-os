# Makakoo OS — Python → Rust Migration Status

Last updated: 2026-04-18 (Sprint 005)

## Principle

**Rust = kernel (infrastructure, persistence, enforcement)**
**Python = userland (skills, agents, LLM glue)**

Subprocess boundaries between them are healthy and intentional.

## Fully Migrated (Rust authoritative)

| Component | Python file | Rust location | Action |
|-----------|-------------|---------------|--------|
| Event bus | `core/orchestration/persistent_event_bus.py` | `makakoo-core/src/event_bus.rs` | ⚠️ DUAL-WRITE. Same SQLite DB. Python active (14 consumers) |
| Superbrain store | `core/superbrain/store.py` | `makakoo-core/src/superbrain/` | ⚠️ DUAL-WRITE. Same `superbrain.db`. Both read/write |
| Superbrain search | `superbrain search` | `makakoo search` | Rust faster. Python still daily-driver CLI |
| Superbrain query | `superbrain query` | `makakoo query` | Rust available. Python has more features |
| Infect (global) | `core/orchestration/infect_global.py` | `makakoo/src/infect/` | ❌ DEPRECATED. Rust has fragment assembly |
| Infect (per-project) | `core/orchestration/infect.py` | not ported | Python still active for per-project |
| Config loader | `core/config/persona.py` | `makakoo-core/src/config.rs` | Rust authoritative |
| Plugin system | N/A | `makakoo-core/src/plugin/` | Rust-native |
| Capability stack | N/A | `makakoo-core/src/capability/` | Rust-native |
| CLI | `bin/harvey` (dispatch) | `makakoo` binary | Rust authoritative |
| SANCHO scheduler | `core/sancho/engine.py` | `makakoo-core/src/sancho/` | Rust schedules, Python tasks execute |
| Database | `core/superbrain/db.py` (PostgreSQL) | `makakoo-core/src/db.rs` (SQLite) | ❌ DEPRECATED. PostgreSQL backend retired |
| Install/setup | N/A | `makakoo install/setup/migrate` | Rust-native |

## Keep in Python (indefinitely)

| Component | LOC | Reason |
|-----------|-----|--------|
| 183 skills | ~15,000 | LLM prompts + API calls, rapid iteration |
| 9 agents | ~32,000 | Domain-specific (polymarket, career, etc.) |
| Agent lifecycle | 3,765 | Spawn, signal handling |
| Coordinator | 694 | 4-phase swarm orchestration |
| GYM + IMPROVE | 5,500 | Self-improvement flywheel |
| HTE (terminal) | 2,430 | Rich UI widgets, vendored as lib-hte |
| Buddy/mascots | 2,267 | Olibia sprite system |
| Dreams | 364 | LLM consolidation |
| Frozen memory | ~500 | Session-level LLM injection |
| LLM omni | ~300 | Multimodal via switchAILocal |
| MCP server | 1,763 | Python legacy, Rust `makakoo-mcp` exists |

## Not Yet Ported (should be Rust eventually)

| Component | Python file | Priority |
|-----------|-------------|----------|
| `superbrain sync` | `core/superbrain/ingest.py` | Medium — Brain indexing |
| `superbrain remember` | `core/superbrain/superbrain.py` | Medium — event logging |
| `superbrain context/stack/gods` | various | Low — CLI convenience |
| Auto-memory router | `core/memory/auto_memory_router.py` | Medium → SANCHO task |
| Per-project infect | `core/orchestration/infect.py` | Low |
