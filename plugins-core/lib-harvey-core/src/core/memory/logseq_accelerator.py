#!/usr/bin/env python3
"""
Logseq accelerator — OPTIONAL speed-up layer for the Brain.

When the Logseq desktop app is running locally (port 12315 by default) with
the HTTP API enabled, this module can satisfy rich queries that the pure
filesystem path cannot answer efficiently:

  - Datalog queries against the Logseq graph (`execute_query`)
  - Full block trees with metadata (`get_page_blocks_tree`)

All write operations still flow through `brain_bridge`, which writes
markdown files directly and treats this module as a best-effort cache
invalidator. If Logseq is not running, every function in this module
returns `None` and the Brain keeps working.

This is deliberately a single-purpose accessor — never import it from
anywhere outside `core/memory/brain_bridge.py`. The contract is: the
Brain owns state, Logseq is a viewer that happens to have an API.
"""

from __future__ import annotations

import json
import sys
import urllib.error
import urllib.request
from typing import Optional


def rpc(method: str, args: Optional[list] = None, *, url: str, token: str,
        timeout: int = 10) -> Optional[dict]:
    """
    Low-level JSON-RPC call to the Logseq HTTP API.
    Returns the parsed JSON response on success, None on any failure.
    Never raises — silent fallback is the contract with brain_bridge.
    """
    if not token:
        return None

    payload = json.dumps({"method": method, "args": args or []}).encode()
    req = urllib.request.Request(
        f"{url}/api",
        data=payload,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {token}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read())
    except urllib.error.URLError as e:
        print(f"[logseq_accelerator] RPC {method}: {e}", file=sys.stderr)
        return None
    except Exception as e:  # defensive — API must never take down the Brain
        print(f"[logseq_accelerator] RPC {method} unexpected: {e}", file=sys.stderr)
        return None


def execute_query(datalog_query: str, *, url: str, token: str) -> Optional[list]:
    """Run a Datalog query. Requires Logseq. Returns None when unavailable."""
    result = rpc("logseq.DB.q", [datalog_query], url=url, token=token)
    if result:
        return result.get("result", [])
    return None


def get_page_blocks_tree(page_name: str, *, url: str, token: str) -> Optional[list]:
    """Get the full block tree of a page. Requires Logseq. Returns None when unavailable."""
    result = rpc("logseq.Editor.get_page_blocks_tree", [page_name], url=url, token=token)
    if result:
        return result.get("result", [])
    return None


# --- Self-test ---------------------------------------------------------------
if __name__ == "__main__":
    import os
    url = os.environ.get("BRAIN_API_URL") or os.environ.get("LOGSEQ_API_URL") or "http://127.0.0.1:12315"
    token = os.environ.get("BRAIN_API_TOKEN") or os.environ.get("LOGSEQ_API_TOKEN") or ""
    print(f"[logseq_accelerator] URL: {url}")
    print(f"[logseq_accelerator] Token: {'set' if token else 'unset'}")
    if token:
        result = rpc("logseq.App.getCurrentGraph", [], url=url, token=token)
        print(f"[logseq_accelerator] Ping: {result}")
    else:
        print("[logseq_accelerator] No token — accelerator inactive (this is fine)")
