#!/usr/bin/env python3
"""
Auto-Memory Consolidator — SANCHO-integrated memory consolidation.

Phase 3 of Auto-Memory system. Runs periodically (via SANCHO) to:
1. Read daily journal entries
2. Extract structured facts
3. Cluster semantically (by embeddings)
4. Extract insights per cluster
5. Update memory files (feedback_*, project_*, research_*)
6. Rebuild knowledge graph

Usage:
    from core.memory.consolidator import ConsolidationEngine
    engine = ConsolidationEngine()
    report = engine.consolidate_daily()
"""

import os
import sys
import json
import logging
from datetime import datetime
from typing import Dict, Any, Optional, List
from pathlib import Path
from dataclasses import dataclass

# Add parent to path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from core.memory import brain_bridge
from core.superbrain import store as sb_store
from core.superbrain import embeddings as sb_embeddings
from core.events import EventBus

log = logging.getLogger("harvey.consolidator")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


@dataclass
class ConsolidationReport:
    """Report from a consolidation run."""
    facts_captured: int
    clusters_created: int
    insights_extracted: int
    memory_files_updated: int
    knowledge_graph_edges_added: int
    duration_sec: float
    timestamp: str


class ConsolidationEngine:
    """
    Consolidates daily captured facts into insights and memory updates.

    Pipeline:
    1. Read today's journal
    2. Parse facts from entries
    3. Embed facts
    4. Cluster by cosine similarity (threshold: 0.75)
    5. Extract insight per cluster (via LLM)
    6. Route insights to memory files
    7. Rebuild knowledge graph
    8. Log metrics
    """

    def __init__(self):
        self.bus = EventBus.instance()
        self.store = sb_store.SuperbrainStore()
        self.memory_dir = Path(HARVEY_HOME) / "data" / "Brain" / "pages"
        self.journals_dir = Path(HARVEY_HOME) / "data" / "Brain" / "journals"

    def _read_today_journal(self) -> str:
        """Read today's journal file and return its content."""
        today = datetime.now().strftime("%Y_%m_%d")
        journal_path = self.journals_dir / f"{today}.md"

        if not journal_path.exists():
            log.debug(f"Journal file not found: {journal_path}")
            return ""

        try:
            return journal_path.read_text(encoding="utf-8")
        except Exception as e:
            log.error(f"Failed to read journal: {e}")
            return ""

    async def consolidate_daily(self) -> ConsolidationReport:
        """
        Run a full consolidation cycle for today's facts.
        Returns a report of what was consolidated.
        """
        start_time = datetime.now()

        try:
            # 1. Read today's journal
            today_journal = self._read_today_journal()
            if not today_journal:
                log.info("No journal entries found for today")
                return ConsolidationReport(
                    facts_captured=0,
                    clusters_created=0,
                    insights_extracted=0,
                    memory_files_updated=0,
                    knowledge_graph_edges_added=0,
                    duration_sec=(datetime.now() - start_time).total_seconds(),
                    timestamp=datetime.now().isoformat(),
                )

            # 2. Parse facts from entries
            facts = self._parse_journal_facts(today_journal)
            if not facts:
                log.info("No facts parsed from journal")
                return ConsolidationReport(
                    facts_captured=0,
                    clusters_created=0,
                    insights_extracted=0,
                    memory_files_updated=0,
                    knowledge_graph_edges_added=0,
                    duration_sec=(datetime.now() - start_time).total_seconds(),
                    timestamp=datetime.now().isoformat(),
                )

            log.info(f"Parsed {len(facts)} facts from today's journal")

            # 3. Cluster facts by semantic similarity
            clusters = await self._cluster_facts(facts)
            if not clusters:
                log.info("No clusters created (facts too dissimilar)")
                return ConsolidationReport(
                    facts_captured=len(facts),
                    clusters_created=0,
                    insights_extracted=0,
                    memory_files_updated=0,
                    knowledge_graph_edges_added=0,
                    duration_sec=(datetime.now() - start_time).total_seconds(),
                    timestamp=datetime.now().isoformat(),
                )

            log.info(f"Created {len(clusters)} clusters from {len(facts)} facts")

            # 4. Extract insights per cluster
            insights = await self._extract_insights(clusters)
            log.info(f"Extracted {len(insights)} insights")

            # 5. Route insights to memory files and update
            memory_files_updated = await self._update_memory_files(insights)
            log.info(f"Updated {memory_files_updated} memory files")

            # 6. Rebuild knowledge graph
            edges_added = await self._rebuild_knowledge_graph(insights)
            log.info(f"Added {edges_added} knowledge graph edges")

            # 6.5 Record consolidation signals for Active Memory promotion
            self._record_consolidation_signals(facts)

            # 7. Log consolidation metrics
            self._log_consolidation_metrics(len(facts), len(clusters), len(insights))

            # 8. Emit consolidation event
            self.bus.publish(
                "memory.consolidated",
                source="consolidation_engine",
                facts_count=len(facts),
                clusters_count=len(clusters),
                insights_count=len(insights),
                timestamp=datetime.now().isoformat(),
            )

            elapsed = (datetime.now() - start_time).total_seconds()
            return ConsolidationReport(
                facts_captured=len(facts),
                clusters_created=len(clusters),
                insights_extracted=len(insights),
                memory_files_updated=memory_files_updated,
                knowledge_graph_edges_added=edges_added,
                duration_sec=elapsed,
                timestamp=datetime.now().isoformat(),
            )

        except Exception as e:
            log.error(f"Consolidation failed: {e}", exc_info=True)
            elapsed = (datetime.now() - start_time).total_seconds()
            return ConsolidationReport(
                facts_captured=0,
                clusters_created=0,
                insights_extracted=0,
                memory_files_updated=0,
                knowledge_graph_edges_added=0,
                duration_sec=elapsed,
                timestamp=datetime.now().isoformat(),
            )

    def _parse_journal_facts(self, journal_text: str) -> List[Dict[str, Any]]:
        """
        Parse structured facts from journal entries.
        Extracts bullets that look like facts (start with -, contain data).
        """
        facts = []
        lines = journal_text.split("\n")

        for i, line in enumerate(lines):
            # Skip empty lines and headers
            if not line.strip() or line.startswith("#"):
                continue

            # Look for bullet points
            if line.startswith("- "):
                fact_text = line[2:].strip()
                # Require minimum length and meaningful content
                if len(fact_text) >= 20:
                    facts.append({
                        "text": fact_text,
                        "line_number": i,
                        "raw_line": line,
                    })

        return facts

    async def _cluster_facts(self, facts: List[Dict[str, Any]]) -> List[List[Dict[str, Any]]]:
        """
        Cluster facts semantically using embeddings.
        Groups facts with cosine similarity > 0.75.
        """
        if not facts:
            return []

        # Embed each fact
        embeddings = []
        for fact in facts:
            try:
                text = fact["text"]
                emb = sb_embeddings.embed_text(text)
                if emb:
                    embeddings.append(emb)
                else:
                    embeddings.append(None)
            except Exception as e:
                log.warning(f"Failed to embed fact: {e}")
                embeddings.append(None)

        # Cluster by similarity
        clusters = []
        used = set()

        for i, emb_i in enumerate(embeddings):
            if i in used or emb_i is None:
                continue

            # Start a new cluster
            cluster = [facts[i]]
            used.add(i)

            # Find similar facts
            for j, emb_j in enumerate(embeddings):
                if j <= i or j in used or emb_j is None:
                    continue

                # Calculate cosine similarity
                similarity = self._cosine_similarity(emb_i, emb_j)
                if similarity > 0.75:
                    cluster.append(facts[j])
                    used.add(j)

            clusters.append(cluster)

        # Add remaining unclustered facts as singleton clusters
        for i, fact in enumerate(facts):
            if i not in used:
                clusters.append([fact])

        return clusters

    async def _extract_insights(self, clusters: List[List[Dict[str, Any]]]) -> List[Dict[str, Any]]:
        """
        Extract insights from fact clusters.
        Generates a summary and determines which memory file should be updated.
        """
        insights = []

        for cluster in clusters:
            try:
                # Combine cluster facts into summary
                cluster_text = "\n".join(f["text"] for f in cluster)

                # Determine insight type and routing
                insight = {
                    "type": self._determine_insight_type(cluster_text),
                    "summary": cluster_text[:200] + ("..." if len(cluster_text) > 200 else ""),
                    "facts": cluster,
                    "entities": self._extract_entities_from_cluster(cluster),
                    "memory_file": self._route_insight(cluster_text),
                    "timestamp": datetime.now().isoformat(),
                }
                insights.append(insight)

            except Exception as e:
                log.error(f"Failed to extract insight from cluster: {e}")

        return insights

    async def _update_memory_files(self, insights: List[Dict[str, Any]]) -> int:
        """
        Update memory files with new insights.
        Routes insights to appropriate memory files based on content.
        """
        files_updated = 0

        for insight in insights:
            try:
                memory_file = insight["memory_file"]
                if not memory_file:
                    continue

                # Read existing memory file if it exists
                file_path = self.memory_dir / f"{memory_file}.md"
                if file_path.exists():
                    existing_content = file_path.read_text()
                else:
                    existing_content = f"---\nname: {memory_file}\ndescription: Auto-updated memory\ntype: feedback\n---\n\n"

                # Append new insight
                timestamp = datetime.now().strftime("%Y-%m-%d %H:%M")
                new_entry = f"\n- [{timestamp}] {insight['summary']}"

                updated_content = existing_content + new_entry

                # Write back to file
                file_path.write_text(updated_content)
                files_updated += 1
                log.debug(f"Updated {memory_file}.md")

            except Exception as e:
                log.error(f"Failed to update memory file: {e}")

        return files_updated

    async def _rebuild_knowledge_graph(self, insights: List[Dict[str, Any]]) -> int:
        """
        Rebuild knowledge graph with new entity relationships from insights.
        """
        try:
            # Use superbrain's existing rebuild_entity_graph method
            self.store.rebuild_entity_graph()

            # Count new edges (rough estimate: entities per insight)
            edges_added = sum(len(i.get("entities", [])) for i in insights)
            return edges_added

        except Exception as e:
            log.error(f"Failed to rebuild knowledge graph: {e}")
            return 0

    def _log_consolidation_metrics(self, facts_count: int, clusters_count: int, insights_count: int):
        """
        Log consolidation metrics to today's journal.
        """
        try:
            entry = f"- [CONSOLIDATION] Processed {facts_count} facts → {clusters_count} clusters → {insights_count} insights"
            brain_bridge.log_to_today_journal(
                entry,
                tags=["consolidation", "auto-memory"]
            )
        except Exception as e:
            log.warning(f"Failed to log metrics: {e}")

    def _determine_insight_type(self, text: str) -> str:
        """Determine insight type from text content."""
        text_lower = text.lower()

        if any(w in text_lower for w in ["decided", "decision", "choose", "chosen", "agreed"]):
            return "decision"
        elif any(w in text_lower for w in ["learned", "learned", "discovered", "realized"]):
            return "learning"
        elif any(w in text_lower for w in ["project", "feature", "built", "implemented"]):
            return "project"
        elif any(w in text_lower for w in ["issue", "problem", "bug", "blocked"]):
            return "blocker"
        else:
            return "event"

    def _extract_entities_from_cluster(self, cluster: List[Dict[str, Any]]) -> List[str]:
        """Extract [[wikilink]] entities from cluster facts."""
        import re
        entities = []

        for fact in cluster:
            text = fact.get("text", "")
            matches = re.findall(r"\[\[([^\]]+)\]\]", text)
            entities.extend(matches)

        return list(set(entities))  # Deduplicate

    def _route_insight(self, text: str) -> Optional[str]:
        """
        Determine which memory file should be updated based on insight content.
        """
        text_lower = text.lower()

        # Simple routing rules
        if any(w in text_lower for w in ["auth", "security", "encryption", "validation"]):
            return "feedback_security"
        elif any(w in text_lower for w in ["superbrain", "search", "embedding", "retrieval"]):
            return "research_superbrain"
        elif any(w in text_lower for w in ["harveychat", "telegram", "chat", "bot"]):
            return "project_harveychat_agent"
        elif any(w in text_lower for w in ["git", "commit", "branch", "merge"]):
            return "feedback_git_workflow"
        elif any(w in text_lower for w in ["test", "testing", "qa", "quality"]):
            return "feedback_testing"
        else:
            # Generic project memory
            return "project_work_in_progress"

    def _record_consolidation_signals(self, facts: List[Dict]) -> None:
        """
        Record consolidation phase signals for Active Memory promotion.

        When SANCHO's dream/consolidation encounters facts, those facts get
        a phase signal boost in the promotion scoring. This creates a virtuous
        cycle: important memories encountered during consolidation become more
        likely to be promoted.
        """
        try:
            from core.memory.recall_tracker import RecallTracker
            tracker = RecallTracker()
            signals_recorded = 0
            for fact in facts:
                text = fact.get("text", "") or ""
                if not text:
                    continue
                snippet = text[:280]
                content_hash = tracker._hash(tracker._normalize(snippet))
                tracker.record_consolidation_hit(content_hash)
                signals_recorded += 1
            if signals_recorded > 0:
                log.info("recorded %d consolidation signals for Active Memory", signals_recorded)
        except Exception as e:
            log.debug("consolidation signal recording failed (non-fatal): %s", e)

    @staticmethod
    def _cosine_similarity(a: List[float], b: List[float]) -> float:
        """Calculate cosine similarity between two vectors."""
        dot = sum(x * y for x, y in zip(a, b))
        norm_a = sum(x * x for x in a) ** 0.5
        norm_b = sum(x * x for x in b) ** 0.5
        if norm_a == 0 or norm_b == 0:
            return 0.0
        return dot / (norm_a * norm_b)


# ═════════════════════════════════════════════════════════════════════════════

if __name__ == "__main__":
    # Test the consolidator
    import asyncio

    logging.basicConfig(level=logging.INFO)

    async def test():
        engine = ConsolidationEngine()
        report = await engine.consolidate_daily()
        print(f"\nConsolidation Report:")
        print(f"  Facts captured: {report.facts_captured}")
        print(f"  Clusters created: {report.clusters_created}")
        print(f"  Insights extracted: {report.insights_extracted}")
        print(f"  Memory files updated: {report.memory_files_updated}")
        print(f"  KG edges added: {report.knowledge_graph_edges_added}")
        print(f"  Duration: {report.duration_sec:.2f}s")

    asyncio.run(test())
