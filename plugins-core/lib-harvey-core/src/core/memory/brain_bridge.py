#!/usr/bin/env python3
"""
Brain bridge — Python interface to Harvey's persistent memory.

The Brain is plain markdown on disk at $MAKAKOO_HOME/data/Brain/:
  - journals/YYYY_MM_DD.md — daily outliner diary
  - pages/Entity_Name.md   — one file per entity

This module owns every read/write path and is **filesystem-first**:
nothing here requires a running database or external service. If the
optional Logseq desktop app is running, the accelerator sibling module
(`logseq_accelerator.py`) can satisfy rich operations (Datalog queries,
block-tree walks) over HTTP — but Brain functionality never depends on it.

Historically this module was named `logseq_bridge.py`. The old name is
still importable via a thin deprecation shim so existing callers keep
working while we migrate the codebase.
"""

from __future__ import annotations

import os
import sys
from datetime import datetime, date
from typing import Any, Optional

# --- Config ------------------------------------------------------------------
# Resolve $MAKAKOO_HOME / $HARVEY_HOME via core.paths when available so the
# tilde-expansion bug class (see memory: embedding_migration) can never bite
# us again. Fall back to manual expansion for standalone script use.
try:
    from core.paths import harvey_home as _harvey_home  # type: ignore
    _HARVEY_HOME = _harvey_home()
except ImportError:
    _HARVEY_HOME = os.path.abspath(os.path.expanduser(
        os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME") or "~/MAKAKOO"
    ))

BRAIN_DIR = os.path.join(_HARVEY_HOME, "data", "Brain")
PAGES_DIR = os.path.join(BRAIN_DIR, "pages")
JOURNALS_DIR = os.path.join(BRAIN_DIR, "journals")

# Accelerator env — `BRAIN_API_*` is the canonical pair; `LOGSEQ_API_*` is the
# legacy alias kept so existing .env files and older scripts keep working.
BRAIN_API_URL = (
    os.environ.get("BRAIN_API_URL")
    or os.environ.get("LOGSEQ_API_URL")
    or "http://127.0.0.1:12315"
)
BRAIN_API_TOKEN = (
    os.environ.get("BRAIN_API_TOKEN")
    or os.environ.get("LOGSEQ_API_TOKEN")
    or ""
)


# --- Internal helpers --------------------------------------------------------

def _page_file_path(page_name: str) -> str:
    """Return the filesystem path for a given page name."""
    safe = page_name.replace("/", "-").replace("\\", "-")
    return os.path.join(PAGES_DIR, f"{safe}.md")


def _journal_file_path(d: date) -> str:
    """Return the filesystem path for a journal date."""
    return os.path.join(JOURNALS_DIR, f"{d.year}_{d.month:02d}_{d.day:02d}.md")


def _try_accelerator(method: str, args: Optional[list] = None) -> Optional[dict]:
    """
    Attempt the same operation via the optional Logseq HTTP accelerator.
    Returns None when the accelerator is unavailable so the caller can fall
    back to direct filesystem writes. Never raises.
    """
    if not BRAIN_API_TOKEN:
        return None
    try:
        from core.memory import logseq_accelerator  # lazy import
    except ImportError:
        return None
    return logseq_accelerator.rpc(method, args, url=BRAIN_API_URL, token=BRAIN_API_TOKEN)


# --- Core API ----------------------------------------------------------------

def create_page(name: str, properties: Optional[dict] = None, content: str = "") -> bool:
    """Create or overwrite a page. Tries accelerator, falls back to filesystem."""
    props = dict(properties or {})
    props.setdefault("type", "page")

    lines = ["---"]
    for k, v in props.items():
        lines.append(f"{k}:: {v}")
    lines.append("---")
    if content:
        lines.append("")
        lines.append(content)
    body = "\n".join(lines)

    if _try_accelerator("logseq.Editor.create_page", [name, body, {"properties": props}]) is not None:
        return True

    try:
        os.makedirs(PAGES_DIR, exist_ok=True)
        with open(_page_file_path(name), "w") as f:
            f.write(body + "\n")
        return True
    except OSError as e:
        print(f"[brain_bridge] Failed to write page {name}: {e}", file=sys.stderr)
        return False


