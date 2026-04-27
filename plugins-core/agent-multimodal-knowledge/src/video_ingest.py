#!/usr/bin/env python3
"""
Video-OCR Transcript Ingest — Scans video-ocr pipeline output and ingests transcripts into Qdrant.

Reads transcript files from $HARVEY_HOME/data/video-ocr/videos/*/,
chunks them, embeds via Gemini Embedding 2, and stores in the "multimodal" Qdrant collection.
Skips videos already ingested (checks by doc_id prefix).
"""
import os
import sys
from pathlib import Path
from uuid import uuid4

import numpy as np
from dotenv import load_dotenv
from google import genai
from google.genai import types
from qdrant_client import QdrantClient
from qdrant_client.http import models

# Load environment
HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
load_dotenv(HARVEY_HOME / ".env")

GEMINI_API_KEY = os.getenv("GEMINI_API_KEY")
QDRANT_URL = os.getenv("QDRANT_URL", "http://localhost:6333")
COLLECTION = "multimodal"
EMBED_MODEL = "gemini-embedding-2-preview"
VIDEO_OCR_DIR = HARVEY_HOME / "data" / "video-ocr" / "videos"

CHUNK_SIZE = 6000
CHUNK_OVERLAP = 500

client_gemini = genai.Client(api_key=GEMINI_API_KEY)
client_qdrant = QdrantClient(url=QDRANT_URL)


def ensure_collection():
    """Create the Qdrant collection if it doesn't exist."""
    collections = [c.name for c in client_qdrant.get_collections().collections]
    if COLLECTION not in collections:
        client_qdrant.create_collection(
            collection_name=COLLECTION,
            vectors_config=models.VectorParams(size=3072, distance=models.Distance.COSINE),
        )
        print(f"Created collection '{COLLECTION}'")


def get_ingested_doc_ids() -> set:
    """Return all doc_id values already in Qdrant for video transcripts."""
    ingested = set()
    offset = None
    while True:
        result = client_qdrant.scroll(
            collection_name=COLLECTION,
            scroll_filter=models.Filter(
                must=[
                    models.FieldCondition(
                        key="content_type",
                        match=models.MatchValue(value="video_transcript"),
                    )
                ]
            ),
            limit=100,
            offset=offset,
            with_payload=["doc_id"],
        )
        points, next_offset = result
        for point in points:
            ingested.add(point.payload.get("doc_id", ""))
        if next_offset is None:
            break
        offset = next_offset
    return ingested


def chunk_text(text: str) -> list[str]:
    """Split text into chunks of CHUNK_SIZE with CHUNK_OVERLAP overlap."""
    chunks = []
    start = 0
    while start < len(text):
        end = start + CHUNK_SIZE
        chunks.append(text[start:end])
        start = end - CHUNK_OVERLAP
    return chunks


def embed_text(text: str) -> list[float]:
    """Embed a text chunk via Gemini Embedding 2, return normalized vector."""
    part = types.Part(text=text)
    resp = client_gemini.models.embed_content(
        model=EMBED_MODEL,
        contents=part,
        config=types.EmbedContentConfig(task_type="RETRIEVAL_DOCUMENT"),
    )
    vec = resp.embeddings[0].values
    norm = np.linalg.norm(vec)
    if norm > 0:
        vec = [v / norm for v in vec]
    return vec


def store_chunk(doc_id: str, video_id: str, filename: str, chunk_idx: int,
                total_chunks: int, text_content: str, vec: list[float]) -> str:
    """Store a single chunk in Qdrant."""
    point_id = str(uuid4())
    client_qdrant.upsert(
        collection_name=COLLECTION,
        points=[
            models.PointStruct(
                id=point_id,
                vector=vec,
                payload={
                    "doc_id": doc_id,
                    "title": video_id,
                    "content_type": "video_transcript",
                    "filename": filename,
                    "chunk_index": chunk_idx,
                    "chunk_total": total_chunks,
                    "text_content": text_content,
                    "metadata": {
                        "source": str(VIDEO_OCR_DIR / video_id / filename),
                        "file_type": "video_transcript",
                        "chunk_info": f"chars_{chunk_idx * (CHUNK_SIZE - CHUNK_OVERLAP)}-{chunk_idx * (CHUNK_SIZE - CHUNK_OVERLAP) + len(text_content)}",
                    },
                },
            )
        ],
    )
    return point_id


def find_transcript(video_dir: Path) -> Path | None:
    """Find the transcript file in a video directory. Prefers transcript.txt, falls back to jina_transcript.txt."""
    for name in ["transcript.txt", "jina_transcript.txt"]:
        p = video_dir / name
        if p.exists():
            return p
    return None


def ingest_video_transcripts(dry_run: bool = False):
    """Scan all video dirs and ingest transcripts that haven't been ingested yet."""
    if not VIDEO_OCR_DIR.exists():
        print(f"Video-OCR directory not found: {VIDEO_OCR_DIR}")
        sys.exit(1)

    ensure_collection()

    # Get already-ingested doc_ids
    ingested = get_ingested_doc_ids()
    print(f"Found {len(ingested)} already-ingested video transcript chunks\n")

    video_dirs = sorted([d for d in VIDEO_OCR_DIR.iterdir() if d.is_dir()])
    print(f"Found {len(video_dirs)} video directories\n")

    total_ingested = 0
    total_skipped = 0

    for video_dir in video_dirs:
        video_id = video_dir.name
        transcript_path = find_transcript(video_dir)

        if transcript_path is None:
            print(f"[SKIP] {video_id} — no transcript found")
            total_skipped += 1
            continue

        # Check if first chunk doc_id already exists
        first_doc_id = f"vt_{video_id}_000"
        if first_doc_id in ingested:
            print(f"[SKIP] {video_id} — already ingested")
            total_skipped += 1
            continue

        text = transcript_path.read_text(encoding="utf-8", errors="replace").strip()
        if not text:
            print(f"[SKIP] {video_id} — empty transcript")
            total_skipped += 1
            continue

        chunks = chunk_text(text)
        filename = transcript_path.name
        print(f"[INGEST] {video_id} — {len(text)} chars, {len(chunks)} chunks ({filename})")

        if dry_run:
            total_ingested += len(chunks)
            continue

        for idx, chunk in enumerate(chunks):
            doc_id = f"vt_{video_id}_{idx:03d}"
            try:
                vec = embed_text(chunk)
                point_id = store_chunk(doc_id, video_id, filename, idx, len(chunks), chunk, vec)
                print(f"  Chunk {idx + 1}/{len(chunks)}: {len(chunk)} chars [{point_id[:8]}...]")
            except Exception as e:
                print(f"  Chunk {idx + 1}/{len(chunks)}: FAILED — {e}")

        total_ingested += len(chunks)

    print(f"\nDone. Ingested: {total_ingested} chunks from {len(video_dirs) - total_skipped} videos. Skipped: {total_skipped}.")


if __name__ == "__main__":
    dry_run = "--dry-run" in sys.argv
    if dry_run:
        print("=== DRY RUN (no writes) ===\n")
    ingest_video_transcripts(dry_run=dry_run)
