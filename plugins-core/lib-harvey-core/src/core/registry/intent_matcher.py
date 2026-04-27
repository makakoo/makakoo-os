#!/usr/bin/env python3
"""
Intent Matcher — Multi-signal ranking for skill matching.
"""

import json
import math
import re
import sys
from pathlib import Path
from typing import Optional

# Weights for scoring signals
WEIGHTS = {
    "phrase_match": 0.40,
    "semantic": 0.35,
    "tag_overlap": 0.15,
    "category_boost": 0.10,
}

# Hybrid weights when using embedding service
HYBRID_WEIGHTS = {
    "semantic": 0.50,
    "phrase_match": 0.30,
    "tag_overlap": 0.20,
}

REGISTRY_DIR = Path(__file__).parent
CATEGORY_BOOST_PATH = REGISTRY_DIR / "CATEGORY_BOOST.json"

# Lazy import for embedding service
_embed_service = None
_use_embeddings = True


def _get_embed_service():
    """Lazy-load embedding service."""
    global _embed_service
    if _embed_service is None:
        try:
            sys.path.insert(0, str(REGISTRY_DIR))
            from embedding_service import get_instance
            _embed_service = get_instance()
        except Exception as e:
            print(f"Warning: Could not load embedding service: {e}", file=sys.stderr)
            return None
    return _embed_service


def load_category_boost() -> dict:
    """Load category boost configuration."""
    if CATEGORY_BOOST_PATH.exists():
        return json.loads(CATEGORY_BOOST_PATH.read_text())
    return {}


def cosine_similarity(vec1: list[float], vec2: list[float]) -> float:
    """Compute cosine similarity between two vectors."""
    if len(vec1) != len(vec2) or len(vec1) == 0:
        return 0.0

    dot = sum(a * b for a, b in zip(vec1, vec2))
    mag1 = math.sqrt(sum(a * a for a in vec1))
    mag2 = math.sqrt(sum(b * b for b in vec2))

    if mag1 == 0 or mag2 == 0:
        return 0.0

    return dot / (mag1 * mag2)


# Semantic expansion map for common synonyms/related terms
SEMANTIC_EXPANSION = {
    "email": {"emails", "emailing", "mail", "gmail", "inbox", "message", "messages"},
    "emails": {"email", "emailing", "mail", "gmail", "inbox", "message", "messages"},
    "mail": {"email", "emails", "gmail", "inbox", "message"},
    "check": {"verify", "validate", "monitor", "watch", "test", "review", "inspect"},
    "review": {"check", "verify", "inspect", "audit", "examine", "analyze"},
    "pr": {"pullrequest", "pull-request", "merge", "code-review", "code_review"},
    "deploy": {"deployment", "deploying", "release", "ship", "launch"},
    "test": {"qa", "testing", "verify", "validate", "check"},
    "code": {"programming", "coding", "implementation", "develop"},
    "build": {"compile", "make", "construct", "create"},
    "fix": {"bug", "fixing", "repair", "patch", "debug"},
    "search": {"find", "lookup", "query", "retrieve", "fetch"},
    "plan": {"planning", "strategy", "roadmap", "approach"},
}

# Reverse map for expansion lookup
EXPANSION_MAP = {}
for key, values in SEMANTIC_EXPANSION.items():
    EXPANSION_MAP[key] = values
    for v in values:
        if v not in EXPANSION_MAP:
            EXPANSION_MAP[v] = set()
        EXPANSION_MAP[v].add(key)


def expand_semantic(tokens: set[str]) -> set[str]:
    """Expand tokens with semantically related terms."""
    expanded = set(tokens)
    for token in tokens:
        if token in EXPANSION_MAP:
            expanded.update(EXPANSION_MAP[token])
        # Also add partial matches (e.g., "emails" contains "email")
        for key in EXPANSION_MAP:
            if token in key or key in token:
                expanded.add(key)
                expanded.update(EXPANSION_MAP[key])
    return expanded


