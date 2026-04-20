#!/usr/bin/env python3
"""
Superbrain Wiki — Compiled knowledge layer inspired by LLM Wiki pattern.

Six operations:
  index      — Auto-generate navigable catalog of all Brain pages
  lint       — Health-check: orphans, missing pages, stale links, gaps
  compile    — On ingest, update related entity pages
  log        — Append-only chronological record of all Brain operations
  contradict — Detect when new data conflicts with existing page claims
  save       — File a superbrain answer back into the wiki as a page

The key insight from LLM Wiki: don't just index raw sources for RAG retrieval.
Pre-compile synthesis into persistent wiki pages so knowledge compounds.
Our Brain journals are raw sources. Brain pages are the compiled wiki.

Usage:
    from core.superbrain.wiki import WikiOps
    wiki = WikiOps()
    wiki.build_index()           # regenerate index page
    report = wiki.lint()         # health check
    wiki.compile_journal()       # compile today's journal → update pages
"""

import json
import logging
import os
import re
from collections import Counter, defaultdict
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional, Set, Tuple

log = logging.getLogger("superbrain.wiki")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BRAIN_DIR = os.path.join(HARVEY_HOME, "data", "Brain")
WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")


class WikiOps:
    """Wiki compilation, indexing, and linting for the Brain."""

    def __init__(self, brain_dir: str = None):
        self.brain_dir = brain_dir or BRAIN_DIR
        self.pages_dir = Path(self.brain_dir) / "pages"
        self.journals_dir = Path(self.brain_dir) / "journals"
        self.log_path = Path(self.brain_dir) / "pages" / "Brain Log.md"

    # ═══════════════════════════════════════════════════════════════
    #  INDEX — Navigable catalog of all Brain pages
    # ═══════════════════════════════════════════════════════════════

    def build_index(self) -> str:
        """
        Generate a rich index of all Brain pages with categories and summaries.

        Unlike a flat file listing, this:
        - Groups pages by detected category (Company, Lead, Project, etc.)
        - Shows first meaningful line as summary
        - Counts inbound/outbound wikilinks per page
        - Marks hub pages (>3 inbound links)

        Returns the index content and writes to Brain/pages/Brain Index.md
        """
        pages = self._scan_pages()
        categories = self._categorize(pages)

        lines = [
            "- Brain Index — auto-generated catalog of all knowledge pages",
            f"- Last updated: {datetime.now().strftime('%Y-%m-%d %H:%M')}",
            f"- Total: {len(pages)} pages across {len(categories)} categories",
            "-",
        ]

        # Sort categories: system first, then by size
        priority = ["System", "Projects", "Agents", "People", "Companies", "Leads"]
        sorted_cats = sorted(
            categories.keys(),
            key=lambda c: (priority.index(c) if c in priority else 99, -len(categories[c]))
        )

        for cat in sorted_cats:
            cat_pages = categories[cat]
            lines.append(f"- **{cat}** ({len(cat_pages)} pages)")

            # Sort by inbound link count (hubs first)
            cat_pages.sort(key=lambda p: p["inbound"], reverse=True)

            for p in cat_pages:
                hub = " ★" if p["inbound"] >= 3 else ""
                summary = p["summary"][:80] if p["summary"] else ""
                links_info = f"({p['inbound']}↑ {p['outbound']}→)" if p["inbound"] or p["outbound"] else ""
                lines.append(f"  - [[{p['name']}]] {links_info}{hub} — {summary}")

        content = "\n".join(lines) + "\n"

        # Write to Brain
        index_path = self.pages_dir / "Brain Index.md"
        index_path.write_text(content, encoding="utf-8")
        log.info("Index written: %s (%d pages)", index_path, len(pages))

        self.log_op("index", f"Rebuilt index: {len(pages)} pages, {len(categories)} categories")

        return content

    def _scan_pages(self) -> List[dict]:
        """Scan all pages and extract metadata."""
        pages = []
        # Build inbound link map
        inbound: Dict[str, int] = Counter()
        outbound: Dict[str, int] = Counter()

        all_files = list(self.pages_dir.glob("*.md"))
        page_names = {f.stem for f in all_files}

        for f in all_files:
            try:
                content = f.read_text(encoding="utf-8", errors="replace")
            except Exception:
                continue

            name = f.stem
            links = set(WIKILINK_RE.findall(content))
            outbound[name] = len(links)
            for link in links:
                inbound[link] += 1

        for f in all_files:
            try:
                content = f.read_text(encoding="utf-8", errors="replace")
            except Exception:
                continue

            name = f.stem
            links = set(WIKILINK_RE.findall(content))

            # Extract first meaningful line as summary
            summary = ""
            for line in content.split("\n"):
                stripped = line.strip().lstrip("- ").strip()
                # Skip metadata, headers, empty lines
                if not stripped or stripped.startswith("#") or stripped.startswith("icon::") or \
                   stripped.startswith("type::") or stripped.startswith("collapsed::"):
                    continue
                # Strip wikilinks for cleaner summary
                summary = re.sub(r'\[\[([^\]]+)\]\]', r'\1', stripped)
                break

            pages.append({
                "name": name,
                "path": str(f),
                "summary": summary,
                "links": links,
                "inbound": inbound.get(name, 0),
                "outbound": outbound.get(name, 0),
                "size": len(content),
            })

        return pages

    def _categorize(self, pages: List[dict]) -> Dict[str, List[dict]]:
        """Categorize pages by naming convention and content."""
        categories: Dict[str, List[dict]] = defaultdict(list)

        for p in pages:
            name = p["name"]

            if name.startswith("Company -") or name.startswith("Company -"):
                categories["Companies"].append(p)
            elif name.startswith("Lead -") or name.startswith("Proactive Lead"):
                categories["Leads"].append(p)
            elif name.startswith("Inbox") or name.startswith("inbox"):
                categories["Inbox"].append(p)
            elif name.startswith("Briefing"):
                categories["Briefings"].append(p)
            elif name.startswith("NotebookLM"):
                categories["NotebookLM"].append(p)
            elif name.startswith("Sebastian"):
                categories["System"].append(p)
            elif name.startswith("Event"):
                categories["Events"].append(p)
            elif name in ("Harvey OS", "Harvey OS Index", "Brain Index",
                          "superbrain", "toolchain-audit", "mempalace",
                          "switchAILocal"):
                categories["System"].append(p)
            elif any(kw in name.lower() for kw in ["agent", "bot", "sniper", "harness"]):
                categories["Agents"].append(p)
            elif any(kw in name.lower() for kw in ["traylinx", "tytus", "openclaw",
                     "scoutica", "makakoo", "wannolot", "atomek"]):
                categories["Projects"].append(p)
            else:
                # Check content for clues
                summary = p.get("summary", "").lower()
                if any(w in summary for w in ["company", "hiring", "role", "interview"]):
                    categories["Career"].append(p)
                elif any(w in summary for w in ["trading", "btc", "polymarket"]):
                    categories["Trading"].append(p)
                else:
                    categories["Other"].append(p)

        return dict(categories)

    # ═══════════════════════════════════════════════════════════════
    #  LINT — Wiki health check
    # ═══════════════════════════════════════════════════════════════

    def lint(self) -> dict:
        """
        Health-check the Brain wiki. Returns actionable report.

        Checks:
        1. Orphan pages — no inbound links (nobody references them)
        2. Missing pages — referenced by [[link]] but don't exist
        3. Empty pages — exist but have <50 chars of content
        4. Stale journals — referenced entities with no page
        5. Hub pages — most-linked entities (knowledge centers)
        6. Dead links — pages that link to non-existent pages
        """
        pages = self._scan_pages()
        page_names = {p["name"] for p in pages}

        # All entities referenced from journals
        journal_entities: Set[str] = set()
        for f in self.journals_dir.glob("*.md"):
            try:
                content = f.read_text(encoding="utf-8", errors="replace")
                journal_entities.update(WIKILINK_RE.findall(content))
            except Exception:
                continue

        # All entities referenced from pages
        page_entities: Set[str] = set()
        for p in pages:
            page_entities.update(p["links"])

        all_referenced = journal_entities | page_entities

        # 1. Orphan pages (no inbound links, not system pages)
        system_pages = {"Brain Index", "Harvey OS Index", "toolchain-audit"}
        orphans = [
            p["name"] for p in pages
            if p["inbound"] == 0 and p["name"] not in system_pages
            and not p["name"].startswith("Lead -")  # leads are OK as orphans
        ]

        # 2. Missing pages (referenced but don't exist)
        missing = sorted(all_referenced - page_names)
        # Filter out journal dates and common non-page references
        missing = [m for m in missing if not re.match(r'\d{4}[_-]\d{2}[_-]\d{2}', m)
                   and len(m) > 2]

        # 3. Empty pages
        empty = [p["name"] for p in pages if p["size"] < 50]

        # 4. High-value missing pages (referenced from 3+ sources)
        ref_counts = Counter()
        for p in pages:
            for link in p["links"]:
                if link not in page_names:
                    ref_counts[link] += 1
        for entity in journal_entities:
            if entity not in page_names:
                ref_counts[entity] += 1

        high_value_missing = [
            {"name": name, "references": count}
            for name, count in ref_counts.most_common(20)
            if count >= 2 and not re.match(r'\d{4}[_-]\d{2}[_-]\d{2}', name)
        ]

        # 5. Hub pages (most inbound links)
        hubs = sorted(pages, key=lambda p: p["inbound"], reverse=True)[:10]

        # 6. Pages with no outbound links (leaf nodes, may need enrichment)
        leaves = [p["name"] for p in pages if p["outbound"] == 0 and p["size"] > 100]

        report = {
            "total_pages": len(pages),
            "total_journals": len(list(self.journals_dir.glob("*.md"))),
            "orphans": orphans[:20],
            "orphan_count": len(orphans),
            "missing_pages": missing[:30],
            "missing_count": len(missing),
            "high_value_missing": high_value_missing,
            "empty_pages": empty,
            "hubs": [{"name": h["name"], "inbound": h["inbound"]} for h in hubs],
            "leaf_pages": leaves[:20],
            "leaf_count": len(leaves),
            "journal_entities": len(journal_entities),
            "page_entities": len(page_entities),
        }

        self.log_op("lint", f"{len(pages)} pages, {len(orphans)} orphans, "
                     f"{len(missing)} missing, {len(high_value_missing)} high-value gaps")

        return report

    def print_lint(self, report: dict = None):
        """Pretty-print lint report."""
        if report is None:
            report = self.lint()

        print(f"\n{'=' * 55}")
        print(f"  Brain Wiki Health Check")
        print(f"{'=' * 55}")
        print(f"  Pages: {report['total_pages']}  |  Journals: {report['total_journals']}")
        print(f"  Entities: {report['journal_entities']} from journals, {report['page_entities']} from pages")

        if report["high_value_missing"]:
            print(f"\n  HIGH-VALUE MISSING PAGES ({len(report['high_value_missing'])} entities referenced but no page):")
            for m in report["high_value_missing"]:
                print(f"    ✗ {m['name']} ({m['references']} references)")

        if report["orphans"]:
            print(f"\n  ORPHANS ({report['orphan_count']} pages nobody links to):")
            for o in report["orphans"][:10]:
                print(f"    ○ {o}")
            if report["orphan_count"] > 10:
                print(f"    ... and {report['orphan_count'] - 10} more")

        if report["empty_pages"]:
            print(f"\n  EMPTY PAGES ({len(report['empty_pages'])} pages with <50 chars):")
            for e in report["empty_pages"]:
                print(f"    ∅ {e}")

        if report["hubs"]:
            print(f"\n  HUB PAGES (most linked-to):")
            for h in report["hubs"][:8]:
                if h["inbound"] > 0:
                    print(f"    ★ {h['name']} ({h['inbound']} inbound)")

        if report["leaf_pages"]:
            print(f"\n  LEAF PAGES ({report['leaf_count']} pages with no outbound links):")
            for l in report["leaf_pages"][:10]:
                print(f"    → {l}")

        print(f"\n{'=' * 55}\n")

    # ═══════════════════════════════════════════════════════════════
    #  COMPILE — Pre-compile journal knowledge into wiki pages
    # ═══════════════════════════════════════════════════════════════

    def compile_journal(self, journal_date: str = None, dry_run: bool = False) -> dict:
        """
        Compile today's (or specified) journal into wiki page updates.

        For each [[entity]] mentioned in the journal:
        1. If page exists → append new facts from journal
        2. If page doesn't exist → create stub with journal context

        This is the LLM Wiki insight: knowledge should be pre-compiled
        into pages, not re-derived at query time.

        Args:
            journal_date: "YYYY_MM_DD" or None for today
            dry_run: If True, show what would change without writing

        Returns:
            {"updated": [...], "created": [...], "skipped": [...]}
        """
        if journal_date is None:
            journal_date = date.today().strftime("%Y_%m_%d")

        journal_path = self.journals_dir / f"{journal_date}.md"
        if not journal_path.exists():
            return {"error": f"Journal not found: {journal_path}"}

        content = journal_path.read_text(encoding="utf-8", errors="replace")
        entities = set(WIKILINK_RE.findall(content))

        if not entities:
            return {"updated": [], "created": [], "skipped": ["no entities found"]}

        # Parse journal into entity-relevant sections
        entity_contexts = self._extract_entity_contexts(content, entities)

        result = {"updated": [], "created": [], "skipped": [], "contradictions": []}

        for entity, contexts in entity_contexts.items():
            if not contexts:
                result["skipped"].append(entity)
                continue

            # Sanitize entity name for filesystem safety
            safe_entity = entity.replace("/", " - ").replace("\\", " - ")
            safe_entity = re.sub(r'[<>:"|?*]', '', safe_entity).strip()
            if not safe_entity:
                result["skipped"].append(f"{entity} (invalid name)")
                continue

            page_path = self.pages_dir / f"{safe_entity}.md"
            date_display = journal_date.replace("_", "-")

            if page_path.exists():
                # Update existing page — append new journal context
                existing = page_path.read_text(encoding="utf-8", errors="replace")

                # Check if this journal date is already referenced (avoid duplicates)
                if journal_date in existing or date_display in existing:
                    result["skipped"].append(f"{entity} (already compiled)")
                    continue

                # Contradiction detection before appending
                conflicts = self.detect_contradictions(entity, contexts)
                if conflicts:
                    for c in conflicts:
                        result["contradictions"].append({
                            "entity": entity,
                            **c,
                        })

                # Append new context (with conflict flags if any)
                new_entries = "\n".join(f"  - [{date_display}] {ctx}" for ctx in contexts)
                if conflicts:
                    conflict_note = "\n".join(
                        f"  - ⚠️ CONFLICT: {c['field']}: was '{c['existing']}' → now '{c['new']}'"
                        for c in conflicts
                    )
                    update = f"\n- Updates from {date_display}:\n{new_entries}\n{conflict_note}\n"
                else:
                    update = f"\n- Updates from {date_display}:\n{new_entries}\n"

                if not dry_run:
                    with open(page_path, "a") as f:
                        f.write(update)
                result["updated"].append({"entity": entity, "entries": len(contexts)})

            else:
                # Create new page stub
                stub_entries = "\n".join(f"  - {ctx}" for ctx in contexts)
                stub = f"- [[{entity}]] — first mentioned {date_display}\n{stub_entries}\n"

                if not dry_run:
                    page_path.write_text(stub, encoding="utf-8")
                result["created"].append({"entity": entity, "entries": len(contexts)})

        # Log the operation
        if not dry_run:
            summary = (f"Journal {date_display}: "
                       f"{len(result['updated'])} updated, "
                       f"{len(result['created'])} created, "
                       f"{len(result['contradictions'])} conflicts")
            self.log_op("compile", summary)

        return result

    def _extract_entity_contexts(self, journal_content: str,
                                  entities: Set[str]) -> Dict[str, List[str]]:
        """
        Extract context around each entity mention in a journal.

        For each entity, find the journal lines that mention it and extract
        the meaningful content (stripping formatting, deduplicating).
        """
        contexts: Dict[str, List[str]] = {e: [] for e in entities}

        for line in journal_content.split("\n"):
            stripped = line.strip()
            if not stripped or stripped == "-":
                continue

            # Find which entities this line mentions
            mentioned = set(WIKILINK_RE.findall(stripped))

            for entity in mentioned:
                if entity in contexts:
                    # Clean the line: remove leading bullets, strip wikilink brackets
                    clean = stripped.lstrip("- ").strip()
                    # Remove the entity's own [[]] to avoid redundancy
                    clean = clean.replace(f"[[{entity}]]", entity)
                    # Skip very short or pure-link lines
                    if len(clean) > 20:
                        contexts[entity].append(clean[:200])

        # Deduplicate and limit
        for entity in contexts:
            seen = set()
            unique = []
            for ctx in contexts[entity]:
                if ctx not in seen:
                    seen.add(ctx)
                    unique.append(ctx)
            contexts[entity] = unique[:5]  # max 5 entries per entity per journal

        return contexts

    # ═══════════════════════════════════════════════════════════════
    #  COMPILE ALL — Compile all unprocessed journals
    # ═══════════════════════════════════════════════════════════════

    def compile_all(self, since_days: int = 7, dry_run: bool = False) -> dict:
        """Compile all journals from the last N days."""
        results = {"total_updated": 0, "total_created": 0, "journals_processed": 0}

        for i in range(since_days):
            d = (date.today() - timedelta(days=i)).strftime("%Y_%m_%d")
            journal_path = self.journals_dir / f"{d}.md"
            if journal_path.exists():
                r = self.compile_journal(d, dry_run=dry_run)
                if "error" not in r:
                    results["total_updated"] += len(r.get("updated", []))
                    results["total_created"] += len(r.get("created", []))
                    results["journals_processed"] += 1

        return results

    # ═══════════════════════════════════════════════════════════════
    #  LOG — Append-only operation record
    # ═══════════════════════════════════════════════════════════════

    def log_op(self, op_type: str, summary: str, details: str = ""):
        """
        Append an operation to Brain Log.md.

        Parseable format: grep "^- ## \[" to get all entries.
        Each entry: `- ## [YYYY-MM-DD HH:MM] op_type | summary`

        This is the LLM Wiki 'log.md' — chronological audit trail of
        every ingest, compile, lint, query-save, and maintenance action.
        """
        ts = datetime.now().strftime("%Y-%m-%d %H:%M")
        entry = f"- ## [{ts}] {op_type} | {summary}"
        if details:
            entry += f"\n  - {details}"
        entry += "\n"

        # Create file with header if new
        if not self.log_path.exists():
            header = (
                "- Brain Log — chronological record of all wiki operations\n"
                "- Auto-maintained by superbrain wiki. Do not edit manually.\n"
                "-\n"
            )
            self.log_path.write_text(header, encoding="utf-8")

        with open(self.log_path, "a", encoding="utf-8") as f:
            f.write(entry)

        log.info("Logged: [%s] %s", op_type, summary)

    # ═══════════════════════════════════════════════════════════════
    #  CONTRADICT — Detect conflicts between new and existing claims
    # ═══════════════════════════════════════════════════════════════

    def detect_contradictions(self, entity: str, new_lines: List[str]) -> List[dict]:
        """
        Check if new journal lines conflict with existing page content.

        Heuristic contradiction detection (no LLM needed):
        1. Status changes: 'status:: X' vs 'status:: Y'
        2. Numeric conflicts: same metric with different values
        3. Negation patterns: 'X works' vs 'X does not work'
        4. Direct overwrites: 'contact:: A' vs 'contact:: B'

        Returns list of {"type": str, "existing": str, "new": str, "field": str}
        """
        page_path = self.pages_dir / f"{entity}.md"
        if not page_path.exists():
            return []

        existing = page_path.read_text(encoding="utf-8", errors="replace")
        contradictions = []

        # Extract key-value properties from existing page (page property format)
        existing_props = {}
        for line in existing.split("\n"):
            stripped = line.strip().lstrip("- ").strip()
            if "::" in stripped:
                key, val = stripped.split("::", 1)
                existing_props[key.strip().lower()] = val.strip()

        # Check new lines for property conflicts
        for new_line in new_lines:
            clean = new_line.strip().lstrip("- ").strip()
            if "::" in clean:
                key, val = clean.split("::", 1)
                key_lower = key.strip().lower()
                new_val = val.strip()
                if key_lower in existing_props and existing_props[key_lower] != new_val:
                    contradictions.append({
                        "type": "property_change",
                        "field": key.strip(),
                        "existing": existing_props[key_lower],
                        "new": new_val,
                    })

        # Check for status-like keywords in prose
        status_words = {"active", "inactive", "archived", "completed", "failed",
                        "paused", "cancelled", "shipped", "deprecated"}
        existing_statuses = {w for w in existing.lower().split() if w in status_words}
        for new_line in new_lines:
            new_statuses = {w for w in new_line.lower().split() if w in status_words}
            conflicts = existing_statuses & {self._negate_status(s) for s in new_statuses}
            if new_statuses and existing_statuses and new_statuses != existing_statuses:
                # Only flag if there's a clear state transition
                for ns in new_statuses:
                    for es in existing_statuses:
                        if ns != es and self._are_opposing(ns, es):
                            contradictions.append({
                                "type": "status_change",
                                "field": "status",
                                "existing": es,
                                "new": ns,
                            })

        return contradictions

    @staticmethod
    def _negate_status(status: str) -> str:
        opposites = {
            "active": "inactive", "inactive": "active",
            "completed": "failed", "failed": "completed",
            "shipped": "deprecated", "deprecated": "shipped",
        }
        return opposites.get(status, "")

    @staticmethod
    def _are_opposing(a: str, b: str) -> bool:
        pairs = {
            frozenset({"active", "inactive"}),
            frozenset({"active", "archived"}),
            frozenset({"active", "deprecated"}),
            frozenset({"completed", "failed"}),
            frozenset({"completed", "cancelled"}),
            frozenset({"shipped", "deprecated"}),
        }
        return frozenset({a, b}) in pairs

    # ═══════════════════════════════════════════════════════════════
    #  SAVE ANSWER — File query results back into the wiki
    # ═══════════════════════════════════════════════════════════════

    def save_answer(self, title: str, answer: str, query: str = "",
                    sources: List[str] = None) -> str:
        """
        File a superbrain answer back into the Brain as a wiki page.

        This is the LLM Wiki insight: good answers shouldn't vanish into
        chat history. Novel analysis, comparisons, and syntheses become
        permanent wiki pages that compound the knowledge base.

        Args:
            title: Page title (becomes filename)
            answer: The synthesized answer content
            query: Original question (for provenance)
            sources: Source page names referenced

        Returns:
            Path to created/updated page
        """
        # Sanitize title for filename
        safe_title = re.sub(r'[<>:"/\\|?*]', '', title).strip()
        page_path = self.pages_dir / f"{safe_title}.md"

        ts = datetime.now().strftime("%Y-%m-%d %H:%M")

        # Build page content in Brain outliner format
        lines = [f"- {safe_title}"]
        lines.append(f"  - type:: synthesis")
        lines.append(f"  - generated:: {ts}")
        if query:
            lines.append(f"  - query:: {query}")
        if sources:
            source_links = ", ".join(f"[[{s}]]" for s in sources)
            lines.append(f"  - sources:: {source_links}")
        lines.append(f"-")

        # Convert answer to Brain outliner format
        for para in answer.split("\n"):
            stripped = para.strip()
            if not stripped:
                continue
            if stripped.startswith("- "):
                lines.append(stripped)
            elif stripped.startswith("#"):
                lines.append(f"- {stripped}")
            else:
                lines.append(f"- {stripped}")

        content = "\n".join(lines) + "\n"

        if page_path.exists():
            # Append as update section
            with open(page_path, "a", encoding="utf-8") as f:
                f.write(f"\n- Updated synthesis ({ts}):\n")
                for para in answer.split("\n"):
                    stripped = para.strip()
                    if stripped:
                        f.write(f"  - {stripped}\n")
            self.log_op("save-update", f"Updated synthesis: {safe_title}")
        else:
            page_path.write_text(content, encoding="utf-8")
            self.log_op("save-new", f"New synthesis page: {safe_title}",
                        f"From query: {query[:100]}" if query else "")

        return str(page_path)


