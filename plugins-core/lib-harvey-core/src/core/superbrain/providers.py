"""
Vector Store provider abstractions.

The embedding pipeline is now handled directly by Superbrain._embed
against switchAILocal — the pluggable EmbeddingProvider abstraction
was never used outside the class definitions themselves and has been
removed. SearchHit and the VectorStore implementations below remain
active (mempalace_client.py imports SearchHit).
"""

import logging
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import List, Optional, Dict, Any

log = logging.getLogger("superbrain.providers")


# ═══════════════════════════════════════════════════════════════
#  Vector Store Providers
# ═══════════════════════════════════════════════════════════════

@dataclass
class SearchHit:
    """Standard search result from any vector store."""
    score: float
    text: str
    title: str
    source: str
    metadata: dict = field(default_factory=dict)


class VectorStore(ABC):
    """Base class for all vector stores."""
    name: str = "base"

    @abstractmethod
    def available(self) -> bool:
        pass

    @abstractmethod
    def ensure_collection(self, collection: str, dim: int):
        pass

    @abstractmethod
    def upsert(self, collection: str, point_id: str, vector: List[float], payload: dict):
        pass

    @abstractmethod
    def search(self, collection: str, vector: List[float], top_k: int = 10,
               filter_dict: dict = None) -> List[SearchHit]:
        pass

    def collection_count(self, collection: str) -> int:
        return 0


class QdrantStore(VectorStore):
    """Qdrant vector store (Docker or cloud)."""
    name = "qdrant"

    def __init__(self, host: str = None, port: int = None):
        import os
        self.host = host or os.environ.get("QDRANT_HOST", "localhost")
        self.port = port or int(os.environ.get("QDRANT_PORT", "6333"))
        self.base_url = f"http://{self.host}:{self.port}"

    def available(self) -> bool:
        import requests
        try:
            resp = requests.get(f"{self.base_url}/collections", timeout=2)
            return resp.status_code == 200
        except Exception:
            return False

    def ensure_collection(self, collection: str, dim: int):
        import requests
        resp = requests.get(f"{self.base_url}/collections/{collection}", timeout=5)
        if resp.status_code == 200:
            return
        requests.put(f"{self.base_url}/collections/{collection}", json={
            "vectors": {"size": dim, "distance": "Cosine"},
            "hnsw_config": {"m": 16, "ef_construct": 100},
            "on_disk_payload": True,
        }, timeout=10)
        log.info("Created Qdrant collection: %s (dim=%d)", collection, dim)

    def upsert(self, collection: str, point_id: str, vector: List[float], payload: dict):
        import requests
        resp = requests.put(f"{self.base_url}/collections/{collection}/points", json={
            "points": [{"id": point_id, "vector": vector, "payload": payload}],
        }, timeout=10)
        return resp.status_code == 200

    def search(self, collection: str, vector: List[float], top_k: int = 10,
               filter_dict: dict = None) -> List[SearchHit]:
        import requests
        body = {"vector": vector, "limit": top_k, "with_payload": True}
        if filter_dict:
            body["filter"] = filter_dict

        try:
            resp = requests.post(
                f"{self.base_url}/collections/{collection}/points/search",
                json=body, timeout=5,
            )
            if resp.status_code != 200:
                return []

            results = []
            for hit in resp.json().get("result", []):
                p = hit.get("payload", {})
                text = p.get("content", "") or p.get("text_content", "")
                if not text:
                    continue
                results.append(SearchHit(
                    score=float(hit["score"]),
                    text=text[:1000],
                    title=p.get("page_name") or p.get("title") or p.get("journal_date") or p.get("filename", ""),
                    source=f"qdrant:{collection}",
                    metadata=p,
                ))
            return results
        except Exception as e:
            log.error("Qdrant search error: %s", e)
            return []

    def collection_count(self, collection: str) -> int:
        import requests
        try:
            resp = requests.get(f"{self.base_url}/collections/{collection}", timeout=2)
            if resp.status_code == 200:
                return resp.json()["result"]["points_count"]
        except Exception:
            pass
        return 0

    def scroll_payloads(self, collection: str, fields: list, limit: int = 100) -> List[dict]:
        """Scroll all payloads (for change detection). Returns list of payload dicts."""
        import requests
        all_payloads = []
        offset = None
        while True:
            body = {"limit": limit, "with_payload": fields}
            if offset:
                body["offset"] = offset
            try:
                resp = requests.post(
                    f"{self.base_url}/collections/{collection}/points/scroll",
                    json=body, timeout=10,
                )
                if resp.status_code != 200:
                    break
                data = resp.json()["result"]
                for pt in data["points"]:
                    all_payloads.append(pt.get("payload", {}))
                offset = data.get("next_page_offset")
                if not offset:
                    break
            except Exception:
                break
        return all_payloads


