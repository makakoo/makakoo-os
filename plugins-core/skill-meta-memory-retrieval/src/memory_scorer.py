"""
Memory Scorer - Relevance Scoring for Memories

Scores and ranks memories based on session topic, recency, usage frequency,
and tag overlap using a weighted scoring algorithm.
"""

import re
from datetime import datetime, timedelta
from typing import List, Dict, Any, Optional


class MemoryScorer:
    """
    Scores memories for relevance to current session using weighted factors:
    - topic_match (40%): keyword overlap
    - recency_decay (30%): exponential decay over 90 days
    - usage_frequency (15%): access_count normalization
    - tag_overlap (15%): explicit tag matches
    """

    TOPIC_WEIGHT = 0.40
    RECENCY_WEIGHT = 0.30
    FREQUENCY_WEIGHT = 0.15
    TAG_WEIGHT = 0.15

    def __init__(self):
        self.half_life_days = 30  # Half-life for recency decay

    def score_memories(
        self,
        memories: List[Dict],
        session_query: str,
        threshold: float = 0.3
    ) -> List[Dict]:
        """
        Score each memory for relevance to current session.

        Args:
            memories: List of memory dicts with title, content, tags, etc.
            session_query: Current session query/topics
            threshold: Minimum score to include (default 0.3)

        Returns:
            Sorted list of memories with relevance_score, descending
        """
        scored = []
        for mem in memories:
            score = self.calculate_score(mem, session_query)
            if score >= threshold:
                scored.append({**mem, "relevance_score": score})

        return sorted(scored, key=lambda x: x["relevance_score"], reverse=True)

    def calculate_score(self, memory: Dict, query: str) -> float:
        """
        Calculate composite relevance score for a single memory.

        Args:
            memory: Memory dict with fields like title, content, tags, last_interaction, access_count
            query: Session query string

        Returns:
            Composite score 0.0 - 1.0
        """
        score = 0.0

        # Topic match (0-1) * 40%
        topic_score = self._topic_match(memory, query)
        score += topic_score * self.TOPIC_WEIGHT

        # Recency decay (0-1) * 30%
        recency_score = self._recency_decay(memory)
        score += recency_score * self.RECENCY_WEIGHT

        # Usage frequency (0-1) * 15%
        freq_score = self._usage_frequency(memory)
        score += freq_score * self.FREQUENCY_WEIGHT

        # Tag overlap (0-1) * 15%
        tag_score = self._tag_overlap(memory, query)
        score += tag_score * self.TAG_WEIGHT

        return min(score, 1.0)

    def _topic_match(self, memory: Dict, query: str) -> float:
        """
        Keyword overlap between memory and query.
        Uses case-insensitive word matching.
        """
        title = memory.get("title", "") or ""
        content = memory.get("content", "") or ""
        text = f"{title} {content}".lower()

        query_terms = set(query.lower().split())
        content_terms = set(text.split())

        # Filter out very short terms and stopwords
        query_terms = {t for t in query_terms if len(t) > 2}
        content_terms = {t for t in content_terms if len(t) > 2}

        if not query_terms:
            return 0.0

        intersection = query_terms & content_terms
        if not intersection:
            return 0.0

        # Jaccard-like similarity
        union = query_terms | content_terms
        return len(intersection) / len(union)

    def _recency_decay(self, memory: Dict) -> float:
        """
        Exponential decay based on days since last update.
        Uses sqrt decay for gentler half-life (~30 days).
        """
        last_update = (
            memory.get("last_interaction")
            or memory.get("updated_at")
            or memory.get("modified_at")
        )

        if not last_update:
            return 0.3  # Default for undated items

        try:
            if isinstance(last_update, str):
                last_date_raw = last_update.replace("Z", "+00:00")
                # Handle timezone-aware datetimes
                if "+" in last_date_raw or last_date_raw.endswith("+00:00"):
                    last_date = datetime.fromisoformat(last_date_raw).replace(tzinfo=None)
                else:
                    last_date = datetime.fromisoformat(last_date_raw)
            else:
                last_date = last_update
        except (ValueError, TypeError):
            return 0.3

        days_old = (datetime.now() - last_date).days
        if days_old < 0:
            days_old = 0

        # sqrt decay: half-life at ~30 days
        decay_factor = 1.0 - (days_old / 90) ** 0.5
        return max(0.0, min(1.0, decay_factor))

    def _usage_frequency(self, memory: Dict) -> float:
        """
        More access_count = more relevant.
        Capped at 20 accesses for max score.
        """
        access_count = memory.get("access_count", 0) or 0
        return min(1.0, access_count / 20)

    def _tag_overlap(self, memory: Dict, query: str) -> float:
        """
        Explicit topic tags matching session query.
        Max 3 tag matches = full score.
        """
        tags = memory.get("tags", []) or []
        if isinstance(tags, str):
            tags = [t.strip() for t in tags.split(",")]

        tag_set = set(t.lower() for t in tags)
        query_lower = query.lower()

        matches = sum(1 for t in tag_set if t in query_lower)
        return min(1.0, matches / 3)


def filter_and_rank(
    memories: List[Dict],
    session_query: str,
    top_k: int = 20
) -> List[Dict]:
    """
    Convenience function to filter and rank memories.

    Args:
        memories: List of memory dicts
        session_query: Current session query
        top_k: Number of top results to return

    Returns:
        Top k scored memories
    """
    scorer = MemoryScorer()
    scored = scorer.score_memories(memories, session_query)
    return scored[:top_k]


def score_single_memory(
    memory: Dict,
    session_query: str
) -> float:
    """Calculate score for a single memory."""
    scorer = MemoryScorer()
    return scorer.calculate_score(memory, session_query)