def create_journal_page(d: date, content: str = "", properties: Optional[dict] = None) -> bool:
    """Create a journal page for a given date."""
    props = {"type": "journal", "date": d.isoformat()}
    if properties:
        props.update(properties)

    lines = ["---"]
    for k, v in props.items():
        lines.append(f"{k}:: {v}")
    lines.append("---")
    if content:
        lines.append("")
        lines.append(content)
    body = "\n".join(lines)

    if _try_accelerator("logseq.Editor.create_journal_page", [d.isoformat(), body, {"properties": props}]) is not None:
        return True

    try:
        os.makedirs(JOURNALS_DIR, exist_ok=True)
        with open(_journal_file_path(d), "w") as f:
            f.write(body + "\n")
        return True
    except OSError as e:
        print(f"[brain_bridge] Failed to write journal {d}: {e}", file=sys.stderr)
        return False


def append_block(page_name: str, block_content: str, properties: Optional[dict] = None) -> bool:
    """Append a bullet block to a page. Creates the page if it doesn't exist."""
    if _try_accelerator(
        "logseq.Editor.append_block_in_page",
        [page_name, block_content, {"properties": properties or {}}],
    ) is not None:
        return True

    try:
        os.makedirs(PAGES_DIR, exist_ok=True)
        path = _page_file_path(page_name)
        if os.path.exists(path):
            with open(path, "r") as f:
                content = f.read()
            if not content.endswith("\n"):
                content += "\n"
        else:
            content = "---\ntype:: page\n---\n\n"

        content += f"- {block_content}\n"
        with open(path, "w") as f:
            f.write(content)
        return True
    except OSError as e:
        print(f"[brain_bridge] Failed to append block to {page_name}: {e}", file=sys.stderr)
        return False


def upsert_property(page_name: str, key: str, value: Any) -> bool:
    """Upsert a property on a page. Reads existing content, updates, writes back."""
    try:
        os.makedirs(PAGES_DIR, exist_ok=True)
        path = _page_file_path(page_name)

        if os.path.exists(path):
            with open(path, "r") as f:
                lines = f.read().splitlines()
        else:
            lines = ["---", "type:: page", "---"]

        prop_line = f"{key}::"
        idx = None
        for i, line in enumerate(lines):
            if line.startswith(prop_line):
                idx = i
                break

        if idx is not None:
            lines[idx] = f"{key}:: {value}"
        else:
            insert_at = len(lines)
            for i, line in enumerate(lines):
                if line.strip() == "---" and i > 0:
                    insert_at = i
                    break
            lines.insert(insert_at, f"{key}:: {value}")

        with open(path, "w") as f:
            f.write("\n".join(lines) + "\n")
        return True
    except OSError as e:
        print(f"[brain_bridge] Failed to upsert property {key} on {page_name}: {e}", file=sys.stderr)
        return False


# --- Accelerator-only convenience wrappers -----------------------------------
# These require the optional Logseq HTTP API; they return None otherwise.

def execute_query(datalog_query: str) -> Optional[list]:
    """Execute a Datalog query. Requires the Logseq accelerator."""
    try:
        from core.memory import logseq_accelerator
    except ImportError:
        return None
    return logseq_accelerator.execute_query(datalog_query, url=BRAIN_API_URL, token=BRAIN_API_TOKEN)


def get_page_blocks_tree(page_name: str) -> Optional[list]:
    """Get the full block tree of a page. Requires the Logseq accelerator."""
    try:
        from core.memory import logseq_accelerator
    except ImportError:
        return None
    return logseq_accelerator.get_page_blocks_tree(page_name, url=BRAIN_API_URL, token=BRAIN_API_TOKEN)


# --- CRM-specific helpers (career-manager + knowledge-extractor) -------------

def sync_lead_to_brain(lead: dict) -> bool:
    """Create/update a Lead page from a career-manager lead dict."""
    company = lead.get("company", "Unknown")
    title = lead.get("title", "Unknown Role")
    page_name = f"Lead - {company} - {title}"

    props = {
        "type": "career-lead",
        "status": lead.get("status", "Active"),
        "source": lead.get("source", "unknown"),
        "contact": lead.get("contact", ""),
        "rate": lead.get("rate", ""),
        "last_interaction": datetime.now().date().isoformat(),
    }

    content = f"- # {title} @ [[{company}]]\n"
    if lead.get("url"):
        content += f"- **Link:** {lead['url']}\n"
    if lead.get("contact"):
        content += f"- **Contact:** {lead['contact']}\n"
    if lead.get("rate"):
        content += f"- **Rate:** {lead['rate']}\n"
    if lead.get("notes"):
        content += f"- **Notes:** {lead['notes']}\n"

    return create_page(page_name, props, content.strip())