def simple_keyword_overlap(text_tokens: set[str], keyword_tokens: list[str]) -> float:
    """Simple keyword overlap score with semantic expansion (used as semantic proxy)."""
    if not keyword_tokens:
        return 0.0

    # Expand text tokens semantically
    expanded_tokens = expand_semantic(text_tokens)
    text_tokens_lower = {t.lower() for t in expanded_tokens}
    keyword_set = set(k.lower() for k in keyword_tokens)

    # Exact overlap
    overlap = len(text_tokens_lower & keyword_set)

    # Partial substring matches
    partial = 0
    for t in text_tokens_lower:
        for k in keyword_set:
            if t != k and (t in k or k in t):
                partial += 0.25

    total = overlap + min(partial, len(keyword_set) * 0.5)
    return min(total / max(len(keyword_set), 1), 1.5)


# Stopwords to filter from phrase matching
PHRASE_STOPWORDS = {
    'a', 'an', 'the', 'my', 'your', 'our', 'its', 'this', 'that', 'these', 'those',
    'i', 'you', 'we', 'he', 'she', 'it', 'they', 'me', 'us', 'him', 'her', 'them',
}

def phrase_match_score(query_tokens: set[str], trigger_conditions: list[str]) -> float:
    """Score based on phrase match with trigger conditions."""
    if not trigger_conditions:
        return 0.0

    # Filter out stopwords from query for phrase matching
    filtered_query = {t for t in query_tokens if len(t) > 1 and t not in PHRASE_STOPWORDS}
    if not filtered_query:
        return 0.0

    scores = []
    for phrase in trigger_conditions:
        phrase_tokens = set(phrase.lower().split())
        if not phrase_tokens:
            continue

        # Calculate overlap with filtered query
        overlap = len(filtered_query & phrase_tokens)
        score = overlap / len(phrase_tokens)
        scores.append(score)

    return max(scores) if scores else 0.0


def tag_overlap_score(query_tokens: set[str], tags: list[str]) -> float:
    """Score based on tag overlap with query tokens (case-insensitive)."""
    if not tags:
        return 0.0

    tag_set = {t.lower() for t in tags}
    expanded_query = expand_semantic(query_tokens)
    query_lower = {t.lower() for t in expanded_query}

    # Exact overlap
    overlap = len(query_lower & tag_set)

    # Partial substring matches (counted at 0.25 each, capped)
    partial = 0
    for t in query_lower:
        for tag in tag_set:
            if t != tag and (t in tag or tag in t):
                partial += 0.25

    total = overlap + min(partial, len(tag_set) * 0.5)  # Cap partial at 50% of tags
    return min(total / max(len(tag_set), 1), 1.5)  # Cap at 1.5 to allow for expansion bonus


def category_boost_score(category: str, query_tokens: set[str], category_config: dict) -> float:
    """Score based on category keyword matching."""
    cat_info = category_config.get(category, {})
    keywords = cat_info.get("keywords", [])

    if not keywords:
        return 0.0

    keyword_set = {k.lower() for k in keywords}
    query_lower = {t.lower() for t in query_tokens}

    overlap = len(query_lower & keyword_set)
    boost = cat_info.get("boost", 1.0)

    # Return boost * overlap ratio
    return (boost - 1.0) * (overlap / max(len(keyword_set), 1))


# Domain-specific query patterns for boosting relevant skills
DOMAIN_PATTERNS = {
    "email": {
        "keywords": {"email", "emails", "mail", "inbox", "gmail", "message", "messages"},
        "boost_tags": {"email", "gmail", "communication"},
        "boost_value": 0.15,
    },
    "pr": {
        "keywords": {"pr", "pull", "request", "merge", "pull-request", "pullrequest"},
        "boost_tags": {"code-review", "code_review", "git", "github"},
        "boost_value": 0.12,
    },
    "deploy": {
        "keywords": {"deploy", "deployment", "release", "ship", "launch", "push-to-main"},
        "boost_tags": {"deploy", "deployment", "ci", "cd", "infrastructure"},
        "boost_value": 0.12,
    },
    "test": {
        "keywords": {"test", "qa", "testing", "bugs", "verify"},
        "boost_tags": {"qa", "testing", "test", "bugs"},
        "boost_value": 0.12,
    },
}


