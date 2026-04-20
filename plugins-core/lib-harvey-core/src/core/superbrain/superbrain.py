#!/usr/bin/env python3
"""
Superbrain — Unified knowledge layer for Harvey OS.

⚠️ Rust alternatives available:
  makakoo search "topic"    — FTS5 search (same DB, faster)
  makakoo query "question"  — search + LLM synthesis

This Python CLI remains the daily driver for: sync, remember, stack, status,
context, gods, neighbors (not yet ported to Rust).

Single entry point for all knowledge operations:
  query    — Search everything (FTS5 + vectors + graph), synthesize answer
  sync     — Index Brain into SQLite FTS5 + entity graph
  status   — Show what's available and current state
  remember — Log an event to the knowledge store + Brain journal
  stack    — Show current memory stack (L0+L1+L2)

Architecture:
  Primary:  SQLite FTS5 (instant keyword search, zero dependencies)
  Optional: Embeddings via switchAILocal/Gemini + cosine similarity
  Always:   Entity graph from [[wikilinks]], memory stack for context

Usage:
    # Python
    from core.superbrain.superbrain import Superbrain
    sb = Superbrain()
    result = sb.query("What do I know about Polymarket?")
    print(result.answer)

    # CLI
    python3 superbrain.py query "your question"
    python3 superbrain.py sync [--force]
    python3 superbrain.py status
"""

import json
import logging
import os
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

from core.superbrain.store import SuperbrainStore
from core.superbrain.memory_stack import MemoryStack

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("superbrain")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


@dataclass
class SearchHit:
    """A single search result."""
    score: float
    text: str
    title: str
    source: str  # "fts5", "vector", "graph", "event"
    metadata: dict = field(default_factory=dict)


@dataclass
class QueryResult:
    """Result from a Superbrain query."""
    answer: str
    sources: List[SearchHit]
    systems_queried: List[str]
    query_time_sec: float
    token_cost: int = 0