def sync_company_to_brain(company_name: str, history_content: str = "", status: str = "Active",
                           contact: str = "", last_interaction: str = "") -> bool:
    """Create/update a Company page."""
    page_name = f"Company - {company_name}"
    props = {
        "type": "company",
        "status": status,
        "contact": contact,
        "last_interaction": last_interaction or datetime.now().date().isoformat(),
    }
    content = f"- # {company_name}\n"
    if history_content:
        content += f"\n{history_content}\n"
    return create_page(page_name, props, content.strip())


def sync_inbound_to_brain(sender: str, subject: str, snippet: str, msg_id: str, urgency: str = "High") -> bool:
    """Create an inbox-event page for an urgent inbound message."""
    page_name = f"Inbox - {sender[:40]} - {datetime.now().strftime('%Y%m%d_%H%M%S')}"
    props = {
        "type": "inbox-event",
        "sender": sender,
        "subject": subject,
        "urgency": urgency,
        "msg_id": msg_id,
        "received": datetime.now().isoformat(),
    }
    content = f"- # Inbox Event\n- **From:** {sender}\n- **Subject:** {subject}\n- **Snippet:** {snippet}\n"
    return create_page(page_name, props, content.strip())


# --- Convenience -------------------------------------------------------------

def append_to_page(page_name: str, block_content: str) -> bool:
    """Append a bullet block to a named page, creating it if needed."""
    return append_block(page_name, block_content)


def log_to_today_journal(block_content: str, tags: Optional[list] = None) -> bool:
    """Append a timestamped entry to today's journal page."""
    now = datetime.now()
    d = now.date()
    ts = now.strftime("%H:%M")
    tag_str = " ".join(f"#{t}" for t in (tags or []))
    entry = f"- {ts} — {tag_str} {block_content}".strip()
    return append_block(f"{d.year}_{d.month:02d}_{d.day:02d}", entry)


# --- Class facade ------------------------------------------------------------

class Brain:
    """
    Thin object-oriented facade around the module-level functions.
    Callers can use either style:

        Brain().create_page("Foo", ...)
        create_page("Foo", ...)
    """
    def __init__(self):
        self.url = BRAIN_API_URL
        self.token = BRAIN_API_TOKEN
        self.brain_dir = BRAIN_DIR
        self.pages_dir = PAGES_DIR
        self.journals_dir = JOURNALS_DIR

    def create_page(self, *a, **kw): return create_page(*a, **kw)
    def create_journal_page(self, *a, **kw): return create_journal_page(*a, **kw)
    def append_block(self, *a, **kw): return append_block(*a, **kw)
    def upsert_property(self, *a, **kw): return upsert_property(*a, **kw)
    def execute_query(self, *a, **kw): return execute_query(*a, **kw)
    def get_page_blocks_tree(self, *a, **kw): return get_page_blocks_tree(*a, **kw)
    def sync_lead_to_brain(self, *a, **kw): return sync_lead_to_brain(*a, **kw)
    def sync_company_to_brain(self, *a, **kw): return sync_company_to_brain(*a, **kw)
    def sync_inbound_to_brain(self, *a, **kw): return sync_inbound_to_brain(*a, **kw)
    def append_to_page(self, *a, **kw): return append_to_page(*a, **kw)
    def log_to_today_journal(self, *a, **kw): return log_to_today_journal(*a, **kw)


# Back-compat alias. Keeps `from logseq_bridge import LogseqBrain` working.
LogseqBrain = Brain


# --- Self-test ---------------------------------------------------------------
if __name__ == "__main__":
    print(f"[brain_bridge] BRAIN_DIR: {BRAIN_DIR}")
    print(f"[brain_bridge] PAGES_DIR exists: {os.path.exists(PAGES_DIR)}")
    print(f"[brain_bridge] JOURNALS_DIR exists: {os.path.exists(JOURNALS_DIR)}")
    print(f"[brain_bridge] Accelerator URL: {BRAIN_API_URL}")
    print(f"[brain_bridge] Accelerator token: {'set' if BRAIN_API_TOKEN else 'unset (filesystem-only)'}")
    if BRAIN_API_TOKEN:
        result = _try_accelerator("logseq.App.getCurrentGraph", [])
        print(f"[brain_bridge] Accelerator ping: {result}")