def domain_specific_boost(query_tokens: set[str], skill: dict) -> float:
    """Apply domain-specific boosts for known query patterns."""
    query_lower = {t.lower() for t in query_tokens}
    skill_tags = {t.lower() for t in skill.get("tags", [])}

    total_boost = 0.0

    for domain, config in DOMAIN_PATTERNS.items():
        domain_keywords = config["keywords"]
        boost_tags = config["boost_tags"]
        boost_value = config["boost_value"]

        # Check if query matches this domain
        if query_lower & domain_keywords:
            # Check if skill has relevant tags
            if skill_tags & boost_tags:
                total_boost += boost_value

    return total_boost


def compute_intent_score(
    query_tokens: set[str],
    skill: dict,
    category_config: dict,
) -> dict:
    """Compute multi-signal intent score for a skill."""
    # Signal 1: Phrase match (40%)
    phrase_score = phrase_match_score(
        query_tokens,
        skill.get("trigger_conditions", [])
    )

    # Signal 2: Semantic (keyword overlap as proxy) (35%)
    semantic_score = simple_keyword_overlap(
        query_tokens,
        skill.get("keywords", [])
    )

    # Signal 3: Tag overlap (15%)
    tag_score = tag_overlap_score(
        query_tokens,
        skill.get("tags", [])
    )

    # Signal 4: Category boost (10%)
    cat_score = category_boost_score(
        skill.get("category", ""),
        query_tokens,
        category_config
    )

    # Signal 5: Domain-specific boost (bonus, not weighted)
    domain_boost = domain_specific_boost(query_tokens, skill)

    # Weighted total
    total = (
        WEIGHTS["phrase_match"] * phrase_score +
        WEIGHTS["semantic"] * semantic_score +
        WEIGHTS["tag_overlap"] * tag_score +
        WEIGHTS["category_boost"] * cat_score +
        domain_boost
    )

    return {
        "total": total,
        "phrase_match": phrase_score,
        "semantic": semantic_score,
        "tag_overlap": tag_score,
        "category_boost": cat_score,
        "domain_boost": domain_boost,
    }


def match_skills(
    query: str,
    skills: list[dict],
    category_config: dict,
    top_k: int = 5,
    min_score: float = 0.015,
    use_embeddings: bool = True,
) -> list[dict]:
    """
    Match skills to a user query using multi-signal ranking.

    When use_embeddings=True and Chroma is available, uses hybrid scoring
    (0.5 semantic + 0.3 phrase + 0.2 tag). Otherwise falls back to keyword-only.

    Returns top-k skills with scores above min_score threshold.
    """
    if not skills:
        return []

    # Try semantic search with embeddings
    semantic_scores = {}
    embed_service = None

    if use_embeddings:
        try:
            embed_service = _get_embed_service()
            if embed_service:
                results = embed_service.search_skills(query, top_k=len(skills))
                for r in results:
                    semantic_scores[r["name"]] = r["score"]
        except Exception as e:
            print(f"Warning: Embedding search failed: {e}", file=sys.stderr)
            semantic_scores = {}

    # Tokenize query
    query_tokens = set(query.lower().split())
    query_tokens = {t.strip('.,!?;:()[]{}"\'-') for t in query_tokens if t.strip()}

    # Check if we have semantic scores to use hybrid scoring
    has_semantic = bool(semantic_scores) and embed_service is not None

    scored = []
    for skill in skills:
        if has_semantic:
            scores = compute_hybrid_score(query_tokens, skill, semantic_scores)
        else:
            scores = compute_intent_score(query_tokens, skill, category_config)

        if scores["total"] >= min_score:
            scored.append({
                "skill": skill,
                "scores": scores,
            })

    # Sort by total score descending
    scored.sort(key=lambda x: x["scores"]["total"], reverse=True)

    return scored[:top_k]


