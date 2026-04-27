#!/usr/bin/env python3
"""
MemPalace MCP Client — Query MemPalace as external memory from Harvey Superbrain.

MemPalace stores verbatim AI conversation transcripts with palace-metadata
(Wings=people/projects, Rooms=topics, Drawers=chunks). This client lets
Harvey search MemPalace and merge results into Superbrain queries.

Usage:
    from core.superbrain.mempalace_client import MemPalaceClient

    mp = MemPalaceClient()
    results = mp.search("BTC sniper params", top_k=5)
    for r in results:
        print(f"[{r.wing}/{r.room}] {r.text[:100]}")

    # Also get entity knowledge graph
    kg_results = mp.kg_query("BTC", time_window="2026-04")
"""

import json
import logging
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

from core.superbrain import config

log = logging.getLogger("superbrain.mempalace")


@dataclass
class MemPalaceResult:
    """A search result from MemPalace."""

    wing: str
    room: str
    text: str
    source_file: str
    chunk_index: int
    added_by: str
    filed_at: str
    score: float = 1.0


# ─────────────────────────────────────────────────────────────────────────────
#  MCP client (calls mempalace CLI via subprocess)
# ─────────────────────────────────────────────────────────────────────────────


class MemPalaceClient:
    """
    Client for MemPalace's CLI interface.

    MemPalace stores conversation transcripts in ChromaDB with palace metadata.
    This client wraps the `mempalace` CLI to search and retrieve memories.
    """

    def __init__(self, mempalace_dir: str = None):
        HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        self.mempalace_dir = (
            mempalace_dir
            or os.path.join(HARVEY_HOME, "agents", "mempalace")
        )
        self.mcp_config_path = Path(self.mempalace_dir) / ".mcp.json"
        self._check_available()

    def _check_available(self):
        """Check if mempalace CLI is accessible."""
        self.available = False
        try:
            result = subprocess.run(
                ["mempalace", "status"],
                capture_output=True,
                text=True,
                timeout=10,
                cwd=self.mempalace_dir,
            )
            self.available = result.returncode == 0
            log.info("MemPalace available: %s", self.available)
        except FileNotFoundError:
            log.warning("MemPalace CLI not found in PATH")
        except Exception as e:
            log.warning("MemPalace check failed: %s", e)

    def search(
        self, query: str, wing: str = None, room: str = None, top_k: int = 5
    ) -> List[MemPalaceResult]:
        """
        Search MemPalace conversations.

        Args:
            query: Natural language search query
            wing: Optional wing filter (person or project name)
            room: Optional room filter (topic)
            top_k: Number of results

        Returns:
            List of MemPalaceResult with wing/room/text/source
        """
        if not self.available:
            return []

        args = ["mempalace", "search", query, "--top", str(top_k), "--json"]
        if wing:
            args.extend(["--wing", wing])
        if room:
            args.extend(["--room", room])

        try:
            result = subprocess.run(
                args,
                capture_output=True,
                text=True,
                timeout=15,
                cwd=self.mempalace_dir,
            )
            if result.returncode != 0:
                log.warning("MemPalace search failed: %s", result.stderr[:200])
                return []

            output = result.stdout.strip()
            if not output:
                return []

            data = json.loads(output)
            return self._parse_search_results(data)

        except json.JSONDecodeError:
            log.warning("MemPalace returned non-JSON: %s", result.stdout[:200])
            return []
        except Exception as e:
            log.error("MemPalace search error: %s", e)
            return []

    def _parse_search_results(self, data) -> List[MemPalaceResult]:
        """Parse mempalace search JSON output to MemPalaceResult objects."""
        results = []
        items = data if isinstance(data, list) else data.get("results", [])

        for item in items:
            if isinstance(item, dict):
                results.append(
                    MemPalaceResult(
                        wing=item.get("wing", "unknown"),
                        room=item.get("room", "unknown"),
                        text=item.get("text", item.get("content", ""))[:500],
                        source_file=item.get("source_file", ""),
                        chunk_index=item.get("chunk_index", 0),
                        added_by=item.get("added_by", "unknown"),
                        filed_at=item.get("filed_at", ""),
                        score=item.get("score", 1.0),
                    )
                )
        return results

    def kg_query(
        self,
        entity: str,
        predicate: str = None,
        time_window: str = None,
        limit: int = 20,
    ) -> List[dict]:
        """
        Query MemPalace knowledge graph (SQLite temporal triples).

        Args:
            entity: Entity to query (subject or object)
            predicate: Optional predicate filter (e.g., "works_on", "decided")
            time_window: ISO date range "YYYY-MM" or "YYYY-MM:YYYY-MM"
            limit: Max results

        Returns:
            List of triple dicts: {subject, predicate, object, valid_from, valid_to, confidence}
        """
        if not self.available:
            return []

        try:
            result = subprocess.run(
                ["mempalace", "kg", "query", entity, "--limit", str(limit), "--json"],
                capture_output=True,
                text=True,
                timeout=10,
                cwd=self.mempalace_dir,
            )
            if result.returncode != 0:
                return []

            data = json.loads(result.stdout.strip())
            items = data if isinstance(data, list) else data.get("triples", [])

            filtered = []
            for item in items:
                if predicate and item.get("predicate") != predicate:
                    continue
                if time_window:
                    vf = item.get("valid_from", "")
                    if (
                        vf and vf > time_window.split(":")[0]
                        if ":" in time_window
                        else vf > time_window[:7]
                    ):
                        continue
                filtered.append(item)

            return filtered[:limit]

        except Exception as e:
            log.warning("MemPalace KG query failed: %s", e)
            return []

    def list_wings(self) -> List[str]:
        """List all wings (people/projects) in MemPalace."""
        if not self.available:
            return []

        try:
            result = subprocess.run(
                ["mempalace", "list-wings", "--json"],
                capture_output=True,
                text=True,
                timeout=10,
                cwd=self.mempalace_dir,
            )
            if result.returncode != 0:
                return []
            data = json.loads(result.stdout.strip())
            return data if isinstance(data, list) else []
        except Exception:
            return []

    def get_rooms(self, wing: str) -> List[str]:
        """List all rooms in a wing."""
        if not self.available:
            return []

        try:
            result = subprocess.run(
                ["mempalace", "list-rooms", wing, "--json"],
                capture_output=True,
                text=True,
                timeout=10,
                cwd=self.mempalace_dir,
            )
            if result.returncode != 0:
                return []
            data = json.loads(result.stdout.strip())
            return data if isinstance(data, list) else []
        except Exception:
            return []

    def status(self) -> dict:
        """Get MemPalace status — vector counts, KG stats."""
        if not self.available:
            return {"available": False}

        try:
            result = subprocess.run(
                ["mempalace", "status", "--json"],
                capture_output=True,
                text=True,
                timeout=10,
                cwd=self.mempalace_dir,
            )
            if result.returncode == 0:
                return json.loads(result.stdout.strip())
        except Exception:
            pass
        return {"available": False}


