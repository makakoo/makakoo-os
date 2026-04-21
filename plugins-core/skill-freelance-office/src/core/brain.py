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
"""
from __future__ import annotations

from typing import Optional

from .errors import FreelanceError


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
    """Inspect today's journal doc; return True if the exact line is
    already among its last ~5 lines. Silent fallback if brain_recent
    raises or we lack brain/read — better to possibly double-log than
    to fail the whole subcommand."""
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
    """Append ``line`` to today's Brain journal. Returns the path
    written to on success, or ``None`` if the line was deduped out.

    Raises :class:`FreelanceError` on capability denial / server error.
    """
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
