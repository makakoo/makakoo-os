#!/usr/bin/env python3
"""
Auto-Memory Vector Indexer — Semantic indexing of captured facts.

Phase 2 of Auto-Memory system. Subscribes to "memory.captured" events
and indexes facts as vectors in Superbrain for semantic search.

Features:
- Batch indexing (50 facts/batch)
- Hash-based deduplication
- Vector storage in SQLite (via SuperbrainStore)
- Semantic search integration with superbrain.query()

Usage:
    from core.memory.auto_memory_indexer import AutoMemoryIndexer
    indexer = AutoMemoryIndexer()
    indexer.start()  # Subscribe to memory.captured events
"""

import os
import sys
import logging
import hashlib
import json
from typing import Dict, Any, Optional, List
from datetime import datetime

# Add parent to path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from core.events import EventBus, Event
from core.superbrain import store as sb_store

log = logging.getLogger("harvey.auto_memory_indexer")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


class AutoMemoryIndexer:
    """
    Indexes captured facts as vectors in Superbrain.

    - Subscribes to memory.captured events from auto_memory_router
    - Batches facts for efficient indexing
    - Deduplicates based on content hash
    - Stores embeddings in SQLite for vector search
    """

    def __init__(self, batch_size: int = 50):
        self.bus = EventBus.instance()
        self.store = sb_store.SuperbrainStore()
        self.batch = []
        self.batch_size = batch_size
        self.seen_hashes = set()  # For deduplication
        self._load_existing_hashes()

    def start(self):
        """Subscribe to memory.captured events."""
        log.info("Starting Auto-Memory Indexer")
        self.bus.subscribe("memory.captured", self._on_fact_captured)

    def _load_existing_hashes(self):
        """Load existing fact hashes to avoid re-indexing."""
        try:
            # Get all existing vectors from store
            rows = self.store._conn.execute(
                "SELECT doc_id FROM brain_vectors"
            ).fetchall()

            # For now, just track count. Full dedup logic uses content hash.
            self.seen_hashes = {row["doc_id"] for row in rows}
            log.info(f"Loaded {len(self.seen_hashes)} existing indexed facts")
        except Exception as e:
            log.warning(f"Could not load existing hashes: {e}")

    def _on_fact_captured(self, event: Event):
        """Called when memory.captured event is emitted."""
        try:
            fact = event.data.get("fact", {})
            if not fact:
                log.debug("Empty fact in memory.captured event")
                return

            # Calculate hash for deduplication
            fact_hash = self._hash_fact(fact)
            if fact_hash in self.seen_hashes:
                log.debug(f"Fact already indexed: {fact_hash[:8]}")
                return

            # Add to batch
            self.batch.append({
                "fact": fact,
                "hash": fact_hash,
                "timestamp": datetime.now().isoformat(),
            })
            self.seen_hashes.add(fact_hash)

            # Flush batch if threshold reached
            if len(self.batch) >= self.batch_size:
                self._flush_batch()

        except Exception as e:
            log.error(f"Error processing memory.captured event: {e}", exc_info=True)

    def _hash_fact(self, fact: Dict[str, Any]) -> str:
        """
        Create a hash of the fact for deduplication.
        Uses the embedding + timestamp to avoid duplicate indexing.
        """
        try:
            # Use embedding + source event as unique key
            embedding = fact.get("_embedding")
            source_event = fact.get("_source_event", "")
            timestamp = fact.get("_timestamp", "")

            # Create deterministic hash
            key = f"{source_event}:{timestamp}:{str(embedding)[:50]}"
            return hashlib.sha256(key.encode()).hexdigest()
        except Exception:
            # Fallback: hash the entire fact
            return hashlib.sha256(json.dumps(fact, sort_keys=True, default=str).encode()).hexdigest()

    def _flush_batch(self):
        """
        Index all batched facts into Superbrain.
        Creates a virtual document for each fact and stores its embedding.
        """
        if not self.batch:
            return

        try:
            start_time = datetime.now()
            indexed_count = 0

            for item in self.batch:
                fact = item["fact"]
                embedding = fact.get("_embedding")

                if not embedding:
                    log.debug(f"Skipping fact with no embedding")
                    continue

                # Create a virtual document for this fact in the store
                doc_id = self._insert_fact_document(fact)
                if doc_id:
                    # Store the embedding vector
                    from core.superbrain.embeddings import CURRENT_MODEL
                    self.store.store_vector(
                        doc_id=doc_id,
                        embedding=embedding,
                        model=CURRENT_MODEL,
                    )
                    indexed_count += 1

            self.batch = []
            elapsed = (datetime.now() - start_time).total_seconds()
            log.info(f"Indexed {indexed_count} facts in {elapsed:.2f}s")

        except Exception as e:
            log.error(f"Error flushing batch: {e}", exc_info=True)

    def _insert_fact_document(self, fact: Dict[str, Any]) -> Optional[int]:
        """
        Insert a fact as a virtual document in brain_docs table.
        Returns the doc_id for use with store_vector().
        """
        try:
            # Create document name and path from fact metadata
            source_event = fact.get("_source_event", "unknown")
            timestamp = fact.get("_timestamp", "")
            significance = fact.get("_significance", 0.5)

            # Document name: event type + timestamp
            doc_name = f"fact_{source_event}_{timestamp[:10]}"
            doc_path = f"memory://captured/{source_event}/{timestamp[:10]}"

            # Create document content (human-readable summary)
            content = self._format_fact_content(fact)

            # Extract entities from fact
            entities = fact.get("_entities", [])

            # Content hash for incremental sync
            import hashlib
            content_hash = hashlib.sha256(content.encode()).hexdigest()

            # Insert into brain_docs table
            cursor = self.store._conn.execute("""
                INSERT INTO brain_docs
                (path, name, doc_type, content, content_hash, entities, char_count)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(path) DO UPDATE SET
                    content = excluded.content,
                    content_hash = excluded.content_hash,
                    entities = excluded.entities,
                    updated_at = datetime('now')
            """, (
                doc_path,
                doc_name,
                "captured_fact",
                content,
                content_hash,
                json.dumps(entities),
                len(content),
            ))
            self.store._conn.commit()

            return cursor.lastrowid

        except Exception as e:
            log.error(f"Error inserting fact document: {e}")
            return None

    def _format_fact_content(self, fact: Dict[str, Any]) -> str:
        """
        Format a fact dict as human-readable text for indexing.
        """
        try:
            lines = []
            for key, value in fact.items():
                if key.startswith("_"):  # Skip metadata
                    continue
                if isinstance(value, (str, int, float)):
                    lines.append(f"{key}: {value}")
                elif isinstance(value, list):
                    lines.append(f"{key}: {', '.join(str(x) for x in value[:5])}")
                elif isinstance(value, dict):
                    lines.append(f"{key}: {json.dumps(value, default=str)[:100]}")

            return "\n".join(lines) if lines else str(fact)
        except Exception:
            return str(fact)

    def flush_and_close(self):
        """Flush remaining batch and close store connection."""
        try:
            self._flush_batch()
            self.store.close()
            log.info("Auto-Memory Indexer closed")
        except Exception as e:
            log.error(f"Error closing indexer: {e}")


# ═════════════════════════════════════════════════════════════════════════════

if __name__ == "__main__":
    # Test the indexer
    logging.basicConfig(level=logging.DEBUG)

    indexer = AutoMemoryIndexer()
    indexer.start()

    # Emit test event
    bus = EventBus.instance()
    bus.publish(
        "memory.captured",
        source="test",
        fact={
            "type": "decision",
            "category": "testing",
            "action": "test_index",
            "_source_event": "test.event",
            "_timestamp": datetime.now().isoformat(),
            "_entities": ["[[TestEntity]]"],
            "_embedding": [0.1, 0.2, 0.3] * 100,  # Dummy embedding (300 dims)
            "_significance": 0.8,
        }
    )

    print("Test fact published. Indexer should process it.")
    import time
    time.sleep(2)  # Give it time to process
    indexer.flush_and_close()
    print("Done.")