# ─────────────────────────────────────────────────────────────────────────────
#  Integration helper for Superbrain query
# ─────────────────────────────────────────────────────────────────────────────


def merge_mempalace_into_results(
    mempalace_results: List[MemPalaceResult],
) -> List[dict]:
    """
    Convert MemPalace results to Superbrain SearchResult format for merging.
    """
    from core.superbrain.providers import SearchHit

    merged = []
    for r in mempalace_results:
        merged.append(
            SearchHit(
                score=r.score * 0.9,  # Slight discount vs Brain authority
                text=f"[{r.wing}/{r.room}] {r.text}",
                title=f"{r.wing}:{r.room}",
                source=f"mempalace:{r.wing}",
                metadata={
                    "wing": r.wing,
                    "room": r.room,
                    "source_file": r.source_file,
                    "chunk_index": r.chunk_index,
                    "added_by": r.added_by,
                },
            )
        )
    return merged


# ─────────────────────────────────────────────────────────────────────────────
#  CLI
# ─────────────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)

    mp = MemPalaceClient()
    print("\nMemPalace Status:", mp.status())

    if mp.available:
        wings = mp.list_wings()
        print(f"\nWings: {wings}")

        if len(sys.argv) > 1:
            query = " ".join(sys.argv[1:])
            print(f"\nSearching: {query}")
            results = mp.search(query, top_k=5)
            for r in results:
                print(f"\n[{r.wing}/{r.room}] {r.text[:200]}")
                print(f"  Source: {r.source_file} | Filed: {r.filed_at}")