class ChromaStore(VectorStore):
    """Chroma embedded vector store (no server needed, local SQLite)."""
    name = "chroma"

    def __init__(self, persist_dir: str = None):
        import os
        self.persist_dir = persist_dir or os.path.join(
            os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
            "data", "knowledge", "superbrain_chroma"
        )
        self._client = None

    def _get_client(self):
        if self._client is None:
            try:
                import chromadb
                self._client = chromadb.PersistentClient(path=self.persist_dir)
            except ImportError:
                log.warning("chromadb not installed. Run: pip install chromadb")
                return None
        return self._client

    def available(self) -> bool:
        try:
            import chromadb
            return True
        except ImportError:
            return False

    def ensure_collection(self, collection: str, dim: int):
        client = self._get_client()
        if client:
            client.get_or_create_collection(collection, metadata={"hnsw:space": "cosine"})

    def upsert(self, collection: str, point_id: str, vector: List[float], payload: dict):
        client = self._get_client()
        if not client:
            return False
        coll = client.get_or_create_collection(collection, metadata={"hnsw:space": "cosine"})
        text = payload.get("content", "") or payload.get("text_content", "")
        coll.upsert(
            ids=[point_id],
            embeddings=[vector],
            documents=[text[:5000]],
            metadatas=[{k: str(v) if not isinstance(v, (str, int, float, bool)) else v
                        for k, v in payload.items() if k != "content"}],
        )
        return True

    def search(self, collection: str, vector: List[float], top_k: int = 10,
               filter_dict: dict = None) -> List[SearchHit]:
        client = self._get_client()
        if not client:
            return []
        try:
            coll = client.get_collection(collection)
            results = coll.query(query_embeddings=[vector], n_results=top_k)
            hits = []
            for i, doc in enumerate(results["documents"][0]):
                meta = results["metadatas"][0][i] if results.get("metadatas") else {}
                dist = results["distances"][0][i] if results.get("distances") else 0
                score = 1.0 - dist  # chroma returns distance, we want similarity
                hits.append(SearchHit(
                    score=score,
                    text=doc[:1000],
                    title=meta.get("page_name") or meta.get("title") or meta.get("journal_date", ""),
                    source=f"chroma:{collection}",
                    metadata=meta,
                ))
            return hits
        except Exception as e:
            log.error("Chroma search error: %s", e)
            return []

    def collection_count(self, collection: str) -> int:
        client = self._get_client()
        if not client:
            return 0
        try:
            return client.get_collection(collection).count()
        except Exception:
            return 0


# ═══════════════════════════════════════════════════════════════
#  Brain Filesystem Search (always available, zero dependencies)
# ═══════════════════════════════════════════════════════════════