class Superbrain:
    """
    Unified knowledge layer. Always works (FTS5 is zero-dependency).
    Embeddings are optional and additive.
    """

    def __init__(self, brain_dir: str = None):
        self.brain_dir = brain_dir or os.path.join(HARVEY_HOME, "data", "Brain")
        self.store = SuperbrainStore(brain_dir=self.brain_dir)
        self.memory = MemoryStack(brain_dir=self.brain_dir)
        self._embedding_available = None
        self._llm_available = None

    # ═══════════════════════════════════════════════════════════════
    #  QUERY — Search everything, synthesize answer
    # ═══════════════════════════════════════════════════════════════

    def query(self, question: str, top_k: int = 10, synthesize: bool = True,
              refine_steps: int = 2) -> QueryResult:
        """
        Query all knowledge systems with iterative refinement.

        Pipeline: FTS5 → Vector → Graph boost → Merge (normalized) → Refine → Synthesize

        When BRAIN_USE_ANCHORS=1 AND the anchor index covers ≥50% of
        brain_docs, delegates to query_anchored() — the Phase D read path
        that searches compressed anchors first and expands full passages
        only on demand (diffusion-style coarse→fine).

        See harvey-os/skills/meta/brain-anchors/SKILL.md.
        """
        if os.environ.get("BRAIN_USE_ANCHORS", "1") == "1":
            try:
                anchored, total = self.store.anchors_count()
            except Exception:
                anchored, total = 0, 0
            if total > 0 and anchored * 2 >= total:
                log.info("  Anchor read path active (%d/%d anchored)", anchored, total)
                return self.query_anchored(
                    question,
                    top_k=top_k,
                    synthesize=synthesize,
                    refine_steps=refine_steps,
                )
            elif anchored > 0:
                log.info(
                    "  Anchor read path skipped — coverage too low (%d/%d); falling back to full-content path",
                    anchored, total,
                )

        start = time.time()
        systems = []
        fts_hits: List[SearchHit] = []
        vec_hits: List[SearchHit] = []

        # 0. Direct date injection for temporal queries
        # "what happened today" → guarantee today's journal is in results
        q_lower = question.lower()
        if "today" in q_lower or "built today" in q_lower:
            today_name = time.strftime("%Y_%m_%d")
            row = self.store._conn.execute(
                "SELECT name, doc_type, content, entities, path FROM brain_docs WHERE name = ?",
                (today_name,)
            ).fetchone()
            if row:
                fts_hits.append(SearchHit(
                    score=100.0,  # very high — will survive normalization
                    text=row["content"][:800],
                    title=row["name"],
                    source="fts5:journal",
                    metadata={"entities": json.loads(row["entities"]) if row["entities"] else [],
                              "path": row["path"]},
                ))

        # 1. FTS5 keyword search (always works, instant)
        fts_results = self.store.search(question, top_k=top_k)
        systems.append("fts5")
        for r in fts_results:
            fts_hits.append(SearchHit(
                score=r["score"],
                text=r["content"][:800],
                title=r["name"],
                source=f"fts5:{r['doc_type']}",
                metadata={"entities": r["entities"], "path": r["path"]},
            ))
        log.info("  FTS5: %d results", len(fts_results))

        # 2. Vector search (semantic — finds meaning, not keywords)
        if self._check_embedding():
            try:
                query_vec = self._embed(question)
                if query_vec:
                    vec_results = self._qdrant_search(query_vec, top_k=top_k)
                    if not vec_results:
                        vec_results = self.store.vector_search(query_vec, top_k=top_k)
                    if vec_results:
                        systems.append("vector")
                        for r in vec_results:
                            vec_hits.append(SearchHit(
                                score=r["score"],
                                text=r["content"][:800],
                                title=r["name"],
                                source=f"vector:{r.get('doc_type', 'qdrant')}",
                                metadata={"entities": r.get("entities", [])},
                            ))
                    log.info("  Vector: %d results", len(vec_results) if vec_results else 0)
            except Exception as e:
                log.warning("  Vector search failed: %s", e)

        # 3. Graph context (from graph.py via store, not raw SQL)
        graph_ctx = ""
        graph = self._ensure_graph()
        if graph:
            try:
                from core.superbrain.graph import graph_context_for_query
                graph_ctx = graph_context_for_query(graph, question)
                if graph_ctx:
                    systems.append("graph")
            except Exception:
                graph_ctx = self.store.graph_context(question)  # fallback
                if graph_ctx:
                    systems.append("graph")
        else:
            graph_ctx = self.store.graph_context(question)
            if graph_ctx:
                systems.append("graph")

        # 4. Normalize + merge + graph boost
        merged = self._merge(fts_hits, vec_hits, question)
        log.info("  Merged: %d results", len(merged))

        # 5. Iterative refinement (diffusion-inspired: each pass sharpens results)
        for step in range(refine_steps):
            if not merged:
                break
            # Enrich query with top result titles
            top_context = " ".join(s.title.replace("_", " ") for s in merged[:3])
            # Also extract entities from top results for richer context
            top_entities = []
            for s in merged[:3]:
                top_entities.extend(s.metadata.get("entities", []))
            entity_context = " ".join(dict.fromkeys(top_entities))  # dedupe, preserve order
            enriched = f"{question} {top_context} {entity_context}".strip()

            # Re-run vector search with enriched query
            if self._check_embedding():
                try:
                    refined_vec = self._embed(enriched)
                    if refined_vec:
                        refined_results = self.store.vector_search(refined_vec, top_k=top_k)
                        if refined_results:
                            refined_hits = [SearchHit(
                                score=r["score"],
                                text=r["content"][:800],
                                title=r["name"],
                                source=f"refine:{r.get('doc_type', 'sqlite')}",
                                metadata={"entities": r.get("entities", [])},
                            ) for r in refined_results]
                            # Merge refined into existing (keeps best per title)
                            merged = self._merge(merged, refined_hits, question)
                            log.info("  Refine step %d: %d results", step + 1, len(merged))
                except Exception:
                    pass

        # 6. Apply temporal/recency boost ONCE (after all merging/refinement)
        if merged:
            merged = self._temporal_boost(merged, question)

        # 7. Synthesize answer
        if synthesize and merged:
            memory_ctx = self.memory.for_query(question)
            answer = self._synthesize(question, merged, graph_ctx, memory_ctx)
        elif merged:
            answer = self._format_raw(merged[:10])
        else:
            answer = "No relevant results found."

        elapsed = time.time() - start
        return QueryResult(
            answer=answer,
            sources=merged,
            systems_queried=systems,
            query_time_sec=elapsed,
        )

    # ═══════════════════════════════════════════════════════════════
    #  QUERY_ANCHORED — Phase D read path: anchors first, expand on demand
    # ═══════════════════════════════════════════════════════════════

    def query_anchored(self, question: str, top_k: int = 10, synthesize: bool = True,
                       refine_steps: int = 2, expand_top_n: int = 2) -> QueryResult:
        """
        Anchor-first read path (Phase D of brain-anchors).

        Contract:
        1. FTS5 search over brain_anchors_fts — returns compressed anchor text,
           not full passages. 5-10x smaller payload than query().
        2. Vector search over the existing brain_vectors (full-content
           embeddings) — kept as an entity/context signal. Phase D2 will add
           dedicated anchor embeddings; for now we reuse the existing index
           and just project hits onto anchors at merge time.
        3. Graph boost — identical to query().
        4. Auto-expand top N hits via store.get_doc_by_id(). The synthesizer
           sees anchors for all K hits but full content for only the top N.
        5. Synthesize on a much smaller context: (K - N) × anchor_chars +
           N × content_chars vs. K × content_chars in the classic path.

        Defaults to expand_top_n=2 which gives ~68% token reduction at K=10.

        Env overrides:
            BRAIN_USE_ANCHORS=0      — disable the anchor path entirely
            BRAIN_EXPAND_TOP_N=<int> — override auto-expand count
        """
        start = time.time()
        systems = ["anchors"]
        anchor_hits: List[SearchHit] = []
        vec_hits: List[SearchHit] = []

        # 0. Today's-journal injection (same as query(), preserved for parity)
        q_lower = question.lower()
        if "today" in q_lower or "built today" in q_lower:
            today_name = time.strftime("%Y_%m_%d")
            row = self.store._conn.execute(
                "SELECT id, name, doc_type, content, entities, path, anchor "
                "FROM brain_docs WHERE name = ?",
                (today_name,),
            ).fetchone()
            if row:
                text = row["anchor"] or row["content"][:400]
                anchor_hits.append(SearchHit(
                    score=100.0,
                    text=text,
                    title=row["name"],
                    source="anchors:journal:today",
                    metadata={
                        "doc_id": row["id"],
                        "entities": json.loads(row["entities"]) if row["entities"] else [],
                        "path": row["path"],
                        "has_anchor": row["anchor"] is not None,
                    },
                ))

        # 1. FTS5 over anchors — primary signal, cheap, fast
        anchor_results = self.store.search_anchors(question, top_k=top_k)
        for r in anchor_results:
            anchor_hits.append(SearchHit(
                score=r["score"],
                text=r["anchor"],
                title=r["name"],
                source=f"anchors:{r['doc_type']}",
                metadata={
                    "doc_id": r["id"],
                    "entities": r["entities"],
                    "anchor_entities": r.get("anchor_entities", []),
                    "anchor_keywords": r.get("anchor_keywords", []),
                    "path": r["path"],
                    "has_anchor": True,
                },
            ))
        log.info("  Anchors FTS5: %d results", len(anchor_results))

        # 2. Vector search — prefer dedicated anchor embeddings (Phase D2)
        # when coverage is decent, otherwise fall back to the classic
        # full-content vector index with anchor substitution.
        if self._check_embedding():
            try:
                query_vec = self._embed(question)
                if query_vec:
                    vec_results = []
                    source_tag = "vector"

                    # Prefer vector_search_anchors if ≥50% of anchored docs
                    # have dedicated anchor embeddings.
                    try:
                        embedded, anchored = self.store.anchor_vectors_count()
                    except Exception:
                        embedded, anchored = 0, 0
                    use_anchor_vectors = (
                        anchored > 0 and embedded * 2 >= anchored
                    )

                    if use_anchor_vectors:
                        vec_results = self.store.vector_search_anchors(
                            query_vec, top_k=top_k
                        )
                        source_tag = "anchor_vec"
                        log.info("  Anchor vectors: %d results (dedicated D2 index)",
                                 len(vec_results))
                    else:
                        # Transition path — use classic vector search and
                        # substitute anchor text post-hoc.
                        vec_results = self._qdrant_search(query_vec, top_k=top_k)
                        if not vec_results:
                            vec_results = self.store.vector_search(query_vec, top_k=top_k)
                        log.info("  Vector (classic): %d results", len(vec_results) if vec_results else 0)

                    if vec_results:
                        systems.append(source_tag)
                        for r in vec_results:
                            doc_id = r.get("id") or r.get("doc_id")
                            anchor_text = r.get("anchor")
                            # Classic path: fetch anchor if not already carried.
                            if anchor_text is None and doc_id is not None and not use_anchor_vectors:
                                try:
                                    fetched = self.store.get_doc_by_id(int(doc_id))
                                    if fetched:
                                        anchor_text = fetched.get("anchor")
                                except Exception:
                                    pass
                            text = anchor_text or (r.get("content", "") or "")[:400]
                            vec_hits.append(SearchHit(
                                score=r["score"],
                                text=text,
                                title=r["name"],
                                source=f"{source_tag}:{r.get('doc_type', 'sqlite')}",
                                metadata={
                                    "doc_id": doc_id,
                                    "entities": r.get("entities", []),
                                    "anchor_entities": r.get("anchor_entities", []),
                                    "has_anchor": anchor_text is not None,
                                },
                            ))
            except Exception as e:
                log.warning("  Vector search (anchor path) failed: %s", e)

        # 3. Graph context (via graph.py, same as query())
        graph_ctx = ""
        graph = self._ensure_graph()
        if graph:
            try:
                from core.superbrain.graph import graph_context_for_query
                graph_ctx = graph_context_for_query(graph, question)
            except Exception:
                graph_ctx = self.store.graph_context(question)
        else:
            graph_ctx = self.store.graph_context(question)
        if graph_ctx:
            systems.append("graph")

        # 4. Merge & dedupe
        merged = self._merge(anchor_hits, vec_hits, question)
        log.info("  Merged: %d results (anchor-first)", len(merged))

        # 5. Refinement — cheap because the context-enrichment query is
        # built from anchor text, not 800-char content slices.
        for step in range(refine_steps):
            if not merged:
                break
            top_context = " ".join(s.title.replace("_", " ") for s in merged[:3])
            top_entities = []
            for s in merged[:3]:
                top_entities.extend(s.metadata.get("entities", []))
                top_entities.extend(s.metadata.get("anchor_entities", []))
            entity_context = " ".join(dict.fromkeys(top_entities))
            enriched = f"{question} {top_context} {entity_context}".strip()

            refined_anchor_results = self.store.search_anchors(enriched, top_k=top_k)
            if refined_anchor_results:
                refined_hits = [SearchHit(
                    score=r["score"],
                    text=r["anchor"],
                    title=r["name"],
                    source=f"refine:anchors:{r['doc_type']}",
                    metadata={
                        "doc_id": r["id"],
                        "entities": r["entities"],
                        "anchor_entities": r.get("anchor_entities", []),
                        "has_anchor": True,
                    },
                ) for r in refined_anchor_results]
                merged = self._merge(merged, refined_hits, question)
                log.info("  Refine step %d (anchors): %d results", step + 1, len(merged))

        # 6. Temporal boost
        if merged:
            merged = self._temporal_boost(merged, question)

        # 7. Auto-expand top N hits — fetch full passages only for the
        # highest-confidence matches. Default: N=2.
        try:
            expand_n = int(os.environ.get("BRAIN_EXPAND_TOP_N", str(expand_top_n)))
        except ValueError:
            expand_n = expand_top_n
        expand_n = max(0, min(expand_n, len(merged)))
        for s in merged[:expand_n]:
            doc_id = s.metadata.get("doc_id")
            if not doc_id:
                continue
            try:
                full = self.store.get_doc_by_id(int(doc_id))
                if full and full.get("content"):
                    # Replace anchor text with full content (truncated to
                    # 800 chars to match query() semantics).
                    s.text = full["content"][:800]
                    s.metadata["expanded"] = True
            except Exception as e:
                log.debug("expand_anchor failed for doc_id=%s: %s", doc_id, e)

        # 8. Synthesize — same path as query(), but on far smaller context
        if synthesize and merged:
            memory_ctx = self.memory.for_query(question)
            answer = self._synthesize(question, merged, graph_ctx, memory_ctx)
        elif merged:
            answer = self._format_raw(merged[:10])
        else:
            answer = "No relevant results found."

        elapsed = time.time() - start
        return QueryResult(
            answer=answer,
            sources=merged,
            systems_queried=systems,
            query_time_sec=elapsed,
        )

    def expand_anchor(self, doc_id: int) -> Optional[dict]:
        """
        Public expansion helper for callers that want to fetch a full
        passage by doc_id after seeing only its anchor. Mirrors the
        Letta-style function-gated expansion pattern: the LLM (or the
        agent loop) decides which anchor merits a full read.
        """
        return self.store.get_doc_by_id(doc_id)

    def _merge(self, group_a: List[SearchHit], group_b: List[SearchHit],
               question: str = "") -> List[SearchHit]:
        """Normalize scores, merge, graph-boost, deduplicate.

        FTS5 BM25 scores (0-30) and cosine similarity (0-1) are on different scales.
        Normalize both to 0-1, weight by source trust, boost by graph adjacency.
        """
        # ── Normalize scores to comparable scale ──
        # Only normalize if scores are clearly on BM25 scale (> 1.5)
        # Already-merged results are in 0-1 range and must NOT be re-normalized
        if group_a:
            max_s = max(h.score for h in group_a)
            if max_s > 1.5:
                # BM25 scale (0-30) — normalize to 0-1 and apply FTS weight
                min_s = min(h.score for h in group_a)
                spread = (max_s - min_s) if max_s != min_s else 1.0
                for h in group_a:
                    h.score = ((h.score - min_s) / spread) * 0.45  # normalize + weight
            # else: already normalized from previous merge pass, keep as-is

        # Vector/refined scores already 0-1 (cosine) — apply weights
        VEC_WEIGHT = 0.55
        REFINE_WEIGHT = 0.5
        for h in group_b:
            h.score = max(0.0, min(1.0, h.score))
            if "refine" in h.source:
                h.score *= REFINE_WEIGHT
            else:
                h.score *= VEC_WEIGHT

        # ── Combine all hits ──
        all_hits = group_a + group_b

        # ── Graph-conditioned boosting ──
        if question:
            all_hits = self._graph_boost(all_hits, question)

        # ── Deduplicate: keep best score per title ──
        by_title: Dict[str, List[SearchHit]] = {}
        for h in all_hits:
            by_title.setdefault(h.title, []).append(h)

        result = []
        for title, group in by_title.items():
            best = max(group, key=lambda x: x.score)
            # Multi-system bonus: found by BOTH FTS5 and vector = high confidence
            sources = set(h.source.split(":")[0] for h in group)
            if len(sources) > 1:
                best.score *= 1.5  # 50% boost for cross-system agreement
            result.append(best)

        result.sort(key=lambda x: x.score, reverse=True)
        return result[:30]  # keep more during merge; final cut happens after temporal boost

    def _ensure_graph(self):
        """Lazy-load knowledge graph from store. Refreshes if entity count changed."""
        current_count = 0
        try:
            current_count = self.store.entity_graph_count()
        except Exception:
            pass

        cached = getattr(self, '_graph_cache', None)
        cached_count = getattr(self, '_graph_count', 0)

        if cached is None or current_count != cached_count:
            try:
                from core.superbrain import graph as graph_mod
                self._graph_cache = graph_mod.load_from_store(self.store)
                self._graph_count = current_count
            except Exception as e:
                log.warning("Failed to load graph: %s", e)
                self._graph_cache = None
        return self._graph_cache

    def _graph_boost(self, hits: List[SearchHit], question: str) -> List[SearchHit]:
        """Boost results using Personalized PageRank seeded on query entities.

        Replaces raw SQL entity matching with graph-algorithm-based ranking.
        Falls back gracefully if graph is unavailable.
        """
        graph = self._ensure_graph()
        if not graph:
            return hits

        try:
            from core.superbrain.graph import ppr_boost_hits
            boost_weight = float(os.environ.get("HARVEY_GRAPH_BOOST_WEIGHT", "0.3"))
            return ppr_boost_hits(graph, hits, question, weight=boost_weight)
        except Exception as e:
            log.warning("PPR graph boost failed: %s", e)
            return hits

    def _temporal_boost(self, hits: List[SearchHit], question: str) -> List[SearchHit]:
        """Apply temporal/recency boost to journal results.

        Called ONCE after all merging is complete (not inside _merge which
        is called multiple times during refinement).
        """
        from datetime import datetime

        temporal_words = {"today", "yesterday", "recent", "recently", "latest",
                          "last", "week", "month", "built", "happened", "did"}
        q_words = set(question.lower().split()) if question else set()
        is_temporal = bool(q_words & temporal_words)

        now = datetime.now()
        for h in hits:
            is_journal = "journal" in h.title or "journal" in (h.source or "")
            if not is_journal:
                continue

            try:
                date_str = h.title.replace("_", "-")[:10]
                d = datetime.strptime(date_str, "%Y-%m-%d")
                days_ago = (now - d).days
            except (ValueError, IndexError):
                days_ago = 999

            if is_temporal:
                if days_ago == 0:
                    h.score *= 3.0
                elif days_ago == 1:
                    h.score *= 2.0
                elif days_ago <= 7:
                    h.score *= 1.5
                else:
                    h.score *= 1.1
            else:
                if days_ago <= 7:
                    h.score *= 1.2
                elif days_ago <= 30:
                    h.score *= 1.1

        hits.sort(key=lambda x: x.score, reverse=True)
        return hits

    def _format_raw(self, hits: List[SearchHit]) -> str:
        """Format raw results without LLM synthesis."""
        return "\n".join(
            f"- [{h.source}: {h.title}] (score={h.score:.2f}) {h.text[:200]}"
            for h in hits
        )

    def _synthesize(self, question: str, results: List[SearchHit],
                    graph_ctx: str, memory_ctx: str) -> str:
        """LLM synthesis via switchAILocal. Falls back to raw results."""
        import requests
        from core.superbrain import config

        context = "\n---\n".join(
            f"[{r.source}: {r.title}] {r.text[:300]}"
            for r in results[:6]
        )

        prompt = f"""{memory_ctx}

{graph_ctx}

Answer concisely based on these sources. Cite with [Source: title].
Question: {question}

Sources:
{context}

Answer:"""

        try:
            resp = requests.post(
                f"{config.LLM_BASE_URL}/chat/completions",
                headers={"Authorization": f"Bearer {config.LLM_API_KEY}"},
                json={
                    "model": config.LLM_SYNTHESIS_MODEL,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": 0.2,
                    "max_tokens": 500,
                },
                timeout=45,
            )
            if resp.status_code == 200:
                msg = resp.json()["choices"][0]["message"]
                answer = msg.get("content", "") or msg.get("reasoning_content", "")
                if answer:
                    return answer
        except Exception as e:
            log.warning("LLM synthesis failed: %s", e)

        return self._format_raw(results[:5])

    # ═══════════════════════════════════════════════════════════════
    #  QDRANT VECTOR SEARCH — multimodal collection (Gemini 3072d)
    #  NOTE: Brain text search uses SQLite vectors (switchAILocal 1024d).
    #        Qdrant "brain" collection has legacy 3072d Gemini vectors —
    #        dimension mismatch with current embeddings. Only query Qdrant
    #        for the "multimodal" collection (OCR/image/video).
    # ═══════════════════════════════════════════════════════════════

    def _qdrant_search(self, query_vec: List[float], top_k: int = 10) -> Optional[List[dict]]:
        """Search Qdrant multimodal collection if available and dims match."""
        import requests
        from core.superbrain import config

        qdrant_url = f"http://{config.QDRANT_HOST}:{config.QDRANT_PORT}"

        # Only search Qdrant collections whose dim matches our query vector
        for collection in ["brain", "multimodal"]:
            try:
                # Check collection exists and get its dim
                info_resp = requests.get(
                    f"{qdrant_url}/collections/{collection}", timeout=2)
                if info_resp.status_code != 200:
                    continue
                col_dim = info_resp.json()["result"]["config"]["params"]["vectors"]["size"]
                if col_dim != len(query_vec):
                    continue  # dimension mismatch, skip

                resp = requests.post(
                    f"{qdrant_url}/collections/{collection}/points/search",
                    json={
                        "vector": query_vec,
                        "limit": top_k,
                        "with_payload": True,
                        "score_threshold": 0.3,
                    },
                    timeout=5,
                )
                if resp.status_code != 200:
                    continue

                results = []
                for point in resp.json().get("result", []):
                    payload = point.get("payload", {})
                    results.append({
                        "name": payload.get("name", payload.get("page_name",
                                payload.get("title", "unknown"))),
                        "doc_type": payload.get("doc_type", payload.get("type", "qdrant")),
                        "content": payload.get("content", payload.get("text_content", "")),
                        "score": point.get("score", 0),
                        "entities": payload.get("entities", []),
                        "path": payload.get("path", ""),
                    })
                if results:
                    return results
            except Exception as e:
                log.debug("Qdrant search on %s failed: %s", collection, e)
                continue

        return None

    # ═══════════════════════════════════════════════════════════════
    #  EMBEDDING
    # ═══════════════════════════════════════════════════════════════

    _EMBED_PROBE_TTL = 60.0  # seconds

    def _check_embedding(self) -> bool:
        """Probe the embedding endpoint with a real 1-token _embed call.

        Historical bug: this used to ping /v1/models, which returns 200
        even when the actual embedding backend is OOM or the model isn't
        pulled — giving false confidence and causing every sync call to
        fail individually instead of short-circuiting. Now we issue a real
        embed against a single token and cache the result for 60s to
        avoid hammering Ollama while a long sync runs.
        """
        now = time.monotonic()
        cached = getattr(self, "_embedding_probe_cache", None)
        if cached is not None:
            value, when = cached
            if now - when < self._EMBED_PROBE_TTL:
                return value

        vec = self._embed("ok")
        available = bool(vec) and len(vec) > 0
        self._embedding_probe_cache = (available, now)
        self._embedding_available = available
        return available

    def _embed(self, text: str) -> Optional[List[float]]:
        """Embed text via switchAILocal.

        Timeout is 120s (not 30s) because under load on CPU-only Ollama
        a single embed of 8K chars on a 32K-context model can take >30s.
        A too-aggressive timeout causes the caller to hammer a clogged
        queue faster than it can drain, turning a slow backend into a
        hard failure.
        """
        import requests
        from core.superbrain import config

        # Truncation: 2000 chars ≈ 500 tokens. Most embedding models are
        # trained with a 512-token max — 8000-char inputs on a 32K-context
        # model run ~30× slower on CPU with no retrieval-quality gain.
        max_chars = int(os.environ.get("SUPERBRAIN_EMBED_MAX_CHARS", "2000"))
        text = text[:max_chars]
        model = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:0.6b")
        base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1")
        api_key = config.LLM_API_KEY or "sk-test-123"
        timeout = float(os.environ.get("SUPERBRAIN_EMBED_TIMEOUT", "120"))

        try:
            resp = requests.post(
                f"{base_url}/embeddings",
                headers={"Authorization": f"Bearer {api_key}"},
                json={"model": model, "input": text},
                timeout=timeout,
            )
            if resp.status_code == 200:
                return resp.json()["data"][0]["embedding"]
            log.warning("Embedding failed (%d): %s", resp.status_code, resp.text[:200])
        except Exception as e:
            log.warning("Embedding error: %s", e)

        return None

    # ═══════════════════════════════════════════════════════════════
    #  SYNC — Index Brain into store
    # ═══════════════════════════════════════════════════════════════

    def sync(self, force: bool = False, embed: Optional[bool] = None) -> dict:
        """
        Sync Brain to SQLite FTS5 + entity graph.

        Args:
            force: Re-index everything (ignores content hashes)
            embed: Also compute embeddings. Default None → auto-detect
                   (embeds if the backend is healthy, skips otherwise).
                   Pass embed=False to opt out explicitly.
        """
        if embed is None:
            embed = self._check_embedding()

        result = self.store.sync_brain(force=force)
        self.store.rebuild_entity_graph()

        if embed and self._check_embedding():
            result["vectors"] = self._embed_all()

        return result

    def _embed_all(self) -> dict:
        """Embed all Brain docs that don't have vectors yet."""
        conn = self.store._conn
        embed_model = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:0.6b")

        rows = conn.execute("""
            SELECT d.id, d.content, d.name
            FROM brain_docs d
            LEFT JOIN brain_vectors v ON d.id = v.doc_id
            WHERE v.doc_id IS NULL
        """).fetchall()

        total = len(rows)
        stats = {"embedded": 0, "errors": 0, "total": total, "model": embed_model}
        failed_rows = []

        log.info("Embedding %d documents with model=%s ...", total, embed_model)
        for i, row in enumerate(rows):
            vec = self._embed(row["content"][:2000])
            if vec:
                self.store.store_vector(row["id"], vec, model=embed_model)
                stats["embedded"] += 1
            else:
                stats["errors"] += 1
                failed_rows.append(row)
            if (i + 1) % 20 == 0:
                log.info("  Progress: %d/%d embedded (%d errors)", stats["embedded"], total, stats["errors"])
            time.sleep(0.02)  # minimal delay — switchAILocal is local

        # Retry failed docs once
        if failed_rows:
            log.info("Retrying %d failed docs...", len(failed_rows))
            for row in failed_rows:
                vec = self._embed(row["content"][:2000])
                if vec:
                    self.store.store_vector(row["id"], vec, model=embed_model)
                    stats["embedded"] += 1
                    stats["errors"] -= 1
                time.sleep(0.02)

        log.info("Embedding complete: %d/%d embedded, %d errors", stats["embedded"], total, stats["errors"])
        return stats

    # ═══════════════════════════════════════════════════════════════
    #  SYNC SINGLE FILE
    # ═══════════════════════════════════════════════════════════════

    def sync_file(self, file_path: str, embed: bool = True) -> bool:
        """Sync a single Brain file to FTS5 + optionally embed.

        Call this after writing to a Brain page or journal.
        Returns True if synced successfully.
        """
        fp = Path(file_path)
        if not fp.exists():
            log.warning("sync_file: %s does not exist", file_path)
            return False

        # Determine doc_type from path
        path_str = str(fp)
        if "/journals/" in path_str:
            doc_type = "journal"
        elif "/pages/" in path_str:
            doc_type = "page"
        else:
            log.warning("sync_file: %s is not in pages/ or journals/", file_path)
            return False

        try:
            result = self.store._sync_file(fp, doc_type, {}, force=True)
            self.store._conn.commit()
            if result in ("pages", "journals"):
                log.info("sync_file: indexed %s", fp.name)
            else:
                log.warning("sync_file: %s returned %s", fp.name, result)
                return False
        except Exception as e:
            log.error("sync_file FTS5 failed for %s: %s", fp.name, e)
            return False

        # Entity graph must be rebuilt after FTS5 index lands — otherwise the
        # new wikilinks in this file don't show up in entity traversal.
        # (opencode caught this miss during the superbrain review 2026-04-11.)
        try:
            self.store.rebuild_entity_graph()
        except Exception as e:
            log.warning("sync_file: entity graph rebuild failed for %s: %s", fp.name, e)

        # Optionally embed
        if embed and self._check_embedding():
            try:
                embed_model = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:0.6b")
                row = self.store._conn.execute(
                    "SELECT id, content FROM brain_docs WHERE path = ?", (path_str,)
                ).fetchone()
                if row:
                    vec = self._embed(row["content"][:2000])
                    if vec:
                        self.store.store_vector(row["id"], vec, model=embed_model)
                        log.info("sync_file: embedded %s", fp.name)
            except Exception as e:
                log.warning("sync_file embed failed for %s: %s", fp.name, e)

        return True

    # ═══════════════════════════════════════════════════════════════
    #  REMEMBER — Log events
    # ═══════════════════════════════════════════════════════════════

    def remember(self, event_type: str, agent: str, summary: str,
                 details: dict = None, write_journal: bool = True) -> bool:
        """Log an event to store + optionally Brain journal."""
        self.store.log_event(event_type, agent, summary, details)

        journal_path = None
        if write_journal:
            from datetime import datetime
            today = datetime.now().strftime("%Y_%m_%d")
            journal_path = Path(self.brain_dir) / "journals" / f"{today}.md"
            entry = f"- [{event_type}] [[{agent}]]: {summary}\n"
            try:
                with open(journal_path, "a") as f:
                    f.write(entry)
            except Exception as e:
                log.error("Journal write failed: %s", e)
                journal_path = None

        # Immediately sync the journal so new entries are searchable
        if journal_path:
            self.sync_file(str(journal_path), embed=False)

        return True

    # ═══════════════════════════════════════════════════════════════
    #  BOOTSTRAP — Self-knowledge for fresh installs
    # ═══════════════════════════════════════════════════════════════

    def bootstrap(self) -> dict:
        """Generate Brain pages from SKILL.md files + core modules.

        On a fresh install, Harvey's Brain is empty. This scans all skills
        and core modules, creates Brain pages for each, and syncs everything
        to FTS5 + vectors. After bootstrap, Harvey knows what Harvey can do.
        """
        from pathlib import Path
        import re

        harvey_os = Path(HARVEY_HOME) / "harvey-os"
        brain_pages = Path(self.brain_dir) / "pages"
        brain_pages.mkdir(parents=True, exist_ok=True)

        stats = {"skills_indexed": 0, "pages_created": 0, "skipped": 0}

        # 1. Scan all SKILL.md files
        for skill_md in harvey_os.rglob("SKILL.md"):
            try:
                content = skill_md.read_text(encoding="utf-8")
                # Extract skill name from frontmatter or directory
                skill_dir = skill_md.parent.name
                name_match = re.search(r"^name:\s*(.+)$", content, re.MULTILINE)
                skill_name = name_match.group(1).strip() if name_match else skill_dir

                # Extract description
                desc_match = re.search(r"^description:\s*(.+)$", content, re.MULTILINE)
                desc = desc_match.group(1).strip() if desc_match else ""

                # Create/update Brain page
                page_path = brain_pages / f"{skill_name}.md"
                if page_path.exists():
                    existing = page_path.read_text(encoding="utf-8")
                    if "auto-generated by bootstrap" in existing:
                        pass  # overwrite auto-generated pages
                    else:
                        stats["skipped"] += 1
                        continue  # don't overwrite manually created pages

                # Write Brain page in Brain outliner format
                category = skill_md.parent.parent.name if skill_md.parent.parent != harvey_os else ""
                page_content = (
                    f"- tags:: #skill #harvey-os {f'#{category}' if category else ''}\n"
                    f"- auto-generated by bootstrap\n"
                    f"- skill-path:: `{skill_md.relative_to(harvey_os)}`\n"
                    f"- description:: {desc}\n"
                    f"\n"
                    f"- ## Overview\n"
                    f"  - {desc}\n"
                    f"  - Source: `harvey-os/{skill_md.relative_to(harvey_os)}`\n"
                )

                # Extract key sections from SKILL.md (trigger phrases, usage, etc.)
                # Keep it compact — just enough for search to find it
                for section in ["When to Use", "Usage", "Quick", "Setup", "Commands"]:
                    match = re.search(
                        rf"##\s*.*{section}.*?\n((?:.*\n)*?)(?=\n##|\Z)",
                        content, re.IGNORECASE
                    )
                    if match:
                        section_text = match.group(1).strip()[:500]
                        page_content += f"\n- ## {section}\n"
                        for line in section_text.split("\n")[:10]:
                            line = line.strip()
                            if line:
                                page_content += f"  - {line}\n"

                page_path.write_text(page_content, encoding="utf-8")
                stats["pages_created"] += 1
                stats["skills_indexed"] += 1
                log.info("  Bootstrapped: %s", skill_name)

            except Exception as e:
                log.warning("  Skip %s: %s", skill_md, e)

        # 2. Scan core modules (core/*/ with Python files)
        core_dir = harvey_os / "core"
        if core_dir.exists():
            for module_dir in sorted(core_dir.iterdir()):
                if not module_dir.is_dir():
                    continue
                # Skip __pycache__ and hidden dirs
                if module_dir.name.startswith(("_", ".")):
                    continue
                # Must have Python files to be a real module
                py_files = list(module_dir.glob("*.py"))
                if not py_files:
                    continue

                module_name = module_dir.name
                page_name = f"Harvey {module_name.replace('_', ' ').title()}"
                page_path = brain_pages / f"{page_name}.md"

                if page_path.exists():
                    existing = page_path.read_text(encoding="utf-8")
                    if "auto-generated by bootstrap" not in existing:
                        stats["skipped"] += 1
                        continue

                # Build description from Python files
                py_names = [f.name for f in py_files if f.name != "__init__.py"]
                loc = sum(len(f.read_text(errors="replace").splitlines()) for f in py_files)

                # Check for AGENT.md or README in the module
                desc = ""
                for doc in ["AGENT.md", "README.md"]:
                    doc_path = module_dir / doc
                    if doc_path.exists():
                        content = doc_path.read_text(encoding="utf-8")
                        desc_match = re.search(r"^description:\s*(.+)$", content, re.MULTILINE)
                        if desc_match:
                            desc = desc_match.group(1).strip()
                        elif content.strip():
                            # Use first non-empty, non-heading line
                            for line in content.split("\n"):
                                line = line.strip()
                                if line and not line.startswith(("#", "-", "---", ">")):
                                    desc = line[:200]
                                    break

                page_content = (
                    f"- tags:: #harvey-os #core #{module_name}\n"
                    f"- auto-generated by bootstrap\n"
                    f"- module-path:: `core/{module_name}/`\n"
                    f"- description:: {desc or f'Harvey OS core module: {module_name}'}\n"
                    f"\n"
                    f"- ## Overview\n"
                    f"  - Core module at `core/{module_name}/` ({loc} LOC)\n"
                    f"  - Files: {', '.join(py_names[:10])}\n"
                )

                page_path.write_text(page_content, encoding="utf-8")
                stats["pages_created"] += 1
                log.info("  Bootstrapped core module: %s", module_name)

        # 3. Scan agents (agents/*/ with AGENT.md or Python files)
        agents_dir = harvey_os / "agents"
        if agents_dir.exists():
            for agent_dir in sorted(agents_dir.iterdir()):
                if not agent_dir.is_dir() or agent_dir.name.startswith(("_", ".")):
                    continue

                agent_name = agent_dir.name
                page_name = f"Agent {agent_name.replace('_', ' ').replace('-', ' ').title()}"
                page_path = brain_pages / f"{page_name}.md"

                if page_path.exists():
                    existing = page_path.read_text(encoding="utf-8")
                    if "auto-generated by bootstrap" not in existing:
                        stats["skipped"] += 1
                        continue

                desc = ""
                agent_md = agent_dir / "AGENT.md"
                if agent_md.exists():
                    content = agent_md.read_text(encoding="utf-8")
                    desc_match = re.search(r"^description:\s*(.+)$", content, re.MULTILINE)
                    if desc_match:
                        desc = desc_match.group(1).strip()

                py_files = list(agent_dir.glob("*.py"))
                py_names = [f.name for f in py_files if f.name != "__init__.py"]

                page_content = (
                    f"- tags:: #harvey-os #agent #{agent_name}\n"
                    f"- auto-generated by bootstrap\n"
                    f"- agent-path:: `agents/{agent_name}/`\n"
                    f"- description:: {desc or f'Harvey agent: {agent_name}'}\n"
                    f"\n"
                    f"- ## Overview\n"
                    f"  - Agent at `agents/{agent_name}/`\n"
                    f"  - Files: {', '.join(py_names[:10]) if py_names else 'see AGENT.md'}\n"
                )

                page_path.write_text(page_content, encoding="utf-8")
                stats["pages_created"] += 1
                log.info("  Bootstrapped agent: %s", agent_name)

        # 4. Create master capabilities page
        caps_path = brain_pages / "Harvey Capabilities.md"
        caps_content = (
            "- tags:: #harvey-os #index #capabilities\n"
            "- auto-generated by bootstrap\n"
            f"- last-bootstrap:: {time.strftime('%Y-%m-%d')}\n"
            "\n"
            "- ## Harvey OS Capabilities\n"
            f"  - {stats['skills_indexed']} skills indexed from SKILL.md files\n"
            "  - Run `superbrain bootstrap` to refresh\n"
            "\n"
        )

        # List all skill pages
        skill_pages = sorted(brain_pages.glob("*.md"))
        for p in skill_pages:
            content = p.read_text(encoding="utf-8")
            if "#skill" in content:
                caps_content += f"  - [[{p.stem}]]\n"

        caps_path.write_text(caps_content, encoding="utf-8")
        stats["pages_created"] += 1

        # 3. Sync everything to FTS5 + entity graph
        log.info("Syncing Brain after bootstrap...")
        sync_result = self.sync(force=True, embed=self._check_embedding())
        stats["sync"] = sync_result

        log.info("Bootstrap complete: %d skills, %d pages created",
                 stats["skills_indexed"], stats["pages_created"])
        return stats

    # ═══════════════════════════════════════════════════════════════
    #  STATUS
    # ═══════════════════════════════════════════════════════════════

    def status(self) -> dict:
        """Full status report."""
        store_stats = self.store.stats()
        memory_usage = self.memory.token_usage()

        return {
            "store": store_stats,
            "memory_stack": memory_usage,
            "embedding_available": self._check_embedding(),
            "brain_dir": self.brain_dir,
            "db_path": self.store.db_path,
        }

    def print_status(self):
        """Pretty-print status."""
        s = self.status()
        st = s["store"]
        mem = s["memory_stack"]

        print(f"\n╔══════════════════════════════════════════╗")
        print(f"║         Superbrain Status                ║")
        print(f"╠══════════════════════════════════════════╣")
        print(f"║  Pages:       {st['pages']:>5}                    ║")
        print(f"║  Journals:    {st['journals']:>5}                    ║")
        print(f"║  Vectors:     {st['vectors']:>5}                    ║")
        print(f"║  Triples:     {st['triples']:>5}                    ║")
        print(f"║  Events:      {st['events']:>5}                    ║")
        print(f"║  DB size:     {st['db_size_mb']:>5.1f} MB               ║")
        print(f"║  Embeddings:  {'YES' if s['embedding_available'] else 'NO':>5}                    ║")
        print(f"║  L0+L1:       ~{mem['l0_l1_total']:>3} tokens             ║")
        print(f"╚══════════════════════════════════════════╝\n")


