"""
Audio utilities for HarveyChat — transcribe voice input, generate voice output.

Audio INPUT:
  Telegram voice → download → ffmpeg convert → faster-whisper (local, no API cost)

Audio OUTPUT:
  Text → macOS 'say' command → AIFF/MP3 → send to Telegram
  (Fallback when TTS API unavailable)
"""

import asyncio
import base64
import logging
import os
import subprocess
import tempfile
from pathlib import Path
from typing import Optional

log = logging.getLogger("harveychat.audio")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


def transcribe_audio(audio_path: str) -> str:
    """
    Transcribe an audio file using faster-whisper (local, no API needed).
    """
    if not os.path.exists(audio_path):
        return ""

    try:
        from faster_whisper import WhisperModel

        model = WhisperModel("base", device="cpu", compute_type="int8")
        segments, _ = model.transcribe(audio_path, language="en", beam_size=5)
        text = " ".join(seg.text for seg in segments).strip()
        log.info(f"Whisper transcription: {len(text)} chars from {audio_path}")
        return text
    except Exception as e:
        log.error(f"Whisper transcription failed: {e}")
        return ""


async def speak_text(text: str, user_id: str) -> Optional[str]:
    """
    Generate audio from text using macOS 'say' command.

    Returns path to generated audio file (MP3), or None if failed.
    """
    clean_text = text.strip()
    if len(clean_text) > 400:
        clean_text = clean_text[:397] + "..."

    if not clean_text:
        return None

    voice = os.environ.get(
        "HARVEY_TTS_VOICE", "Samantha"
    )  # macOS comes with many voices
    out_path = Path(HARVEY_HOME) / "data" / "chat" / "voice"
    out_path.mkdir(parents=True, exist_ok=True)

    aiff_path = (
        out_path / f"say_{user_id}_{int(asyncio.get_event_loop().time() * 1000)}.aiff"
    )
    mp3_path = aiff_path.with_suffix(".mp3")

    try:
        # Generate AIFF with macOS say
        proc = await asyncio.create_subprocess_exec(
            "say",
            "-o",
            str(aiff_path),
            "-v",
            voice,
            clean_text,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=30)

        if proc.returncode != 0:
            log.warning(f"say command failed: {stderr.decode()[:200]}")
            return None

        if not aiff_path.exists():
            log.warning(f"say produced no output file")
            return None

        # Convert AIFF → MP3 with ffmpeg
        proc2 = await asyncio.create_subprocess_exec(
            "ffmpeg",
            "-y",
            "-i",
            str(aiff_path),
            "-b:a",
            "128k",
            "-ar",
            "24000",
            str(mp3_path),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout2, stderr2 = await asyncio.wait_for(proc2.communicate(), timeout=30)

        # Clean up AIFF
        aiff_path.unlink(missing_ok=True)

        if proc2.returncode == 0 and mp3_path.exists():
            log.info(f"TTS generated: {mp3_path} ({mp3_path.stat().st_size} bytes)")
            return str(mp3_path)

        log.warning(f"ffmpeg failed: {stderr2.decode()[:200]}")
        return None

    except asyncio.TimeoutError:
        log.error("say/ffmpeg timed out")
        return None
    except Exception as e:
        log.error(f"TTS generation failed: {e}")
        return None


async def download_telegram_voice(file_id: str, bot_token: str) -> Optional[str]:
    """
    Download a Telegram voice message and convert to WAV (for Whisper).

    Returns path to the WAV audio file, or None if failed.
    Telegram voice notes are .oga (opus) — Whisper needs WAV/MP3.
    """
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

            # Save original .oga
            tmp_dir = Path(tempfile.gettempdir())
            oga_path = tmp_dir / f"voice_{file_id}.oga"
            oga_path.write_bytes(resp.content)

            # Convert to WAV (Whisper prefers WAV)
            wav_path = tmp_dir / f"voice_{file_id}.wav"
            proc = await asyncio.create_subprocess_exec(
                "ffmpeg",
                "-y",
                "-i",
                str(oga_path),
                "-ar",
                "16000",
                "-ac",
                "1",
                str(wav_path),
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=30)

            # Clean up .oga
            oga_path.unlink(missing_ok=True)

            if proc.returncode == 0 and wav_path.exists():
                log.info(f"Voice downloaded & converted: {wav_path}")
                return str(wav_path)

            log.warning(f"ffmpeg voice conversion failed: {stderr.decode()[:200]}")
            return None

    except Exception as e:
        log.error(f"Voice download failed: {e}")
        return None


def voice_note_available() -> bool:
    """Check if we can process voice notes."""
    try:
        from faster_whisper import WhisperModel
        import shutil

        return shutil.which("ffmpeg") is not None and shutil.which("say") is not None
    except Exception:
        return False
