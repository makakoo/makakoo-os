#!/usr/bin/env python3
"""
Semantic Matcher — Hybrid scoring combining semantic embeddings with keyword matching.
"""

import sys
from pathlib import Path
from typing import Optional

REGISTRY_DIR = Path(__file__).parent
sys.path.insert(0, str(REGISTRY_DIR))

from embedding_service import EmbeddingService, get_instance


# Weights for hybrid scoring
WEIGHTS = {
    "semantic": 0.50,
    "phrase_match": 0.30,
    "tag_overlap": 0.20,
}


class SemanticMatcher:
    """
    Hybrid matcher combining semantic (Chroma) search with keyword-based scoring.

    Falls back to keyword-only scoring if embeddings are unavailable.
    """

    def __init__(self):
        self.embed_service: Optional[EmbeddingService] = None
        self._keyword_weights = {
            "phrase_match": 0.40,
            "semantic": 0.35,
            "tag_overlap": 0.15,
            "category_boost": 0.10,
        }

    def _get_embed_service(self) -> Optional[EmbeddingService]:
        """Lazy-load embedding service."""
        if self.embed_service is None:
            try:
                self.embed_service = get_instance()
            except Exception as e:
                print(f"Warning: Could not initialize embedding service: {e}", file=sys.stderr)
        return self.embed_service

    def match(
        self,
        query: str,
        skills: list[dict],
        top_k: int = 5,
        use_semantic: bool = True,
    ) -> list[dict]:
        """
        Match skills to a query using hybrid scoring.

        Args:
            query: User query string
            skills: List of skill dicts with name, description, tags, etc.
            top_k: Number of results to return
            use_semantic: If False, skip embedding search (keyword-only mode)

        Returns:
            List of dicts with skill info and hybrid score breakdown
        """
        if not skills:
            return []

        # Try semantic search first
        semantic_scores = {}
        embed_service = self._get_embed_service()

        if use_semantic and embed_service:
            try:
                semantic_results = embed_service.search_skills(query, top_k=len(skills))
                for result in semantic_results:
                    semantic_scores[result["name"]] = result["score"]
            except Exception as e:
                print(f"Warning: Semantic search failed: {e}", file=sys.stderr)
                use_semantic = False

        # Compute hybrid scores
        scored = []
        for skill in skills:
            name = skill.get("name", "")
            scores = self._compute_hybrid_score(query, skill, semantic_scores, use_semantic)
            scored.append({
                "skill": skill,
                "scores": scores,
            })

        # Sort by total score descending
        scored.sort(key=lambda x: x["scores"]["total"], reverse=True)

        return scored[:top_k]

    def _compute_hybrid_score(
        self,
        query: str,
        skill: dict,
        semantic_scores: dict,
        use_semantic: bool,
    ) -> dict:
        """Compute hybrid score combining semantic and keyword signals."""
        query_tokens = set(query.lower().split())
        query_tokens = {t.strip('.,!?;:()[]{}"\'-') for t in query_tokens if t.strip()}

        # Semantic score (0.5 weight)
        if use_semantic and skill.get("name") in semantic_scores:
            semantic_score = semantic_scores[skill.get("name")]
        else:
            # Fallback to keyword-only semantic proxy
            semantic_score = self._keyword_semantic_score(query_tokens, skill)

        # Phrase match score (0.3 weight)
        phrase_score = self._phrase_match_score(query_tokens, skill)

        # Tag overlap score (0.2 weight)
        tag_score = self._tag_overlap_score(query_tokens, skill)

        # Weighted total
        total = (
            WEIGHTS["semantic"] * semantic_score +
            WEIGHTS["phrase_match"] * phrase_score +
            WEIGHTS["tag_overlap"] * tag_score
        )

        return {
            "total": total,
            "semantic": semantic_score,
            "phrase_match": phrase_score,
            "tag_overlap": tag_score,
        }

    def _keyword_semantic_score(self, query_tokens: set[str], skill: dict) -> float:
        """Fallback semantic score using keyword overlap."""
        skill_keywords = set(skill.get("keywords", []))
        if not skill_keywords:
            # Use description words as proxy
            description = skill.get("description", "")
            words = description.lower().split()
            stopwords = {
                'a', 'an', 'the', 'and', 'or', 'but', 'in', 'on', 'at', 'to', 'for',
                'of', 'with', 'by', 'from', 'as', 'is', 'was', 'are', 'were', 'been',
                'be', 'have', 'has', 'had', 'do', 'does', 'did', 'will', 'would',
                'could', 'should', 'may', 'might', 'must', 'shall', 'can', 'need',
            }
            skill_keywords = {w for w in words if w not in stopwords and len(w) > 2}

        if not skill_keywords:
            return 0.0

        # Simple overlap with partial matching
        overlap = 0
        for qt in query_tokens:
            for sk in skill_keywords:
                if qt == sk or qt in sk or sk in qt:
                    overlap += 1

        return min(overlap / max(len(skill_keywords), 1), 1.0)

    def _phrase_match_score(self, query_tokens: set[str], skill: dict) -> float:
        """Score based on phrase match with trigger conditions."""
        trigger_conditions = skill.get("trigger_conditions", [])
        if not trigger_conditions:
            return 0.0

        # Filter stopwords
        stopwords = {
            'a', 'an', 'the', 'my', 'your', 'our', 'its', 'this', 'that', 'these', 'those',
            'i', 'you', 'we', 'he', 'she', 'it', 'they', 'me', 'us', 'him', 'her', 'them',
        }
        filtered_query = {t for t in query_tokens if len(t) > 1 and t not in stopwords}
        if not filtered_query:
            return 0.0

        scores = []
        for phrase in trigger_conditions:
            phrase_tokens = set(phrase.lower().split())
            if not phrase_tokens:
                continue

            overlap = len(filtered_query & phrase_tokens)
            score = overlap / len(phrase_tokens)
            scores.append(score)

        return max(scores) if scores else 0.0

    def _tag_overlap_score(self, query_tokens: set[str], skill: dict) -> float:
        """Score based on tag overlap with query tokens."""
        tags = skill.get("tags", [])
        if not tags:
            return 0.0

        tag_set = {t.lower() for t in tags}
        query_lower = {t.lower() for t in query_tokens}

        # Exact overlap
        overlap = len(query_lower & tag_set)

        # Partial substring matches (0.25 each, capped at 50% of tags)
        partial = 0
        for t in query_lower:
            for tag in tag_set:
                if t != tag and (t in tag or tag in t):
                    partial += 0.25

        total = overlap + min(partial, len(tag_set) * 0.5)
        return min(total / max(len(tag_set), 1), 1.5)


def get_matcher() -> SemanticMatcher:
    """Convenience function to get a SemanticMatcher instance."""
    return SemanticMatcher()


if __name__ == "__main__":
    # Quick test
    matcher = get_matcher()
    test_skills = [
        {
            "name": "code-review",
            "description": "Review code changes and pull requests",
            "category": "dev",
            "tags": ["code-review", "github", "pr"],
            "trigger_conditions": ["review code", "check my diff", "pre-landing review"],
            "keywords": ["code", "review", "pull", "request", "diff", "github"],
        },
        {
            "name": "gmail",
            "description": "Send and receive emails via Gmail",
            "category": "productivity",
            "tags": ["email", "gmail", "communication"],
            "trigger_conditions": ["send email", "check emails", "gmail"],
            "keywords": ["email", "mail", "gmail", "message"],
        },
    ]

    results = matcher.match("I want to review my pull request and send an email", test_skills, top_k=5)
    print("Hybrid match results:")
    for r in results:
        print(f"  {r['skill']['name']}: {r['scores']['total']:.3f}")
        print(f"    semantic={r['scores']['semantic']:.2f}, phrase={r['scores']['phrase_match']:.2f}, tag={r['scores']['tag_overlap']:.2f}")
