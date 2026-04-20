"""
Memory Loader - Pre-Session Context Loading System

Loads relevant context from the Brain before each session using
a 6-layer priority retrieval strategy.
"""

import os
import json
import requests
from datetime import datetime, timedelta
from typing import Optional, List, Dict, Any

_HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_DIR = os.path.join(_HARVEY_HOME, "data", "Brain")
BRAIN_API = "http://127.0.0.1:12315"
LOGSEQ_BRIDGE_PATH = os.path.join(_HARVEY_HOME, "harvey-os", "skills", "logseq-brain", "logseq_bridge.py")

LAYER_CONFIGS = {
    "identity": {
        "pages": ["Sebastian - Profile", "Sebastian - Preferences", "Sebastian - Insights"],
        "max_tokens": 2000,
        "priority": 1
    },
    "today": {
        "journal_date": lambda: datetime.now().strftime("%Y_%m_%d"),
        "max_tokens": 4000,
        "priority": 2
    },
    "active_projects": {
        "max_tokens": 4000,
        "priority": 3
    },
    "recent_leads": {
        "limit": 5,
        "max_tokens": 4000,
        "priority": 4
    },
    "pending_actions": {
        "tags": ["action", "pending", "waiting"],
        "max_tokens": 2000,
        "priority": 5
    },
    "recent_context": {
        "journal_count": 3,
        "max_tokens": 6000,
        "priority": 6
    }
}


def _tokens_from_text(text: str) -> int:
    """Rough token estimate: 1 token ≈ 4 characters."""
    return len(text) // 4


def _load_via_api(endpoint: str, method: str = "GET", data: dict = None) -> Optional[dict]:
    """Try loading data from Brain API (optional Logseq)."""
    try:
        url = f"{BRAIN_API}{endpoint}"
        if method == "GET":
            resp = requests.get(url, timeout=5)
        else:
            resp = requests.post(url, json=data, timeout=5)
        if resp.status_code == 200:
            return resp.json()
    except Exception:
        pass
    return None


def _load_page_via_api(page_name: str) -> Optional[str]:
    """Load a page's content via Brain API (optional Logseq)."""
    result = _load_via_api(f"/api/page/{page_name}")
    if result and "blocks" in result:
        blocks = result["blocks"]
        return "\n".join(_extract_block_text(b) for b in blocks)
    return None


def _extract_block_text(block: dict) -> str:
    """Extract text content from a block dict."""
    content = block.get("content", "")
    children = block.get("children", [])
    if children:
        child_texts = [_extract_block_text(c) for c in children]
        content = content + "\n" + "\n".join(child_texts)
    return content


def _load_page_from_file(page_name: str) -> Optional[str]:
    """Fallback: load page content from Brain/pages/ directory."""
    safe_name = page_name.replace("/", "_").replace("\\", "_")
    page_path = os.path.join(BRAIN_DIR, "pages", f"{safe_name}.md")
    if os.path.exists(page_path):
        with open(page_path, "r") as f:
            return f.read()
    return None


def _load_journal_from_file(journal_date: str) -> Optional[str]:
    """Fallback: load journal content from Brain/journals/ directory."""
    journal_path = os.path.join(BRAIN_DIR, "journals", f"{journal_date}.md")
    if os.path.exists(journal_path):
        with open(journal_path, "r") as f:
            return f.read()
    return None


def _load_identity_layer(config: dict) -> str:
    """Layer 1: Load identity pages (Profile, Preferences, Insights)."""
    parts = []
    for page in config["pages"]:
        content = _load_page_via_api(page)
        if not content:
            content = _load_page_from_file(page)
        if content:
            parts.append(f"### {page}\n{content}")
    return "\n\n".join(parts)


def _load_today_layer(config: dict) -> str:
    """Layer 2: Load today's journal entry."""
    journal_date = config["journal_date"]()
    content = _load_via_api(f"/api/journal/{journal_date}")
    if content and "blocks" in content:
        blocks = content["blocks"]
        return "\n".join(_extract_block_text(b) for b in blocks)

    # Fallback to file
    content = _load_journal_from_file(journal_date)
    if content:
        return content

    return f"No journal entry found for {journal_date}"


def _load_active_projects_layer(config: dict) -> str:
    """Layer 3: Load active project pages."""
    # Try via API first
    result = _load_via_api("/api/pages", method="POST", data={
        "filters": {"properties": {"type": "project", "status": "active"}}
    })
    if result and "pages" in result:
        parts = []
        for page in result["pages"][:5]:
            page_name = page.get("name", page.get("title", ""))
            content = _load_page_via_api(page_name)
            if content:
                parts.append(f"### {page_name}\n{content}")
        if parts:
            return "\n\n".join(parts)

    # Fallback: scan pages directory for project files
    pages_dir = os.path.join(BRAIN_DIR, "pages")
    if os.path.exists(pages_dir):
        parts = []
        for fname in os.listdir(pages_dir):
            if fname.endswith(".md"):
                fpath = os.path.join(pages_dir, fname)
                with open(fpath, "r") as f:
                    content = f.read()
                if "type:: project" in content.lower() and "status:: active" in content.lower():
                    parts.append(f"### {fname[:-3]}\n{content}")
        if parts:
            return "\n\n".join(parts[:5])

    return "No active projects found"


