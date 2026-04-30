"""
Media utilities for HarveyChat — images, documents, PDFs.

Image handling:
  Telegram photo → download → base64 encode → MiniMax vision API → description

Document handling:
  Telegram document → download → ffmpeg/textract → plain text → feed to LLM
"""

import base64
import logging
import os
import re
import tempfile
from pathlib import Path
from typing import Optional

import requests

log = logging.getLogger("harveychat.media")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


def _get_switchai_config() -> tuple[str, str]:
    switchai_url = os.environ.get("SWITCHAI_URL", "http://localhost:18080/v1")
    switchai_key = os.environ.get("SWITCHAI_KEY", "sk-test-123")
    return switchai_url, switchai_key


async def download_telegram_file(file_id: str, bot_token: str) -> Optional[str]:
    """Download any Telegram file and return local path."""
    import httpx

    try:
        async with httpx.AsyncClient(timeout=60.0) as client:
            resp = await client.get(
                f"https://api.telegram.org/bot{bot_token}/getFile",
                params={"file_id": file_id},
            )
            if resp.status_code != 200:
                return None

            data = resp.json()
            file_path = data.get("result", {}).get("file_path")
            if not file_path:
                return None

            file_url = f"https://api.telegram.org/file/bot{bot_token}/{file_path}"
            resp = await client.get(file_url)
            if resp.status_code != 200 or not resp.content:
                return None

            # Save to temp
            ext = Path(file_path).suffix or ""
            tmp_path = Path(tempfile.gettempdir()) / f"harvey_{file_id}{ext}"
            tmp_path.write_bytes(resp.content)
            return str(tmp_path)

    except Exception as e:
        log.error(f"File download failed: {e}")
        return None


def describe_image(image_path: str) -> str:
    """
    Describe an image using Makakoo omni first, then legacy VL fallbacks.
    """
    if not os.path.exists(image_path):
        return ""

    try:
        from core.llm.omni import describe_image as omni_describe_image

        content = omni_describe_image(
            image_path,
            "Describe this image in detail. Extract visible text and explain what the user likely wants me to notice.",
            max_completion_tokens=700,
        ).strip()
        if content:
            log.info(f"Omni image described: {len(content)} chars")
            return content
    except Exception as e:
        log.warning(f"Omni image description failed: {e}; trying legacy VL fallback")

    switchai_url, switchai_key = _get_switchai_config()

    with open(image_path, "rb") as f:
        img_data = base64.b64encode(f.read()).decode()

    mime_type = _guess_mime_type(image_path)

    # Try MiniMax image-01 model
    payload = {
        "model": "minimax:image-01",
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "Describe this image in detail. Be specific about objects, text, setting, and any notable features.",
                    },
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:{mime_type};base64,{img_data}"},
                    },
                ],
            }
        ],
        "max_tokens": 500,
    }

    headers = {
        "Authorization": f"Bearer {switchai_key}",
        "Content-Type": "application/json",
    }

    try:
        r = requests.post(
            f"{switchai_url}/chat/completions",
            headers=headers,
            json=payload,
            timeout=60,
        )

        if r.status_code == 200:
            data = r.json()
            content = data["choices"][0]["message"]["content"]
            log.info(f"Image described: {len(content)} chars")
            return content

        log.warning(f"Image-01 failed: {r.status_code} {r.text[:100]}")

    except Exception as e:
        log.warning(f"Image-01 error: {e}")

    # Fallback: try Qwen VL
    return _describe_with_qwen_vl(image_path, switchai_url, switchai_key)


def _describe_with_qwen_vl(
    image_path: str, switchai_url: str, switchai_key: str
) -> str:
    """Fallback: use Qwen VL model for image description."""
    with open(image_path, "rb") as f:
        img_data = base64.b64encode(f.read()).decode()

    mime_type = _guess_mime_type(image_path)

    for model in [
        "alibaba/qwen-vl-plus-2025-05-07",
        "qwen-vl-flash",
        "pixtral-12b-2409",
    ]:
        payload = {
            "model": model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Describe this image briefly."},
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:{mime_type};base64,{img_data}"},
                        },
                    ],
                }
            ],
            "max_tokens": 300,
        }

        headers = {
            "Authorization": f"Bearer {switchai_key}",
            "Content-Type": "application/json",
        }

        try:
            r = requests.post(
                f"{switchai_url}/chat/completions",
                headers=headers,
                json=payload,
                timeout=60,
            )
            if r.status_code == 200:
                data = r.json()
                content = data["choices"][0]["message"]["content"]
                log.info(f"Qwen VL image described: {model}")
                return content

        except Exception:
            continue

    log.error("All VL models failed for image description")
    return ""


