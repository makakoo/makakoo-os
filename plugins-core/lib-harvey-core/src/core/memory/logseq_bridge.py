#!/usr/bin/env python3
"""
DEPRECATED shim — kept for backward compatibility with older imports.

The canonical module is now `core.memory.brain_bridge`. The optional
Logseq HTTP API path has been split out into `core.memory.logseq_accelerator`.

Existing code that does::

    from core.memory import logseq_bridge
    logseq_bridge.log_to_today_journal("...")

keeps working through this shim. New code should import directly from
`brain_bridge` instead. A `DeprecationWarning` is emitted once per process.
"""

from __future__ import annotations

import warnings as _warnings

_warnings.warn(
    "core.memory.logseq_bridge is deprecated; import from core.memory.brain_bridge instead "
    "(Logseq-specific calls live in core.memory.logseq_accelerator).",
    DeprecationWarning,
    stacklevel=2,
)

# Re-export the full public surface so every existing call site keeps working.
from core.memory.brain_bridge import (  # noqa: F401
    BRAIN_DIR,
    PAGES_DIR,
    JOURNALS_DIR,
    BRAIN_API_URL,
    BRAIN_API_TOKEN,
    Brain,
    LogseqBrain,
    _page_file_path,
    _journal_file_path,
    _try_accelerator,
    create_page,
    create_journal_page,
    append_block,
    upsert_property,
    upsert_page_properties,
    search,
    get_all_pages,
    link,
    execute_query,
    get_page_blocks_tree,
    sync_lead_to_brain,
    sync_company_to_brain,
    sync_inbound_to_brain,
    append_to_page,
    log_to_today_journal,
)

# Legacy env-var aliases referenced by some external scripts/docs.
LOGSEQ_API_URL = BRAIN_API_URL
LOGSEQ_API_TOKEN = BRAIN_API_TOKEN


if __name__ == "__main__":
    print("[logseq_bridge] DEPRECATED — use `python -m core.memory.brain_bridge` instead.")
    from core.memory import brain_bridge as _bb
    _bb.__dict__.get("__name__")  # keep linters quiet
