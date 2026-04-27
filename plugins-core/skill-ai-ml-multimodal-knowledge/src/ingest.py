#!/usr/bin/env python3
"""
Multimodal Knowledge Ingest — Gemini Embedding 2 + Qdrant
Supports: PDF, Images (PNG/JPEG/WebP), Video (MP4), Audio (M4A/MP3/WAV), Text
"""
import os, json, sys, subprocess, tempfile
from pathlib import Path
from uuid import uuid4

import numpy as np
from google import genai
from google.genai import types
from qdrant_client import QdrantClient
from qdrant_client.http import models

# Config
GEMINI_API_KEY = os.getenv("GEMINI_API_KEY", "")
QDRANT_URL = os.getenv("QDRANT_URL", "http://localhost:6333")
COLLECTION = "multimodal"

client_gemini = genai.Client(api_key=GEMINI_API_KEY)
client_qdrant = QdrantClient(url=QDRANT_URL)
EMBED_MODEL = "gemini-embedding-2-preview"
DESC_MODEL = "gemini-3-flash-preview"

CHUNK_LIMITS = {"video": 120, "audio": 75, "pdf": 5, "image": 1, "text": 6000}
MIME_MAP = {
    ".mp4": "video/mp4", ".mov": "video/quicktime", ".m4a": "audio/mp4",
    ".mp3": "audio/mp3", ".wav": "audio/wav", ".pdf": "application/pdf",
    ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".webm": "video/webm", ".webp": "image/webp", ".txt": "text/plain", ".md": "text/plain"
}


def get_file_type(path):
    ext = Path(path).suffix.lower()
    if ext in [".mp4", ".mov", ".webm"]:
        return "video"
    if ext in [".m4a", ".mp3", ".wav"]:
        return "audio"
    if ext == ".pdf":
        return "pdf"
    if ext in [".png", ".jpg", ".jpeg", ".webp"]:
        return "image"
    return "text"


def chunk_file(path, file_type):
    """Split file into chunks using ffmpeg, return list of (bytes, mime, info)."""
    if file_type == "text":
        text = Path(path).read_text()
        return [(text.encode(), "text/plain", f"chunk_0_{len(text)}chars")]

    if file_type == "image":
        mime = MIME_MAP.get(Path(path).suffix.lower(), "image/png")
        return [(Path(path).read_bytes(), mime, "image_single")]

    if file_type == "audio":
        from pydub import AudioSegment
        audio = AudioSegment.from_file(path)
        chunks = []
        for i in range(0, len(audio), 75000):
            ch = audio[i:i+75000]
            p = f"/tmp/audio_chunk_{i//75000}.wav"
            ch.export(p, format="wav")
            chunks.append((Path(p).read_bytes(), "audio/wav", f"{i//75000}"))
        return chunks

    if file_type == "video":
        result = subprocess.run(
            f'ffprobe -v quiet -show_entries format=duration -of csv=p=0 "{str(path)}"',
            capture_output=True, text=True, shell=True
        )
        duration_str = result.stdout.strip()
        if not duration_str:
            print(f"    ffprobe failed: {result.stderr[-200:]}")
            return []
        total = float(duration_str)
        chunk_secs = 30  # 30s chunks for Gemini compatibility
        chunks = []
        for i in range(0, int(total), chunk_secs):
            end = min(i + chunk_secs, total)
            p = f"/tmp/video_chunk_{i//chunk_secs}.mp4"
            rc = subprocess.run([
                "ffmpeg", "-y", "-i", str(path), "-ss", str(i), "-t", str(end - i),
                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "28",
                "-c:a", "aac", "-b:a", "64k", p
            ], capture_output=True)
            if rc.returncode == 0:
                chunks.append((Path(p).read_bytes(), "video/mp4", f"{i}s-{int(end)}s"))
        return chunks

    if file_type == "pdf":
        import fitz
        doc = fitz.open(path)
        chunks = []
        for i in range(0, len(doc), 5):
            page_slice = doc[i:i+5]
            tmp = f"/tmp/pdf_chunk_{i//5}.pdf"
            page_slice.save(tmp)
            chunks.append((Path(tmp).read_bytes(), "application/pdf", f"pages_{i}-{min(i+5,len(doc))}"))
        return chunks

    return [(Path(path).read_bytes(), "application/octet-stream", "unknown")]


def embed_and_describe(file_bytes, mime, file_type):
    """Return (description, embedding_vector)."""
    # Build the part — text must use .text field, binary uses .from_bytes
    if mime == "text/plain":
        text_content = file_bytes.decode("utf-8", errors="replace")
        part = types.Part(text=text_content)
    else:
        part = types.Part.from_bytes(data=file_bytes, mime_type=mime)

    # Get description using Gemini Flash
    prompt = {
        "video": "Describe this video briefly: speakers, topics, key points. Be concise.",
        "audio": "Transcribe and briefly describe this audio.",
        "image": "Describe this image concisely.",
        "pdf": "Summarize these PDF pages concisely.",
        "text": "Summarize this text briefly.",
    }.get(file_type, "Describe this content concisely.")

    resp = client_gemini.models.generate_content(
        model=DESC_MODEL,
        contents=[types.Content(role="user", parts=[types.Part(text=prompt), part])]
    )
    desc = resp.text.strip()

    # Embed using Gemini 2
    emb = client_gemini.models.embed_content(
        model=EMBED_MODEL,
        contents=part,
        config=types.EmbedContentConfig(task_type="RETRIEVAL_DOCUMENT"),
    )
    vec = emb.embeddings[0].values
    norm = np.linalg.norm(vec)
    vec = [v / norm for v in vec]

    return desc, vec


def store_qdrant(doc_id, title, file_type, filename, chunk_idx, total_chunks, desc, metadata, vec):
    """Store in Qdrant with payload."""
    point_id = str(uuid4())
    client_qdrant.upsert(
        collection_name=COLLECTION,
        points=[
            models.PointStruct(
                id=point_id,
                vector=vec,
                payload={
                    "doc_id": doc_id,
                    "title": title,
                    "content_type": file_type,
                    "filename": filename,
                    "chunk_index": chunk_idx,
                    "chunk_total": total_chunks,
                    "text_content": desc,
                    "metadata": metadata,
                }
            )
        ]
    )
    return point_id


def ingest(path, title=None):
    path = Path(path)
    file_type = get_file_type(str(path))
    mime = MIME_MAP.get(path.suffix.lower(), "application/octet-stream")
    chunks = chunk_file(str(path), file_type)
    total = len(chunks)
    title = title or f"{path.name} ({file_type})"

    print(f"Ingesting: {path.name} ({file_type}, {total} chunks)")

    for idx, (bytes_data, mime, info) in enumerate(chunks):
        print(f"  Chunk {idx+1}/{total}: {info}")
        try:
            desc, vec = embed_and_describe(bytes_data, mime, file_type)
        except Exception as e:
            print(f"    Embed failed: {e}")
            continue

        doc_id = f"{path.stem[:30]}_{idx:03d}"
        metadata = {"source": str(path), "file_type": file_type, "chunk_info": info}
        try:
            point_id = store_qdrant(doc_id, title, file_type, path.name, idx, total, desc, metadata, vec)
            print(f"    -> '{desc[:80]}...' [{point_id[:8]}...]")
        except Exception as e:
            print(f"    Store failed: {e}")

    print(f"Done: {total} chunks processed.")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: ingest.py <file_path> [title]")
        sys.exit(1)
    title = sys.argv[2] if len(sys.argv) > 2 else None
    ingest(sys.argv[1], title=title)