def compute_hybrid_score(
    query_tokens: set[str],
    skill: dict,
    semantic_scores: dict,
) -> dict:
    """Compute hybrid score using embedding-based semantic + keyword signals."""
    name = skill.get("name", "")

    # Semantic score from Chroma (0.5 weight)
    semantic_score = semantic_scores.get(name, 0.0)

    # Phrase match score (0.3 weight)
    phrase_score = phrase_match_score(
        query_tokens,
        skill.get("trigger_conditions", [])
    )

    # Tag overlap score (0.2 weight)
    tag_score = tag_overlap_score(
        query_tokens,
        skill.get("tags", [])
    )

    # Weighted total
    total = (
        HYBRID_WEIGHTS["semantic"] * semantic_score +
        HYBRID_WEIGHTS["phrase_match"] * phrase_score +
        HYBRID_WEIGHTS["tag_overlap"] * tag_score
    )

    return {
        "total": total,
        "semantic": semantic_score,
        "phrase_match": phrase_score,
        "tag_overlap": tag_score,
        "category_boost": 0.0,
        "domain_boost": 0.0,
        "hybrid": True,
    }


# Words to strip from start of decomposed segments
SEGMENT_LEADERS = {
    'also', 'then', 'and', 'or', 'but', 'so', 'yet',
    'now', 'finally', 'lastly', 'next', 'first', 'second',
}

def decompose_request(query: str) -> list[str]:
    """
    Split multi-skill requests into individual skill clusters.

    Splits on "and", "also", ",", "+" separators.
    """
    # Normalize query
    query = query.strip()

    # Split on common separators
    # Be careful not to split on "and" within quotes
    segments = re.split(
        r'\s+(?:and|also|,|\+|then)\s+',
        query,
        flags=re.IGNORECASE
    )

    # Filter empty segments and clean up
    result = []
    for seg in segments:
        seg = seg.strip().lower()
        # Strip leading connective words
        words = seg.split()
        while words and words[0] in SEGMENT_LEADERS:
            words.pop(0)
        seg = ' '.join(words)
        if seg and len(seg) > 2:
            result.append(seg)

    # If no splits found, return original as single-element list
    if not result:
        result = [query]

    return result


def resolve_dependencies(
    skill_names: list[str],
    skills: list[dict],
    max_depth: int = 5,
) -> list[str]:
    """
    Resolve transitive closure of related_skills.

    Returns ordered list of skill names to load.
    """
    if not skills:
        return list(skill_names)

    # Build name -> skill lookup
    skill_map = {s["name"]: s for s in skills}

    # Track what needs to be loaded
    to_load = list(skill_names)
    loaded = []
    seen = set()

    depth = 0
    while to_load and depth < max_depth:
        next_to_load = []
        for skill_name in to_load:
            if skill_name in seen:
                continue
            seen.add(skill_name)
            loaded.append(skill_name)

            # Get related skills for this skill
            skill = skill_map.get(skill_name, {})
            related = skill.get("related_skills", [])

            for rel in related:
                if rel not in seen:
                    next_to_load.append(rel)

        to_load = next_to_load
        depth += 1

    return loaded


def match_and_rank(
    query: str,
    skills: list[dict],
    category_config: dict,
    top_k: int = 5,
) -> dict:
    """
    Main entry point: decompose query, match skills, resolve dependencies.

    Returns dict with:
      - segments: list of (segment_text, matched_skills)
      - all_skills: list of all matched skills (with deps resolved)
      - final_skills: list of top skills per segment
    """
    # Decompose query into segments
    segments = decompose_request(query)

    segment_results = []
    all_matched_names = set()

    for segment in segments:
        matched = match_skills(segment, skills, category_config, top_k=top_k)
        segment_results.append({
            "query": segment,
            "matches": matched,
        })
        for m in matched:
            all_matched_names.add(m["skill"]["name"])

    # Resolve dependencies
    resolved = resolve_dependencies(list(all_matched_names), skills)

    return {
        "original_query": query,
        "segments": segment_results,
        "resolved_dependencies": resolved,
        "all_matched": list(all_matched_names),
    }
