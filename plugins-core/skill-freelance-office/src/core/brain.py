"""Canonical brain/write protocol wrapper.

Uses the Makakoo Python client (`makakoo_client.Client`) which ships
as the `lib-makakoo-client` library plugin — on PYTHONPATH by default
when `makakoo skill freelance-office …` dispatches this plugin.

The kernel auto-prefixes ``- `` and auto-routes to
``~/MAKAKOO/data/Brain/journals/{YYYY_MM_DD}.md``. Every successful
write-side subcommand should emit ONE outliner line tagged with
``[[freelance-office]]``.

Dedupe: before writing, we read the last few lines of today's journal
via ``brain_recent(limit=1, doc_type="journal")`` and skip if the
exact line is already present. This tolerates idempotent retries.

--- brain/page writes ---

All page writes go to ``~/MAKAKOO/data/Brain/pages/<slug>/<slug>.md``.
Paths are absolute so any CLI can follow them. Both journal and page
writes are non-fatal — failures are logged as warnings and the
subcommand continues.
"""
from __future__ import annotations

import os
from datetime import date
from pathlib import Path
from typing import Any, Dict, List, Optional

from jinja2 import Environment, FileSystemLoader, StrictUndefined

from .errors import FreelanceError

# Resolve MAKAKOO_HOME once at module load time.
_MAKAKOO_HOME = Path(os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME") or str(Path.home() / "MAKAKOO"))
_BRAIN_PAGES = _MAKAKOO_HOME / "data" / "Brain" / "pages"
_PLUGIN_TEMPLATES = Path(__file__).resolve().parent.parent / "templates"


def _jinja_env() -> Environment:
    return Environment(
        loader=FileSystemLoader(str(_PLUGIN_TEMPLATES)),
        trim_blocks=True,
        lstrip_blocks=True,
        undefined=StrictUndefined,
        keep_trailing_newline=True,
    )


# ---------------------------------------------------------------------------
# Journal
# ---------------------------------------------------------------------------

def _client():
    try:
        from makakoo_client import Client, CapabilityDenied, ServerError  # noqa: F401
    except ImportError as e:
        raise FreelanceError(
            "makakoo_client library plugin missing — install lib-makakoo-client "
            "(`makakoo plugin install --core lib-makakoo-client`)"
        ) from e
    return Client.connect_from_env()


def _is_duplicate(client, line: str) -> bool:
    try:
        hits = client.brain_recent(limit=1, doc_type="journal")
        if not hits:
            return False
        first = hits[0]
        doc_id = first.get("id") or first.get("doc_id")
        if not doc_id:
            return False
        doc = client.brain_read(doc_id)
        if not doc:
            return False
        content = doc.get("content") or doc.get("body") or ""
        if not content:
            return False
        tail = [ln.strip() for ln in content.splitlines()[-5:]]
        probe = line.strip()
        if not probe.startswith("- "):
            probe = "- " + probe
        for candidate in tail:
            if candidate == probe or candidate == probe[2:] or candidate == "- " + probe:
                return True
        return False
    except Exception:
        return False


def append_journal_line(line: str) -> Optional[str]:
    line = line.strip()
    if not line:
        raise FreelanceError("refusing to journal an empty line")

    try:
        from makakoo_client import CapabilityDenied, ServerError
    except ImportError as e:
        raise FreelanceError(
            "makakoo_client library plugin missing — install lib-makakoo-client"
        ) from e

    client = _client()
    try:
        if _is_duplicate(client, line):
            return None
        path = client.brain_write_journal(line)
        return path
    except CapabilityDenied as e:
        raise FreelanceError(
            f"brain/write denied by kernel — add `brain/write` to plugin.toml grants ({e})"
        ) from e
    except ServerError as e:
        raise FreelanceError(f"brain/write kernel error: {e}") from e
    finally:
        try:
            client.close()
        except Exception:
            pass


# ---------------------------------------------------------------------------
# Brain pages (filesystem direct — no makakoo_client needed)
# ---------------------------------------------------------------------------

def create_client_brain_page(
    client_slug: str,
    client_name: str,
    sector: str = "",
    contact_email: str = "",
    website: str = "",
    day_rate: int = 0,
    payment_terms_days: int = 30,
    status: str = "prospecting",
    nda_status: str = "not signed",
    projects: Optional[List[Dict[str, Any]]] = None,
) -> str:
    """Render and write a client brain page to
    ``~/MAKAKOO/data/Brain/pages/<slug>/<slug>.md``.

    Returns the path written. Non-fatal — on write error, raises
    FreelanceError.
    """
    env = _jinja_env()
    ctx = {
        "client_slug": client_slug,
        "client_name": client_name,
        "sector": sector,
        "contact_email": contact_email,
        "website": website,
        "day_rate": day_rate,
        "payment_terms_days": payment_terms_days,
        "status": status,
        "nda_status": nda_status,
        "started": str(date.today()),
        "projects": projects or [],
    }
    rendered = env.get_template("CLIENT_BRAIN_PAGE.md.j2").render(**ctx)
    page_dir = _BRAIN_PAGES / client_slug
    page_dir.mkdir(parents=True, exist_ok=True)
    page_path = page_dir / f"{client_slug}.md"
    try:
        page_path.write_text(rendered, encoding="utf-8")
    except OSError as e:
        raise FreelanceError(f"failed to write client brain page: {e}") from e
    return str(page_path)


def create_project_brain_page(
    client_slug: str,
    project_slug: str,
    project_title: str,
    description: str = "",
    day_rate: int = 0,
    total_days: int = 0,
    total_contract_value: int = 0,
    phase: str = "prospecting",
    contract_version: int = 1,
    milestones: Optional[List[Dict[str, Any]]] = None,
) -> str:
    """Render and write a project brain page to
    ``~/MAKAKOO/data/Brain/pages/<slug>/<project>.md``.

    Returns the path written. Non-fatal — on write error, raises
    FreelanceError.
    """
    env = _jinja_env()
    ctx = {
        "client_slug": client_slug,
        "project_slug": project_slug,
        "project_title": project_title,
        "description": description,
        "day_rate": day_rate,
        "total_days": total_days,
        "total_contract_value": total_contract_value,
        "phase": phase,
        "contract_version": contract_version,
        "started": str(date.today()),
        "milestones": milestones or [],
        "milestones_count": len(milestones) if milestones else 0,
    }
    rendered = env.get_template("PROJECT_BRAIN_PAGE.md.j2").render(**ctx)
    page_dir = _BRAIN_PAGES / client_slug
    page_dir.mkdir(parents=True, exist_ok=True)
    page_path = page_dir / f"{project_slug}.md"
    try:
        page_path.write_text(rendered, encoding="utf-8")
    except OSError as e:
        raise FreelanceError(f"failed to write project brain page: {e}") from e
    return str(page_path)


def append_project_to_client_brain(client_slug: str, project: Dict[str, Any]) -> None:
    """Append a project row to the client brain page's Active Projects table.
    If the client page doesn't exist, this is a no-op.
    """
    page_path = _BRAIN_PAGES / client_slug / f"{client_slug}.md"
    if not page_path.exists():
        return
    try:
        content = page_path.read_text(encoding="utf-8")
    except OSError:
        return
    marker = "| Project | Phase | Volume | Notes |"
    new_row = (
        f"| [[{project['slug']}]] | {project.get('phase', '')} "
        f"| €{project.get('value', '')} | {project.get('notes', '')} |"
    )
    if new_row in content:
        return  # already present
    if marker in content:
        content = content.replace(marker, marker + "\n" + new_row)
        try:
            page_path.write_text(content, encoding="utf-8")
        except OSError:
            pass