def _load_recent_leads_layer(config: dict) -> str:
    """Layer 4: Load recent CRM leads."""
    # Try via API
    result = _load_via_api("/api/pages", method="POST", data={
        "filters": {"type": "career-lead"}
    })
    if result and "pages" in result:
        pages = result["pages"][:config["limit"]]
        parts = []
        for page in pages:
            page_name = page.get("name", page.get("title", ""))
            content = _load_page_via_api(page_name)
            if content:
                parts.append(f"### {page_name}\n{content}")
        if parts:
            return "\n\n".join(parts)

    # Fallback: scan pages directory
    pages_dir = os.path.join(BRAIN_DIR, "pages")
    if os.path.exists(pages_dir):
        parts = []
        for fname in os.listdir(pages_dir):
            if fname.endswith(".md"):
                fpath = os.path.join(pages_dir, fname)
                with open(fpath, "r") as f:
                    content = f.read()
                if "type:: career-lead" in content.lower():
                    parts.append(f"### {fname[:-3]}\n{content}")
        parts = parts[:config["limit"]]
        if parts:
            return "\n\n".join(parts)

    return "No recent leads found"


def _load_pending_actions_layer(config: dict) -> str:
    """Layer 5: Load pending actions (tagged action/pending/waiting)."""
    tags = config["tags"]
    pages_dir = os.path.join(BRAIN_DIR, "pages")
    parts = []

    if os.path.exists(pages_dir):
        for fname in os.listdir(pages_dir):
            if fname.endswith(".md"):
                fpath = os.path.join(pages_dir, fname)
                with open(fpath, "r") as f:
                    content = f.read()
                for tag in tags:
                    if f"#{tag}" in content.lower():
                        parts.append(f"### {fname[:-3]}\n{content[:1000]}")  # Truncate
                        break

    if parts:
        return "\n\n".join(parts[:10])
    return "No pending actions found"


def _load_recent_context_layer(config: dict) -> str:
    """Layer 6: Load last 3 journal entries."""
    parts = []
    today = datetime.now()

    for i in range(1, config["journal_count"] + 1):
        date = today - timedelta(days=i)
        journal_date = date.strftime("%Y_%m_%d")
        content = _load_via_api(f"/api/journal/{journal_date}")
        if content and "blocks" in content:
            blocks = content["blocks"]
            text = "\n".join(_extract_block_text(b) for b in blocks)
        else:
            text = _load_journal_from_file(journal_date) or ""
        if text:
            parts.append(f"### Journal {journal_date}\n{text}")

    return "\n\n".join(parts) if parts else "No recent journals found"


class MemoryLoader:
    """
    Core pre-session loader with 6-layer priority retrieval.
    """

    def __init__(self, brain_api: str = BRAIN_API, brain_dir: str = BRAIN_DIR):
        self.brain_api = brain_api
        self.brain_dir = brain_dir

    def load_session_context(self, available_tokens: int = 80000) -> str:
        """
        Build pre-session context by loading memory layers in priority order.

        Args:
            available_tokens: Token budget for context (default 80k)

        Returns:
            Markdown string ready for system prompt injection
        """
        context_parts = []
        remaining_tokens = available_tokens

        # Sort layers by priority
        sorted_layers = sorted(
            LAYER_CONFIGS.items(),
            key=lambda x: x[1]["priority"]
        )

        for layer_name, config in sorted_layers:
            if remaining_tokens <= 0:
                break

            layer_text = self._load_layer(layer_name, config)
            if not layer_text:
                continue

            layer_tokens = _tokens_from_text(layer_text)
            if layer_tokens <= remaining_tokens:
                context_parts.append(f"\n## {layer_name.upper()}\n{layer_text}")
                remaining_tokens -= layer_tokens

        return "\n".join(context_parts)

    def _load_layer(self, layer_name: str, config: dict) -> str:
        """Route to appropriate layer loader."""
        loaders = {
            "identity": _load_identity_layer,
            "today": _load_today_layer,
            "active_projects": _load_active_projects_layer,
            "recent_leads": _load_recent_leads_layer,
            "pending_actions": _load_pending_actions_layer,
            "recent_context": _load_recent_context_layer,
        }
        loader = loaders.get(layer_name)
        if loader:
            return loader(config)
        return ""

    def load_layer_raw(self, layer_name: str) -> str:
        """Load a specific layer by name."""
        config = LAYER_CONFIGS.get(layer_name, {})
        return self._load_layer(layer_name, config)


if __name__ == "__main__":
    loader = MemoryLoader()
    context = loader.load_session_context()
    print(context)