class BrainFilesystemSearch:
    """TF-IDF-ranked Brain search. Works on every install, zero dependencies."""
    name = "brain_fs"

    # Common English stop words to skip during scoring
    STOP_WORDS = frozenset({
        "the", "and", "for", "are", "but", "not", "you", "all", "can", "had",
        "her", "was", "one", "our", "out", "has", "his", "how", "its", "may",
        "new", "now", "old", "see", "way", "who", "did", "get", "got", "let",
        "say", "she", "too", "use", "with", "from", "this", "that", "what",
        "when", "where", "which", "while", "about", "after", "being", "could",
        "every", "first", "have", "into", "just", "like", "long", "make",
        "many", "most", "only", "over", "some", "such", "take", "than",
        "them", "then", "they", "very", "will", "been", "each", "more",
        "also", "back", "made", "much", "should", "would", "their", "there",
        "these", "those", "through", "using", "based", "does", "done",
    })

    def __init__(self, brain_dir: str = None):
        import os
        self.brain_dir = brain_dir or os.path.join(
            os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")),
            "data", "Brain"
        )
        self._doc_freq_cache: Optional[Dict] = None
        self._doc_count: int = 0

    def available(self) -> bool:
        from pathlib import Path
        return Path(self.brain_dir).exists()

    def _tokenize(self, text: str) -> List[str]:
        """Extract meaningful tokens, skip stop words and short fragments."""
        import re
        words = re.findall(r'[a-z][a-z0-9_-]+', text.lower())
        return [w for w in words if len(w) > 2 and w not in self.STOP_WORDS]

    def _build_doc_freq(self):
        """Build document frequency index (how many docs contain each term). Cached."""
        if self._doc_freq_cache is not None:
            return
        from pathlib import Path
        import math

        doc_freq: Dict[str, int] = {}
        doc_count = 0
        brain = Path(self.brain_dir)

        for md_file in brain.rglob("*.md"):
            # Skip logseq internals
            if "logseq/" in str(md_file) or ".git" in str(md_file):
                continue
            try:
                content = md_file.read_text(encoding="utf-8")
            except Exception:
                continue
            doc_count += 1
            seen = set()
            for token in self._tokenize(content):
                if token not in seen:
                    doc_freq[token] = doc_freq.get(token, 0) + 1
                    seen.add(token)

        self._doc_freq_cache = doc_freq
        self._doc_count = doc_count

    def search(self, query_text: str, top_k: int = 10) -> List[SearchHit]:
        """TF-IDF ranked keyword search over Brain files."""
        from pathlib import Path
        import math
        import re
        from datetime import datetime, timedelta

        self._build_doc_freq()

        query_tokens = self._tokenize(query_text)
        if not query_tokens:
            return []

        # Also extract exact phrases (bigrams) from query for phrase matching bonus
        query_lower = query_text.lower()
        query_bigrams = []
        qt = query_tokens
        for i in range(len(qt) - 1):
            query_bigrams.append(f"{qt[i]} {qt[i+1]}")

        results = []
        brain = Path(self.brain_dir)
        N = max(self._doc_count, 1)

        for md_file in brain.rglob("*.md"):
            if "logseq/" in str(md_file) or ".git" in str(md_file):
                continue
            try:
                content = md_file.read_text(encoding="utf-8")
            except Exception:
                continue

            if len(content.strip()) < 20:
                continue

            content_lower = content.lower()

            # Quick check: at least one query token must appear
            if not any(t in content_lower for t in query_tokens):
                continue

            # Tokenize document
            doc_tokens = self._tokenize(content)
            if not doc_tokens:
                continue

            doc_len = len(doc_tokens)

            # Term frequency (normalized by doc length to avoid long-doc bias)
            tf_counts: Dict[str, int] = {}
            for t in doc_tokens:
                tf_counts[t] = tf_counts.get(t, 0) + 1

            # TF-IDF score
            tfidf_score = 0.0
            matched_terms = 0
            for qt_token in set(query_tokens):
                tf = tf_counts.get(qt_token, 0)
                if tf == 0:
                    continue
                matched_terms += 1
                # Normalized TF: log(1 + tf) / log(1 + doc_len)
                tf_norm = math.log(1 + tf) / math.log(1 + doc_len)
                # IDF: log(N / (1 + df))
                df = self._doc_freq_cache.get(qt_token, 1)
                idf = math.log(N / (1 + df))
                tfidf_score += tf_norm * idf

            if matched_terms == 0:
                continue

            # Coverage bonus: fraction of query terms matched (0-1)
            coverage = matched_terms / len(set(query_tokens))

            # Phrase bonus: consecutive query terms appearing together in doc
            phrase_bonus = 0.0
            for bigram in query_bigrams:
                if bigram in content_lower:
                    phrase_bonus += 0.15

            # Title match bonus: query terms in filename
            title = md_file.stem.lower().replace("_", " ").replace("-", " ")
            title_tokens = set(self._tokenize(title))
            title_overlap = len(set(query_tokens) & title_tokens)
            title_bonus = 0.3 * title_overlap if title_overlap else 0.0

            # Wikilink density bonus: more [[links]] = more structured/authoritative
            wikilinks = len(re.findall(r'\[\[', content))
            link_bonus = min(0.1, wikilinks * 0.005)

            # Recency bonus for journals
            recency_bonus = 0.0
            rel_path = md_file.relative_to(brain)
            source_type = "journal" if "journals" in str(rel_path) else "page"
            if source_type == "journal":
                try:
                    date_str = md_file.stem.replace("_", "-")[:10]
                    doc_date = datetime.strptime(date_str, "%Y-%m-%d")
                    days_ago = (datetime.now() - doc_date).days
                    if days_ago <= 7:
                        recency_bonus = 0.2 * (1 - days_ago / 7)
                    elif days_ago <= 30:
                        recency_bonus = 0.05 * (1 - days_ago / 30)
                except (ValueError, IndexError):
                    pass

            # Final composite score
            score = (
                tfidf_score * 0.5          # core relevance
                + coverage * 0.25           # query coverage
                + phrase_bonus              # exact phrase matches
                + title_bonus               # filename relevance
                + link_bonus                # authority signal
                + recency_bonus             # freshness for journals
            )

            # Extract best matching snippet (not just first 1000 chars)
            snippet = self._extract_snippet(content, query_tokens, max_len=1000)

            results.append(SearchHit(
                score=round(score, 4),
                text=snippet,
                title=md_file.stem,
                source=f"brain_fs:{source_type}",
                metadata={"path": str(md_file), "type": source_type,
                          "matched_terms": matched_terms,
                          "total_query_terms": len(set(query_tokens))},
            ))

        results.sort(key=lambda x: x.score, reverse=True)
        return results[:top_k]

    def _extract_snippet(self, content: str, query_tokens: List[str],
                         max_len: int = 1000) -> str:
        """Extract the most relevant snippet around query term occurrences."""
        content_lower = content.lower()
        best_pos = 0
        best_density = 0

        # Sliding window to find densest region of query terms
        window = min(max_len, len(content))
        step = max(100, window // 10)

        for start in range(0, max(1, len(content) - window + 1), step):
            chunk = content_lower[start:start + window]
            density = sum(chunk.count(t) for t in query_tokens)
            if density > best_density:
                best_density = density
                best_pos = start

        snippet = content[best_pos:best_pos + max_len]
        # Clean up: don't start mid-line
        if best_pos > 0:
            nl = snippet.find('\n')
            if nl > 0 and nl < 100:
                snippet = snippet[nl + 1:]
        return snippet.strip()