# ═══════════════════════════════════════════════════════════════
#  CLI
# ═══════════════════════════════════════════════════════════════

if __name__ == "__main__":
    # Load .env
    env_path = os.path.join(HARVEY_HOME, ".env")
    if os.path.exists(env_path):
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    k, v = line.split("=", 1)
                    os.environ.setdefault(k.strip(), v.strip())

    sb = Superbrain()

    if len(sys.argv) < 2 or sys.argv[1] == "--help":
        print("Superbrain — Harvey's unified knowledge layer\n")
        print("Usage:")
        print("  superbrain status                     # show status")
        print("  superbrain sync [--force] [--embed]   # index Brain")
        print("  superbrain search \"terms\"              # FTS5 keyword search (no LLM)")
        print("  superbrain query \"your question\"       # search + LLM synthesis")
        print("  superbrain query --raw \"question\"      # search without LLM (= search)")
        print("  superbrain context [\"query\"]           # compact memory context")
        print("  superbrain stack [\"query\"]             # show memory stack (= context)")
        print("  superbrain remember \"summary\"          # append to today's journal")
        print("  superbrain gods                       # top entities")
        print("  superbrain neighbors \"entity\"          # entity relationships")
        print("  superbrain bootstrap                  # seed Brain from skills + modules")
        print("  superbrain promotions [--run|--stats]  # active memory promotion pipeline")
        print("  superbrain communities [resolution]    # knowledge graph communities (Louvain)")
        print("  superbrain trending                    # entities trending up/down (7d vs 30d)")
        print("  superbrain map [resolution]            # full knowledge landscape dashboard")
        sys.exit(0)

    cmd = sys.argv[1]

    if cmd == "status":
        sb.print_status()

    elif cmd == "sync":
        force = "--force" in sys.argv
        embed = "--embed" in sys.argv
        result = sb.sync(force=force, embed=embed)
        print(json.dumps(result, indent=2))
        sb.print_status()

    elif cmd in ("query", "search"):
        args = [a for a in sys.argv[2:] if not a.startswith("--")]
        # `search` is an alias for `query --raw` — FTS5 keyword search without LLM synthesis.
        raw = cmd == "search" or "--raw" in sys.argv
        question = " ".join(args)
        if not question:
            print(f"Error: provide a {'search term' if cmd == 'search' else 'question'}")
            sys.exit(1)

        result = sb.query(question, synthesize=not raw)

        print(f"\n{'=' * 60}")
        print("SUPERBRAIN")
        print(f"{'=' * 60}")
        print(result.answer)
        print(f"\n{'-' * 60}")
        print(f"Systems: {', '.join(result.systems_queried)} | "
              f"Sources: {len(result.sources)} | "
              f"Time: {result.query_time_sec:.2f}s")
        if result.sources:
            print("\nTop sources:")
            for s in result.sources[:5]:
                print(f"  [{s.source}: {s.title}] score={s.score:.3f}")

        try:
            from core.terminal.gimmicks import maybe_gimmick
            maybe_gimmick("search")
        except Exception:
            pass

    elif cmd in ("stack", "context"):
        query = " ".join(sys.argv[2:]) if len(sys.argv) > 2 else None
        if query:
            print(f"\n=== Memory {'Context' if cmd == 'context' else 'Stack'} (query: {query}) ===\n")
            print(sb.memory.for_query(query))
        else:
            print(f"\n=== Memory {'Context' if cmd == 'context' else 'Stack'} (compact) ===\n")
            print(sb.memory.compact())
        print(f"\n--- {sb.memory.token_usage()} ---")

    elif cmd == "remember":
        args = [a for a in sys.argv[2:] if not a.startswith("--")]
        if not args:
            print("Usage: superbrain remember \"summary of what happened\"")
            sys.exit(1)
        summary = " ".join(args)
        # Parse optional --agent and --type flags
        agent_flag = "cli"
        event_type = "note"
        for i, a in enumerate(sys.argv):
            if a == "--agent" and i + 1 < len(sys.argv):
                agent_flag = sys.argv[i + 1]
            elif a == "--type" and i + 1 < len(sys.argv):
                event_type = sys.argv[i + 1]
        ok = sb.remember(event_type=event_type, agent=agent_flag, summary=summary)
        if ok:
            print(f"logged to today's journal: [{event_type}] [[{agent_flag}]]: {summary}")
        else:
            print("remember failed — see logs")
            sys.exit(1)

    elif cmd == "gods":
        gods = sb.store.god_nodes(top_n=int(sys.argv[2]) if len(sys.argv) > 2 else 15)
        for g in gods:
            print(f"  {g['name']:<35} {g['mentions']:>3}x  {', '.join(g['sources'][:3])}")

    elif cmd == "bootstrap":
        print("Bootstrapping Harvey's self-knowledge...")
        result = sb.bootstrap()
        print(f"  Skills indexed: {result['skills_indexed']}")
        print(f"  Pages created: {result['pages_created']}")
        print(f"  Skipped: {result['skipped']}")
        if "sync" in result:
            sr = result["sync"]
            print(f"  FTS5: {sr.get('pages', 0) + sr.get('journals', 0)} indexed")
            if "vectors" in sr:
                print(f"  Vectors: {sr['vectors'].get('embedded', 0)} embedded")

    elif cmd == "neighbors":
        entity = " ".join(sys.argv[2:])
        if not entity:
            print("Usage: superbrain neighbors \"entity name\"")
            sys.exit(1)
        neighbors = sb.store.entity_neighbors(entity)
        for n in neighbors:
            direction = "→" if n["subject"] == entity else "←"
            other = n["object"] if n["subject"] == entity else n["subject"]
            print(f"  {direction} {other} ({n['predicate']}) conf={n['confidence']}")

    elif cmd == "promotions":
        from core.memory.memory_promoter import MemoryPromoter
        from core.memory.recall_tracker import RecallTracker

        if "--stats" in sys.argv:
            tracker = RecallTracker()
            stats = tracker.get_stats()
            print("\n=== Active Memory Recall Stats ===\n")
            print(f"  recall_log entries:  {stats.get('recall_log_entries', 0)}")
            print(f"  recall_stats entries: {stats.get('recall_stats_entries', 0)}")
            print(f"  promoted:            {stats.get('promoted_count', 0)}")
            top = stats.get("top_recalled", [])
            if top:
                print("\n  Top recalled:")
                for t in top:
                    print(f"    {t.get('doc_path', '?'):<50} "
                          f"recalls={t.get('recall_count', 0):>3} "
                          f"days={t.get('unique_days', 0):>2} "
                          f"max_score={t.get('max_score', 0):.2f}")
        elif "--run" in sys.argv:
            promoter = MemoryPromoter()
            report = promoter.promote()
            print(f"\n=== Active Memory Promotion Run ===\n")
            print(f"  Candidates scored: {report['candidates']}")
            print(f"  Promoted:          {report['promoted']}")
            for e in report.get("entries", []):
                snippet = (e.get("snippet") or "")[:80]
                print(f"    [{e.get('promotion_score', 0):.2f}] {snippet}")
        else:
            # Dry run — show candidates without promoting
            promoter = MemoryPromoter()
            report = promoter.promote(dry_run=True)
            print(f"\n=== Active Memory Promotion Candidates (dry run) ===\n")
            print(f"  Candidates: {report['candidates']}")
            if report.get("entries"):
                for e in report["entries"]:
                    snippet = (e.get("snippet") or "")[:60]
                    c = e.get("components", {})
                    print(f"\n  [{e.get('promotion_score', 0):.3f}] {snippet}")
                    print(f"    freq={c.get('frequency', 0):.2f} "
                          f"rel={c.get('relevance', 0):.2f} "
                          f"div={c.get('diversity', 0):.2f} "
                          f"rec={c.get('recency', 0):.2f} "
                          f"con={c.get('consolidation', 0):.2f} "
                          f"cpt={c.get('conceptual', 0):.2f} "
                          f"boost={c.get('phase_boost', 0):.3f}")
                    print(f"    recalls={e.get('recall_count', 0)} "
                          f"queries={e.get('unique_queries', 0)} "
                          f"days={e.get('unique_days', 0)} "
                          f"path={e.get('doc_path', '?')}")
            else:
                print("  No candidates meet promotion gates yet.")
                print("  (Need: 3+ recalls, 2+ unique queries, score >= 0.70)")

        try:
            from core.terminal.gimmicks import maybe_gimmick
            maybe_gimmick("memory")
        except Exception:
            pass

    elif cmd == "communities":
        from core.superbrain import graph as graph_mod
        resolution = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0
        g = graph_mod.load_from_store(sb.store)
        comms = graph_mod.communities_louvain(g, resolution=resolution)
        print(f"\nKnowledge Communities ({len(comms)} detected, resolution={resolution}):\n")
        for c in comms:
            top = ", ".join(c["top_nodes"][:3])
            print(f"  [{top}]")
            print(f"    {c['size']} nodes, cohesion={c['cohesion_score']}, "
                  f"internal_edges={c['internal_edges']}")
            extra = [m for m in c["members"][:8] if m not in c["top_nodes"][:3]]
            if extra:
                print(f"    also: {', '.join(extra)}")
            print()

    elif cmd == "trending":
        from core.superbrain import graph as graph_mod
        g = graph_mod.load_from_store(sb.store)
        trends = graph_mod.trending_entities(g)
        if not trends:
            print("\nNo significant trending changes detected.")
        else:
            print(f"\nTrending Entities (7-day vs 30-day):\n")
            for t in trends:
                arrow = "↑" if t["direction"] == "up" else "↓"
                print(f"  {arrow} {t['entity']:<30} {t['delta']:+.3f}  "
                      f"(7d:{t['recent_mentions']}, 30d:{t['baseline_mentions']})")

    elif cmd == "map":
        from core.superbrain import graph as graph_mod
        resolution = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0
        g = graph_mod.load_from_store(sb.store)
        print(graph_mod.knowledge_map(g, resolution=resolution))

    else:
        print(f"Unknown command: {cmd}. Run with --help for usage.")
        sys.exit(1)