def extract_text_from_file(file_path: str, mime_type: str = "") -> str:
    """
    Extract plain text from a file (PDF, text, etc.) using ffmpeg or text extraction.
    Returns empty string if extraction fails.
    """
    if not os.path.exists(file_path):
        return ""

    ext = Path(file_path).suffix.lower()
    mime_type = (mime_type or "").lower()

    # Plain text files
    if ext in (
        ".txt",
        ".md",
        ".py",
        ".js",
        ".ts",
        ".json",
        ".yaml",
        ".yml",
        ".xml",
        ".html",
        ".css",
        ".csv",
        ".log",
    ):
        try:
            text = Path(file_path).read_text(errors="replace")
            log.info(f"Extracted text from {ext}: {len(text)} chars")
            return text
        except Exception as e:
            log.warning(f"Text file read failed: {e}")
            return ""

    # PDF files
    if ext == ".pdf" or "pdf" in mime_type:
        return _extract_pdf(file_path)

    # Images (OCR)
    if ext in (".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"):
        return _ocr_image(file_path)

    # Word docs
    if ext in (".doc", ".docx") or "word" in mime_type:
        return _extract_docx(file_path)

    # Unknown — try as binary text
    try:
        text = Path(file_path).read_text(errors="replace")
        # Strip binary garbage
        text = re.sub(r"[\x00-\x08\x0e-\x1f]", " ", text)
        text = re.sub(r" {3,}", " ", text)
        log.info(f"Binary-as-text: {len(text)} chars from {ext}")
        return text.strip()
    except Exception:
        return ""


def _extract_pdf(pdf_path: str) -> str:
    """Extract text from PDF using pdftotext (poppler) or PyPDF2."""
    import subprocess

    # Try pdftotext (fast, accurate)
    try:
        result = subprocess.run(
            ["pdftotext", "-layout", pdf_path, "-"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0 and result.stdout.strip():
            text = result.stdout.strip()
            log.info(f"PDF extracted via pdftotext: {len(text)} chars")
            return text
    except FileNotFoundError:
        pass
    except Exception as e:
        log.warning(f"pdftotext failed: {e}")

    # Fallback: PyPDF2
    try:
        import PyPDF2

        with open(pdf_path, "rb") as f:
            reader = PyPDF2.PdfReader(f)
            parts = []
            for page in reader.pages:
                text = page.extract_text()
                if text:
                    parts.append(text)
            result = "\n\n".join(parts)
            log.info(f"PDF extracted via PyPDF2: {len(result)} chars")
            return result
    except ImportError:
        log.warning("PyPDF2 not installed — cannot extract PDF text")
    except Exception as e:
        log.warning(f"PyPDF2 failed: {e}")

    return ""


def _ocr_image(image_path: str) -> str:
    """Extract text from image using Tesseract OCR."""
    import subprocess

    try:
        result = subprocess.run(
            ["tesseract", image_path, "stdout", "--psm", "3", "quiet"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode == 0:
            text = result.stdout.strip()
            log.info(f"OCR extracted: {len(text)} chars")
            return text
    except FileNotFoundError:
        log.warning("tesseract not installed — cannot OCR images")
    except Exception as e:
        log.warning(f"tesseract failed: {e}")

    return ""


def _extract_docx(docx_path: str) -> str:
    """Extract text from .docx using python-docx."""
    try:
        from docx import Document

        doc = Document(docx_path)
        parts = []
        for para in doc.paragraphs:
            if para.text.strip():
                parts.append(para.text)
        result = "\n".join(parts)
        log.info(f"DOCX extracted: {len(result)} chars")
        return result
    except ImportError:
        log.warning("python-docx not installed — cannot extract DOCX")
    except Exception as e:
        log.warning(f"docx failed: {e}")
    return ""


def _guess_mime_type(file_path: str) -> str:
    """Guess MIME type from file extension."""
    ext = Path(file_path).suffix.lower()
    types = {
        ".jpg": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".png": "image/png",
        ".gif": "image/gif",
        ".webp": "image/webp",
        ".bmp": "image/bmp",
        ".pdf": "application/pdf",
        ".txt": "text/plain",
    }
    return types.get(ext, "application/octet-stream")


def describe_video(video_path: str) -> str:
    """Describe a video using Makakoo omni. Returns empty string on failure."""
    if not os.path.exists(video_path):
        return ""
    try:
        from core.llm.omni import describe_video as omni_describe_video

        content = omni_describe_video(
            video_path,
            "Watch this Telegram video. Summarize the visible action and extract any readable text.",
            fps=2,
            media_resolution="default",
            max_completion_tokens=900,
        ).strip()
        if content:
            log.info(f"Omni video described: {len(content)} chars")
            return content
    except Exception as e:
        log.error(f"Video description failed: {e}")
    return ""
