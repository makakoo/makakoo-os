#!/usr/bin/env python3
"""
Multimodal Knowledge CLI — Wrapper for the RAG pipeline.

Usage:
    python3 multimodal_knowledge.py ingest <file> --title "..."
    python3 multimodal_knowledge.py query "question text"
    python3 multimodal_knowledge.py stats

Loads .env from ~/MAKAKOO/.env and uses the core.orchestration.memory_substrate
artifact registry (lives inside the lib-harvey-core plugin).
"""

import argparse
import os
import sys
import json
from pathlib import Path
from datetime import datetime

# Resolve HARVEY_HOME
HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
ENV_FILE = HARVEY_HOME / ".env"

# Load .env before any other imports that might need API keys
if ENV_FILE.exists():
    with open(ENV_FILE) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                os.environ[k.strip()] = v.strip()

# Add multimodal-rag lib to path (must happen after env is loaded)
RAG_DIR = HARVEY_HOME / "tmp" / "multimodal-rag"
sys.path.insert(0, str(RAG_DIR))

# Add memory substrate for Layer 6 integration
MEMORY_SUBSTRATE_DIR = HARVEY_HOME / "plugins-core" / "orchestration" / "memory_substrate"
sys.path.insert(0, str(MEMORY_SUBSTRATE_DIR))

from lib import rag, db


def _register_artifact(title: str, content_type: str, doc_ids: list[str], filename: str, text_excerpt: str = ""):
    """Register ingested content as a Layer 6 artifact."""
    try:
        from substrate import MemorySubstrate
        substrate = MemorySubstrate()
        art = substrate.artifact.create_artifact(
            name=title,
            content=text_excerpt or f"Multimodal {content_type}: {filename}",
            producer="multimodal-knowledge",
            depends_on=[],
            ttl_seconds=86400 * 30,  # 30 days default
            pinned=False,
        )
        # Store Supabase doc IDs in session metadata
        if doc_ids and hasattr(substrate.artifact.session, 'current_session_id'):
            sess_id = substrate.artifact.session.current_session_id
            # Also store in artifact consumed_by for traceability
            for doc_id in doc_ids:
                substrate.artifact.add_consumer(art.id, doc_id)
        return art.id
    except Exception as e:
        print(f"[WARNING] Could not register artifact in Layer 6: {e}", file=sys.stderr)
        return None


def ingest_file(file_path: str, title: str) -> dict:
    """Ingest a file into the vector store."""
    path = Path(file_path)
    if not path.exists():
        return {"error": f"File not found: {file_path}"}

    file_bytes = path.read_bytes()
    filename = path.name

    # Detect MIME type from extension
    ext = filename.rsplit(".", 1)[-1].lower() if "." in filename else ""
    mime_map = {
        "pdf": "application/pdf",
        "mp4": "video/mp4",
        "mov": "video/quicktime",
        "avi": "video/x-msvideo",
        "mp3": "audio/mpeg",
        "wav": "audio/wav",
        "png": "image/png",
        "jpg": "image/jpeg",
        "jpeg": "image/jpeg",
        "webp": "image/webp",
        "gif": "image/gif",
        "txt": "text/plain",
    }
    mime_type = mime_map.get(ext, "application/octet-stream")

    # Ingest via RAG pipeline
    results = rag.ingest(
        file_bytes=file_bytes,
        filename=filename,
        title=title,
        mime_type=mime_type,
    )

    doc_ids = [r.get("id") for r in results if r.get("id")]
    content_type = rag.detect_content_type(mime_type, filename)
    excerpt = ""
    if results and results[0].get("text_content"):
        excerpt = results[0]["text_content"][:500]

    # Register in Layer 6 artifact registry
    artifact_id = _register_artifact(title, content_type, doc_ids, filename, excerpt)

    return {
        "title": title,
        "content_type": content_type,
        "filename": filename,
        "chunks_ingested": len(results),
        "doc_ids": doc_ids,
        "artifact_id": artifact_id,
    }


def query_knowledge(query_text: str, top_k: int = 5, threshold: float = 0.5) -> dict:
    """Query the knowledge base."""
    result = rag.query(
        query_text=query_text,
        top_k=top_k,
        threshold=threshold,
        use_codex=True,
    )
    return result


def show_stats() -> dict:
    """Return knowledge base statistics."""
    try:
        stats = db.get_stats()
    except Exception as e:
        return {"error": str(e), "total": 0, "by_type": {}}

    # Also get from Layer 6
    artifact_count = 0
    try:
        from substrate import MemorySubstrate
        substrate = MemorySubstrate()
        arts = substrate.artifact.list_artifacts(producer="multimodal-knowledge", limit=1000)
        artifact_count = len(arts)
    except Exception:
        pass

    return {
        "stats": stats,
        "artifacts_registered": artifact_count,
    }


def main():
    parser = argparse.ArgumentParser(description="Multimodal Knowledge CLI")
    sub = parser.add_subparsers(dest="command", required=True)

    # Ingest command
    ingest_parser = sub.add_parser("ingest", help="Ingest a file into the knowledge base")
    ingest_parser.add_argument("file", help="Path to the file to ingest")
    ingest_parser.add_argument("--title", "-t", required=True, help="Title for this content")

    # Query command
    query_parser = sub.add_parser("query", help="Query the knowledge base")
    query_parser.add_argument("text", help="Query text")
    query_parser.add_argument("--top-k", "-k", type=int, default=5, help="Number of results")
    query_parser.add_argument("--threshold", type=float, default=0.5, help="Similarity threshold")

    # Stats command
    sub.add_parser("stats", help="Show knowledge base statistics")

    args = parser.parse_args()

    if args.command == "ingest":
        result = ingest_file(args.file, args.title)
        print(json.dumps(result, indent=2, default=str))

    elif args.command == "query":
        result = query_knowledge(args.text, top_k=args.top_k, threshold=args.threshold)
        print(json.dumps(result, indent=2, default=str))

    elif args.command == "stats":
        result = show_stats()
        print(json.dumps(result, indent=2, default=str))


if __name__ == "__main__":
    main()
