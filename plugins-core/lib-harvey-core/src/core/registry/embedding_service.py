#!/usr/bin/env python3
"""
Embedding Service — Chroma + sentence-transformers for semantic skill search.
"""

import os
import sys
from pathlib import Path
from typing import Optional

# Use the correct Python with dependencies installed
PYTHON = "/usr/local/opt/python@3.11/bin/python3.11"

# Add registry to path for imports
REGISTRY_DIR = Path(__file__).parent
sys.path.insert(0, str(REGISTRY_DIR))

# Data directory for Chroma persistence
DATA_DIR = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "data" / "knowledge" / "skills_embeddings"
DATA_DIR.mkdir(parents=True, exist_ok=True)

# Chroma collection name
COLLECTION_NAME = "harvey_skills"

# Embedding model
EMBEDDING_MODEL = "sentence-transformers/all-MiniLM-L6-v2"


class EmbeddingService:
    """Singleton service for embedding skills and searching."""

    _instance: Optional["EmbeddingService"] = None
    _embedder: Optional[object] = None
    _chroma_client: Optional[object] = None
    _collection: Optional[object] = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    @classmethod
    def get_instance(cls) -> "EmbeddingService":
        """Get the singleton instance."""
        if cls._instance is None:
            cls._instance = cls()
        return cls._instance

    def _load_dependencies(self):
        """Lazy-load Chroma and sentence-transformers."""
        if self._embedder is not None:
            return

        try:
            import chromadb
            from chromadb.config import Settings
            from sentence_transformers import SentenceTransformer
        except ImportError as e:
            print(f"Warning: Missing dependency: {e}", file=sys.stderr)
            raise

        # Initialize Chroma PersistentClient
        self._chroma_client = chromadb.PersistentClient(
            path=str(DATA_DIR),
            settings=Settings(anonymized_telemetry=False)
        )

        # Load embedding model
        self._embedder = SentenceTransformer(EMBEDDING_MODEL)

        # Get or create collection
        try:
            self._collection = self._chroma_client.get_collection(name=COLLECTION_NAME)
        except Exception:
            # Collection doesn't exist, create it
            self._collection = self._chroma_client.create_collection(
                name=COLLECTION_NAME,
                metadata={"model": EMBEDDING_MODEL}
            )

    @property
    def embedder(self):
        """Lazy-load embedder."""
        if self._embedder is None:
            self._load_dependencies()
        return self._embedder

    @property
    def collection(self):
        """Lazy-load collection."""
        if self._collection is None:
            self._load_dependencies()
        return self._collection

    def embed_texts(self, texts: list[str]) -> list[list[float]]:
        """Embed a list of texts using sentence-transformers."""
        if not texts:
            return []

        embeddings = self.embedder.encode(texts, convert_to_numpy=True)
        return embeddings.tolist()

    def embed_and_index_skills(self, skills: list[dict]) -> int:
        """
        Index all skills in Chroma for semantic search.

        Each skill should have: name, description, category, path, tags
        """
        if not skills:
            return 0

        self._load_dependencies()

        # Prepare data for Chroma
        ids = []
        documents = []
        metadatas = []
        embeddings = []

        for idx, skill in enumerate(skills):
            name = skill.get("name", "")
            description = skill.get("description", "")
            category = skill.get("category", "")
            path = skill.get("path", "")
            tags = skill.get("tags", [])

            # Use path + name for unique ID
            skill_id = f"{path}_{idx}".replace("/", "_").replace(" ", "_").lower()

            # Combine name and description for embedding
            text = f"{name}. {description}"

            ids.append(skill_id)
            documents.append(text)
            metadatas.append({
                "name": name,
                "description": description,
                "category": category,
                "path": path,
                "tags": ",".join(tags) if isinstance(tags, list) else str(tags),
            })

        # Batch embed all documents
        embeddings = self.embed_texts(documents)

        # Delete existing collection to re-index
        try:
            self._chroma_client.delete_collection(name=COLLECTION_NAME)
        except Exception:
            pass

        # Recreate collection
        self._collection = self._chroma_client.create_collection(
            name=COLLECTION_NAME,
            metadata={"model": EMBEDDING_MODEL}
        )

        # Add to collection
        self._collection.add(
            ids=ids,
            documents=documents,
            metadatas=metadatas,
            embeddings=embeddings
        )

        return len(skills)

    def search_skills(self, query: str, top_k: int = 10) -> list[dict]:
        """
        Semantic search for skills matching the query.

        Returns list of dicts with: name, description, category, path, tags, score
        """
        if not query:
            return []

        try:
            self._load_dependencies()
        except Exception as e:
            print(f"Warning: Could not load embedding dependencies: {e}", file=sys.stderr)
            return []

        # Embed the query
        query_embedding = self.embed_texts([query])

        if not query_embedding:
            return []

        # Search Chroma
        results = self.collection.query(
            query_texts=[query],
            query_embeddings=query_embedding,
            n_results=top_k,
            include=["metadatas", "distances"]
        )

        # Format results
        matched = []
        if results["ids"] and len(results["ids"]) > 0:
            for i, skill_id in enumerate(results["ids"][0]):
                metadata = results["metadatas"][0][i]
                distance = results["distances"][0][i] if results["distances"] else 0.0

                # Convert distance to similarity score (Chroma uses L2 distance)
                # For cosine similarity: score = 1 - distance/2
                score = max(0.0, 1.0 - distance / 2.0)

                tags = metadata.get("tags", "")
                if isinstance(tags, str):
                    tags = [t.strip() for t in tags.split(",") if t.strip()]

                matched.append({
                    "name": metadata.get("name", skill_id),
                    "description": metadata.get("description", ""),
                    "category": metadata.get("category", ""),
                    "path": metadata.get("path", ""),
                    "tags": tags,
                    "score": score,
                })

        return matched


def get_instance() -> EmbeddingService:
    """Convenience function to get the singleton instance."""
    return EmbeddingService.get_instance()


if __name__ == "__main__":
    # Test the service
    svc = get_instance()
    results = svc.search_skills("I want to review a pull request and check emails", top_k=5)
    print("Search results:")
    for r in results:
        print(f"  {r['name']}: {r['score']:.3f}")
        print(f"    Category: {r['category']}")
