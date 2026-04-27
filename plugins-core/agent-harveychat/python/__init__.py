"""Reference Python gateway for Makakoo per-slot subagents.

Connects to the Rust supervisor's Unix-socket IPC, runs the LLM
dispatch loop, prefixes every Brain journal line with the slot id,
and preflight-checks tool / path scope before invoking MCP. The Rust
MCP/grant layer remains the authoritative scope enforcer.

Locked by `docs/specs/ipc-contract-v2.md`.
"""

__version__ = "2.0.0"