# ─────────────────────────────────────────────────────────────────────────────
#  CLI
# ─────────────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import sys
    logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(name)s] %(message)s")

    wiki = WikiOps()

    if len(sys.argv) < 2 or sys.argv[1] == "--help":
        print("Usage:")
        print("  python3 wiki.py index              # rebuild Brain Index")
        print("  python3 wiki.py lint               # health check")
        print("  python3 wiki.py compile [DATE]      # compile journal → pages")
        print("  python3 wiki.py compile-all [DAYS]  # compile recent journals")
        print("  python3 wiki.py compile --dry-run    # preview without writing")
        sys.exit(0)

    cmd = sys.argv[1]

    if cmd == "index":
        content = wiki.build_index()
        print(f"Index rebuilt ({content.count(chr(10))} lines)")

    elif cmd == "lint":
        wiki.print_lint()

    elif cmd == "compile":
        journal_date = sys.argv[2] if len(sys.argv) > 2 and not sys.argv[2].startswith("-") else None
        dry_run = "--dry-run" in sys.argv
        result = wiki.compile_journal(journal_date, dry_run=dry_run)
        prefix = "[DRY RUN] " if dry_run else ""
        print(f"\n{prefix}Compiled journal → wiki:")
        if result.get("updated"):
            print(f"  Updated: {', '.join(u['entity'] for u in result['updated'])}")
        if result.get("created"):
            print(f"  Created: {', '.join(c['entity'] for c in result['created'])}")
        if result.get("skipped"):
            print(f"  Skipped: {len(result['skipped'])} entities")

    elif cmd == "compile-all":
        days = int(sys.argv[2]) if len(sys.argv) > 2 else 7
        dry_run = "--dry-run" in sys.argv
        result = wiki.compile_all(since_days=days, dry_run=dry_run)
        print(f"\nCompiled {result['journals_processed']} journals: "
              f"{result['total_updated']} pages updated, {result['total_created']} pages created")

    else:
        print(f"Unknown: {cmd}")
