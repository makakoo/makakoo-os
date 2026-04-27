"""
Layer 6: Artifact / Persistent Store

Long-term artifact registry with TTL, pinning, and garbage collection.
"""
import json
import time
import uuid
from pathlib import Path
from dataclasses import dataclass, field, asdict
from typing import Optional
from .layer5_session import SessionLayer


ARTIFACT_DIR = Path("data/Brain/artifacts")
ARTIFACT_REGISTRY = ARTIFACT_DIR / "registry.jsonl"


@dataclass
class Artifact:
    id: str
    name: str
    type: str  # mime type
    producer: str  # agent_id
    session: str
    created_at: float
    depends_on: list = field(default_factory=list)
    consumed_by: list = field(default_factory=list)
    ttl_seconds: int = 86400  # default 24h
    pinned: bool = False
    content_size: int = 0
    content: str = ""


class ArtifactLayer:
    """Layer 6: Long-term artifact registry and state persistence."""

    def __init__(self, session: SessionLayer):
        self.session = session
        ARTIFACT_DIR.mkdir(parents=True, exist_ok=True)
        self._cache: dict[str, Artifact] = {}
        self._load_registry()

    def _load_registry(self):
        """Load artifact registry into cache."""
        if not ARTIFACT_REGISTRY.exists():
            return
        with open(ARTIFACT_REGISTRY) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                art = Artifact(**json.loads(line))
                self._cache[art.id] = art

    def _append_to_registry(self, art: Artifact):
        """Append artifact to JSONL registry."""
        with open(ARTIFACT_REGISTRY, "a") as f:
            f.write(json.dumps(asdict(art)) + "\n")

    def create_artifact(
        self,
        name: str,
        content: str,
        producer: str,
        depends_on: list = None,
        ttl_seconds: int = 86400,
        pinned: bool = False,
    ) -> Artifact:
        """Create and persist a new artifact."""
        current_session = getattr(self.session, "current_session_id", "unknown")
        art = Artifact(
            id=f"artifact://harvey/{uuid.uuid4()}",
            name=name,
            type="text/markdown",
            producer=producer,
            session=current_session,
            created_at=time.time(),
            depends_on=depends_on or [],
            consumed_by=[],
            ttl_seconds=ttl_seconds,
            pinned=pinned,
            content_size=len(content),
            content=content,
        )
        self._cache[art.id] = art
        self._append_to_registry(art)
        # Track in session
        self.session.add_artifact_to_session(current_session, art.id)
        return art

    def get_artifact(self, artifact_id: str) -> Optional[Artifact]:
        """Retrieve an artifact by ID."""
        return self._cache.get(artifact_id)

    def add_dependency(self, artifact_id: str, depends_on_id: str):
        """Add a dependency edge to an artifact."""
        art = self._cache.get(artifact_id)
        if art and depends_on_id not in art.depends_on:
            art.depends_on.append(depends_on_id)

    def add_consumer(self, artifact_id: str, consumer_id: str):
        """Record that an artifact is consumed by another artifact."""
        art = self._cache.get(artifact_id)
        if art and consumer_id not in art.consumed_by:
            art.consumed_by.append(consumer_id)

    def gc_artifacts(self) -> int:
        """Remove expired (non-pinned) artifacts. Returns count of removed."""
        now = time.time()
        valid = []
        removed = 0
        if not ARTIFACT_REGISTRY.exists():
            return 0
        with open(ARTIFACT_REGISTRY) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                art_dict = json.loads(line)
                age = now - art_dict["created_at"]
                if art_dict["pinned"] or age < art_dict["ttl_seconds"]:
                    valid.append(line)
                else:
                    self._cache.pop(art_dict["id"], None)
                    removed += 1
        with open(ARTIFACT_REGISTRY, "w") as f:
            f.write("\n".join(valid) + "\n")
        return removed

    def register_multimodal_doc(
        self,
        title: str,
        content_type: str,
        doc_ids: list[str],
        filename: str,
        content: str = "",
        ttl_seconds: int = 86400 * 30,
    ) -> Artifact:
        """Register a multimodal document ingestion as a Layer 6 artifact.

        Args:
            title: Human-readable title of the document
            content_type: Type e.g. 'video', 'audio', 'pdf', 'image', 'text'
            doc_ids: List of Supabase/postgres doc IDs for traceability
            filename: Original filename
            content: Text excerpt (optional, for searchability)
            ttl_seconds: How long before this artifact is GC'd (default 30 days)

        Returns:
            The created Artifact
        """
        art = self.create_artifact(
            name=title,
            content=content or f"Multimodal {content_type}: {filename}",
            producer="multimodal-knowledge",
            depends_on=[],
            ttl_seconds=ttl_seconds,
            pinned=False,
        )
        # Link Supabase doc IDs as consumers so we can trace back
        for doc_id in doc_ids:
            self.add_consumer(art.id, doc_id)
        return art

    def list_artifacts(self, producer: str = "", limit: int = 100) -> list[Artifact]:
        """List artifacts, optionally filtered by producer."""
        arts = list(self._cache.values())
        if producer:
            arts = [a for a in arts if a.producer == producer]
        return sorted(arts, key=lambda a: a.created_at, reverse=True)[:limit]
