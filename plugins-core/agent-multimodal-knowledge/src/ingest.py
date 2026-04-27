#!/usr/bin/env python3
"""
Multimodal Knowledge Ingest — Gemini Embedding 2 + Qdrant

Supports: PDF, Images (PNG/JPEG/WebP), Video (MP4), Audio (M4A/MP3/WAV), Text.
Accepts local paths OR http(s) URLs (YouTube routed through yt-dlp).

CLI:
    ingest.py --source <url-or-path> [--kind video|audio|pdf|image|text]
              [--title <t>] [--note <n>] [--json]

    ingest.py <path> [title]              # legacy positional form (preserved)

With --json, stdout emits one JSON line; human progress goes to stderr.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Optional
from urllib.parse import urlparse
from uuid import uuid4

import numpy as np
from google import genai
from google.genai import types
from qdrant_client import QdrantClient
from qdrant_client.http import models

# Config — clients built lazily so `--help` / dry-inspection doesn't require keys.
QDRANT_URL = os.getenv("QDRANT_URL", "http://localhost:6333")
COLLECTION = "multimodal"
EMBED_MODEL = "gemini-embedding-2-preview"
DESC_MODEL = "gemini-3-flash-preview"

CHUNK_LIMITS = {"video": 120, "audio": 75, "pdf": 5, "image": 1, "text": 6000}
MIME_MAP = {
    ".mp4": "video/mp4", ".mov": "video/quicktime", ".m4a": "audio/mp4",
    ".mp3": "audio/mp3", ".wav": "audio/wav", ".pdf": "application/pdf",
    ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".webm": "video/webm", ".webp": "image/webp", ".txt": "text/plain", ".md": "text/plain"
}

_client_gemini = None
_client_qdrant = None


def _gemini():
    global _client_gemini
    if _client_gemini is None:
        key = os.getenv("GEMINI_API_KEY")
        if not key:
            raise RuntimeError("GEMINI_API_KEY environment variable required")
        _client_gemini = genai.Client(api_key=key)
    return _client_gemini


def _qdrant():
    global _client_qdrant
    if _client_qdrant is None:
        _client_qdrant = QdrantClient(url=QDRANT_URL)
    return _client_qdrant


# ─────────────────────────────────────────────────────────────────────
# Source resolution — URL or local path → local file
# ─────────────────────────────────────────────────────────────────────

def _is_url(s: str) -> bool:
    try:
        u = urlparse(s)
        return u.scheme in ("http", "https") and bool(u.netloc)
    except Exception:
        return False


def _is_youtube(url: str) -> bool:
    host = urlparse(url).netloc.lower()
    return host.endswith("youtube.com") or host.endswith("youtu.be")


def _download_youtube(url: str, outdir: Path, log) -> Path:
    """yt-dlp → capped-quality mp4. Requires yt-dlp on PATH."""
    if shutil.which("yt-dlp") is None:
        raise RuntimeError("yt-dlp not found on PATH (needed for YouTube URLs)")
    outtpl = str(outdir / "yt_%(id)s.%(ext)s")
    cmd = [
        "yt-dlp",
        "-f", "bv*[height<=720][ext=mp4]+ba[ext=m4a]/b[height<=720][ext=mp4]/b[ext=mp4]",
        "--merge-output-format", "mp4",
        "-o", outtpl,
        "--no-playlist",
        "--quiet", "--no-warnings",
        url,
    ]
    log(f"yt-dlp: downloading {url}")
    rc = subprocess.run(cmd, capture_output=True, text=True)
    if rc.returncode != 0:
        raise RuntimeError(f"yt-dlp failed: {rc.stderr.strip()[-300:]}")
    files = sorted(outdir.glob("yt_*.mp4"))
    if not files:
        raise RuntimeError("yt-dlp reported success but no mp4 produced")
    return files[-1]


def _download_http(url: str, outdir: Path, log) -> Path:
    """Generic http(s) download. Uses requests if available, urllib otherwise."""
    log(f"http: downloading {url}")
    try:
        import requests
        r = requests.get(url, timeout=60, stream=True)
        r.raise_for_status()
        ext = Path(urlparse(url).path).suffix or _ext_from_content_type(
            r.headers.get("content-type", "")
        )
        dest = outdir / f"dl_{uuid4().hex[:10]}{ext}"
        with open(dest, "wb") as f:
            for chunk in r.iter_content(chunk_size=1 << 15):
                if chunk:
                    f.write(chunk)
        return dest
    except ImportError:
        from urllib.request import urlopen
        with urlopen(url, timeout=60) as r:
            ct = r.headers.get("content-type", "")
            ext = Path(urlparse(url).path).suffix or _ext_from_content_type(ct)
            dest = outdir / f"dl_{uuid4().hex[:10]}{ext}"
            with open(dest, "wb") as f:
                shutil.copyfileobj(r, f)
        return dest


def _ext_from_content_type(ct: str) -> str:
    ct = (ct or "").split(";")[0].strip().lower()
    return {
        "video/mp4": ".mp4", "video/webm": ".webm",
        "audio/mp4": ".m4a", "audio/mpeg": ".mp3", "audio/wav": ".wav",
        "application/pdf": ".pdf",
        "image/png": ".png", "image/jpeg": ".jpg", "image/webp": ".webp",
        "text/plain": ".txt", "text/markdown": ".md",
    }.get(ct, "")


def resolve_source(source: str, workdir: Path, log) -> Path:
    """Return a local Path for `source`. Downloads URLs into `workdir`."""
    if _is_url(source):
        if _is_youtube(source):
            return _download_youtube(source, workdir, log)
        return _download_http(source, workdir, log)
    p = Path(os.path.expanduser(source))
    if not p.exists():
        raise RuntimeError(f"local path does not exist: {p}")
    return p


# ─────────────────────────────────────────────────────────────────────
# File-type detection + chunking
# ─────────────────────────────────────────────────────────────────────

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


def chunk_file(path, file_type, log):
    """Split file into chunks, return list of (bytes, mime, info)."""
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
            log(f"ffprobe failed: {result.stderr[-200:]}")
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


# ─────────────────────────────────────────────────────────────────────
# Embedding + storage (with 429 backoff)
# ─────────────────────────────────────────────────────────────────────

def _with_backoff(fn, *, attempts=3, base_delay=2.0):
    last = None
    for i in range(attempts):
        try:
            return fn()
        except Exception as e:
            last = e
            msg = str(e).lower()
            retriable = "429" in msg or "rate" in msg or "resource exhausted" in msg
            if not retriable or i == attempts - 1:
                raise
            time.sleep(base_delay * (2 ** i))
    raise last  # pragma: no cover


def embed_and_describe(file_bytes, mime, file_type):
    """Return (description, embedding_vector)."""
    if mime == "text/plain":
        text_content = file_bytes.decode("utf-8", errors="replace")
        part = types.Part(text=text_content)
    else:
        part = types.Part.from_bytes(data=file_bytes, mime_type=mime)

    prompt = {
        "video": "Describe this video briefly: speakers, topics, key points. Be concise.",
        "audio": "Transcribe and briefly describe this audio.",
        "image": "Describe this image concisely.",
        "pdf": "Summarize these PDF pages concisely.",
        "text": "Summarize this text briefly.",
    }.get(file_type, "Describe this content concisely.")

    def _describe():
        return _gemini().models.generate_content(
            model=DESC_MODEL,
            contents=[types.Content(role="user", parts=[types.Part(text=prompt), part])]
        )

    resp = _with_backoff(_describe)
    desc = resp.text.strip()

    def _embed():
        return _gemini().models.embed_content(
            model=EMBED_MODEL,
            contents=part,
            config=types.EmbedContentConfig(task_type="RETRIEVAL_DOCUMENT"),
        )

    emb = _with_backoff(_embed)
    vec = emb.embeddings[0].values
    norm = np.linalg.norm(vec)
    vec = [v / norm for v in vec]

    return desc, vec


def store_qdrant(doc_id, title, file_type, filename, chunk_idx, total_chunks, desc, metadata, vec):
    point_id = str(uuid4())
    _qdrant().upsert(
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


# ─────────────────────────────────────────────────────────────────────
# Public entrypoint
# ─────────────────────────────────────────────────────────────────────

def ingest(
    source: str,
    *,
    title: Optional[str] = None,
    kind: Optional[str] = None,
    note: Optional[str] = None,
    log=None,
) -> dict:
    """
    Ingest media at `source` (URL or local path) into the multimodal Qdrant collection.

    Returns a dict: {ingested, doc_ids, summary, errors, source, resolved_path, chunks, file_type}.
    Raises RuntimeError on unrecoverable setup errors (missing binaries, bad URL, missing key).
    """
    log = log or (lambda *_a, **_kw: None)
    errors: list[str] = []
    doc_ids: list[str] = []
    workdir = Path(tempfile.mkdtemp(prefix="harvey_ingest_"))
    try:
        local = resolve_source(source, workdir, log)
        file_type = kind or get_file_type(str(local))
        chunks = chunk_file(str(local), file_type, log)
        total = len(chunks)
        display_title = title or f"{local.name} ({file_type})"
        log(f"Ingesting: {local.name} ({file_type}, {total} chunks)")

        for idx, (bytes_data, mime, info) in enumerate(chunks):
            log(f"  Chunk {idx+1}/{total}: {info}")
            try:
                desc, vec = embed_and_describe(bytes_data, mime, file_type)
            except Exception as e:
                errors.append(f"chunk {idx}: embed failed: {e}")
                log(f"    Embed failed: {e}")
                continue

            doc_id = f"{local.stem[:30]}_{idx:03d}"
            meta = {"source": str(local), "file_type": file_type, "chunk_info": info}
            if _is_url(source):
                meta["origin_url"] = source
            if note:
                meta["note"] = note
            try:
                point_id = store_qdrant(doc_id, display_title, file_type, local.name, idx, total, desc, meta, vec)
                doc_ids.append(doc_id)
                log(f"    -> '{desc[:80]}...' [{point_id[:8]}...]")
            except Exception as e:
                errors.append(f"chunk {idx}: store failed: {e}")
                log(f"    Store failed: {e}")

        log(f"Done: {len(doc_ids)}/{total} chunks stored.")
        return {
            "ingested": len(doc_ids) > 0,
            "doc_ids": doc_ids,
            "summary": f"{len(doc_ids)}/{total} chunks stored from {local.name}",
            "errors": errors,
            "source": source,
            "resolved_path": str(local),
            "chunks": total,
            "file_type": file_type,
        }
    finally:
        # Keep downloads around only if caller wants debugging; default wipe.
        shutil.rmtree(workdir, ignore_errors=True)


def _cli():
    ap = argparse.ArgumentParser(
        prog="ingest.py",
        description="Ingest media (local path or URL) into the multimodal Qdrant collection.",
    )
    ap.add_argument("--source", help="URL or local path. Preferred over positional.")
    ap.add_argument("--kind", choices=sorted(CHUNK_LIMITS.keys()),
                    help="Override auto-detected file type.")
    ap.add_argument("--title", help="Display title.")
    ap.add_argument("--note", help="Free-form metadata note.")
    ap.add_argument("--json", dest="emit_json", action="store_true",
                    help="Emit one JSON line on stdout; progress to stderr.")
    ap.add_argument("positional", nargs="*",
                    help="Legacy: <path> [title]. Used only if --source is absent.")
    args = ap.parse_args()

    source = args.source
    title = args.title
    if not source:
        if not args.positional:
            ap.error("provide --source <url-or-path> or a positional <path>")
        source = args.positional[0]
        if title is None and len(args.positional) > 1:
            title = args.positional[1]

    if args.emit_json:
        # Human progress → stderr; only JSON on stdout.
        def log(msg): print(msg, file=sys.stderr, flush=True)
    else:
        def log(msg): print(msg, flush=True)

    try:
        result = ingest(source, title=title, kind=args.kind, note=args.note, log=log)
    except Exception as e:
        payload = {
            "ingested": False, "doc_ids": [], "chunks": 0,
            "file_type": None, "summary": f"fatal: {e}",
            "errors": [str(e)], "source": source, "resolved_path": None,
        }
        if args.emit_json:
            print(json.dumps(payload), flush=True)
        else:
            print(f"ERROR: {e}", file=sys.stderr)
        sys.exit(1)

    if args.emit_json:
        print(json.dumps(result), flush=True)
    sys.exit(0 if result["ingested"] else 1)


if __name__ == "__main__":
    _cli()
