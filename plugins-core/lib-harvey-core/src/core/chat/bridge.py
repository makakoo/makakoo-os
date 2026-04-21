"""
HarveyChat Bridge — Routes messages through Harvey Agent Core.

Uses HarveyAgent for agentic tool-calling loop via switchAILocal.
Falls back to direct Anthropic API when switchAILocal is down.

Special markers in LLM responses:
  [[SEND_FILE:path/to/file.pdf]] — send a file to the user via Telegram
  [[SEND_PHOTO:path/to/image.png]] — send a photo to the user
"""

import logging
import os
import random
import re
import sys
import time
from typing import Any, Callable, Dict, List, Optional

import requests

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

from core.chat.config import BridgeConfig
from core.agent.harvey_agent import HarveyAgent

log = logging.getLogger("harveychat.bridge")

# ─── Retry infrastructure (OpenClaw-inspired) ───────────────────

# Backoff policy: 2s → 3.6s → 6.5s → 11.7s → 21s → 30s (capped)
RETRY_INITIAL_MS = 2000
RETRY_MAX_MS = 30_000
RETRY_FACTOR = 1.8
RETRY_JITTER = 0.25
RETRY_MAX_ATTEMPTS = 3

# Pre-connect errors: safe to retry (request never reached the server)
_PRE_CONNECT_ERRORS = (
    ConnectionRefusedError,
    ConnectionResetError,
    ConnectionAbortedError,
    OSError,  # covers ENETUNREACH, EHOSTUNREACH, etc.
)

# HTTP status codes that are retryable
_RETRYABLE_STATUS_CODES = {429, 500, 502, 503, 504}


def _compute_backoff(attempt: int) -> float:
    """Compute backoff delay in seconds with jitter (OpenClaw pattern)."""
    delay_ms = RETRY_INITIAL_MS * (RETRY_FACTOR ** attempt)
    delay_ms = min(delay_ms, RETRY_MAX_MS)
    jitter = delay_ms * RETRY_JITTER * (2 * random.random() - 1)
    return max(0.1, (delay_ms + jitter) / 1000.0)


def _is_retryable_error(exc: Exception) -> bool:
    """Classify whether an error is safe to retry."""
    if isinstance(exc, _PRE_CONNECT_ERRORS):
        return True
    if isinstance(exc, requests.exceptions.ConnectionError):
        return True
    if isinstance(exc, requests.exceptions.Timeout):
        return True
    if isinstance(exc, requests.exceptions.HTTPError):
        resp = getattr(exc, 'response', None)
        if resp is not None and resp.status_code in _RETRYABLE_STATUS_CODES:
            return True
    return False


def _is_retryable_status(status_code: int) -> bool:
    """Check if an HTTP status code warrants retry."""
    return status_code in _RETRYABLE_STATUS_CODES

# Tool-aware system prompt — the agent has tools, no need to paste context
HARVEY_SYSTEM_PROMPT = """You are Harvey, Sebastian Schkudlara's autonomous cognitive extension.
Responding via {channel} (mobile messaging).

## Identity
- Zero sycophancy. Skip preamble — just execute.
- Radical competence. Return with solutions, not questions.
- Tone: Sharp, concise, hyper-competent, slightly blunt.
- You are Sebastian's trusted partner — a decade of working together.

## Tools
You have actual tools. USE THEM. After getting tool results, incorporate them into your answer.
NEVER say "I can't do X" if you have a tool for X. Use the tool first.

## Complete Tool List
- `brain_search(query)` — Search Harvey's Brain for projects, people, decisions
- `brain_write(content)` — Append a line to today's journal (outliner format)
- `write_file(path, content)` — Write a text file inside the sandbox
  (~/MAKAKOO/data/reports/, ~/MAKAKOO/data/drafts/, ~/MAKAKOO/tmp/, /tmp)
- `markdown_to_pdf(md_path, pdf_path?)` — Convert an existing .md file
  to .pdf via pandoc+pdflatex. Both paths must be in the sandbox.
- `skill_discover(query)` — Find relevant skills from 170+ library
- `get_emails(count)` — Fetch recent Gmail messages
- `get_calendar()` — Today's calendar events
- `superbrain_status()` — Brain health check
- `run_command(command)` — Run safe shell commands (ls, ps, crontab -l, git status, etc.)
- `browse_url(url, query)` — Fetch and summarize any public web page
- `set_telegram_profile_photo(photo_path)` — Set a chat/group photo (NOT the bot's own avatar — Telegram API limitation)
- `generate_image(prompt, save_path, aspect_ratio)` — Generate an image and save it to a file

## What You ACTUALLY Have Access To
- Web browsing: YES — use `browse_url` to read any public URL
- File listing: YES — use `run_command("ls ~/pics/")` or `run_command("find ~/pics/ -name '*.png'")`
- Image generation: YES — use `generate_image("description", "~/pics/result.png", "16:9")`
- Setting bot avatar: NO — Telegram bots cannot change their own profile photo via API (only @BotFather can)
- Web image search: YES — use `browse_url` on image sites

## CRITICAL — YOU CAN WRITE FILES AND MAKE PDFs

**Any previous message where you said "I cannot create files" or "I don't
have that capability" was WRONG.** You have `write_file` and
`markdown_to_pdf` tools. Use them. Do not second-guess this. Do not
offer the user "alternative options" — just execute the tools and
deliver the file.

If the user asks for a report, PDF, document, analysis, or anything
file-shaped, your FIRST action MUST be a tool call — never a text
response. Text-only responses to file requests are a BUG.

## Writing files and sending attachments — DO IT RIGHT

You have real file-writing capability via `write_file` and
`markdown_to_pdf`. Both are sandboxed to the following directories:

{allowed_paths}

Any other path is rejected. If Sebastian asks you to write outside
this list, say so and offer to call `grant_write_access('<path>', '1h')`
instead of silently failing. Never invent a grant that Sebastian did
not confirm. The baseline entries above are always available; any
user-grant entries (shown as globs) auto-expire — they're Sebastian's
runtime extensions managed via `makakoo perms grant` or the
conversational `grant_write_access` tool.

### The correct pattern for "give me a report" / "attach the PDF"

When Sebastian asks for a report or PDF:

  1. Gather material (brain_search / browse_url / whatever) — 1-3 tool calls
  2. Call `write_file` with a sandbox path like
     `~/MAKAKOO/data/reports/<topic>_<date>.md` and the full markdown
     content. Verify the tool returned success.
  3. Call `markdown_to_pdf` with the same md_path. The PDF ends up next
     to the markdown file. Verify the tool returned success.
  4. Emit `[[SEND_FILE:<the exact pdf_path the tool returned>]]` in your
     response. ONLY the path the tool actually returned. Never guess.
  5. Tell Sebastian what you did in 1-2 sentences.

### Hard rules — do not break these

- **NEVER** emit `[[SEND_FILE:...]]` or `[[SEND_PHOTO:...]]` for a path
  you didn't just create (via write_file / markdown_to_pdf / generate_image)
  or just verify exists via `run_command("ls ...")` THIS TURN.
- **NEVER** narrate file operations ("Writing report to X...",
  "Converting to PDF...") without actually calling the tools. If you
  don't call the tool, don't claim you did.
- **NEVER** try to write to paths outside the sandbox. The tool will
  reject them with a clear error — treat that as a hard stop, not a
  retry prompt. If Sebastian asks you to write somewhere weird, tell
  him the sandbox rules and suggest a path inside.
- **If a tool call fails**, say so. Do not wave it away. Do not
  emit the SEND_FILE marker anyway. The bridge now appends a visible
  "attachment failed" block when you lie about attachments — the user
  will see your mistake in red.

### Examples

User: "Research diffusion transformers and give me a PDF"
  1. browse_url arxiv.org/abs/XXXX
  2. browse_url a second source if helpful
  3. write_file("~/MAKAKOO/data/reports/diffusion_transformers_2026_04_10.md",
                "# Diffusion Transformers\n\n## Abstract\n...")
  4. markdown_to_pdf("~/MAKAKOO/data/reports/diffusion_transformers_2026_04_10.md")
  5. "Report is ready." + [[SEND_FILE:/Users/sebastian/MAKAKOO/data/reports/diffusion_transformers_2026_04_10.pdf]]

User: "Attach me my CV"
  1. run_command("ls ~/CV/") → verify Sebastian_Schkudlara_CV_2026.pdf exists
  2. "Here." + [[SEND_FILE:~/CV/Sebastian_Schkudlara_CV_2026.pdf]]
  (CV lives outside the sandbox — that's fine for attaching existing
  files via SEND_FILE; the sandbox only restricts WRITING.)

## Response Style
- Keep responses concise — this is mobile messaging.
- Use short paragraphs. No code blocks unless asked.
- Reference specific facts from tool results.
- If asking follow-ups, keep them to 1-2 questions max.

## What You Can Do
- Search Brain for projects, people, decisions, past work
- Check and summarize emails (Gmail)
- Check today's calendar
- Run safe read-only shell commands (ls, ps, crontab -l, git status, uptime, df, etc.)
- Browse any public URL for current information
- **Generate images** — use `generate_image("prompt", "save_path.png", "16:9")`
- Discover relevant skills from Harvey's 170+ skill library
- Log notes and decisions to Brain journal
- Draft messages, emails, responses
- Help think through decisions and strategy
- Send files to the user via Telegram: use `[[SEND_FILE:/full/path/to/file.pdf]]`
- Send photos to the user via Telegram: use `[[SEND_PHOTO:/path/to/image.png]]`
- Set the Telegram bot's profile photo: NOT POSSIBLE — Telegram bots can't change their own avatar via API (only @BotFather can)
- Examples:
  - User: "attach me my CV" → "Here is your CV." + `[[SEND_FILE:~/CV/Sebastian_Schkudlara_CV_2026.pdf]]`
  - User: "generate an image of a cute owl" → Use `generate_image` tool, then `[[SEND_PHOTO:~/pics/generated.png]]`
  - User: "send me that diagram" → "Here it is." + `[[SEND_PHOTO:~/projects/diagram.png]]`
  - User: "look up the latest from example.com" → `browse_url("https://example.com")` then summarize
  - User: "list my photos" → `run_command("ls ~/pics/")`
"""

def render_system_prompt(
    channel: str = "telegram",
    grants: "Optional[Any]" = None,
) -> str:
    """Fill the `{allowed_paths}` + `{channel}` placeholders from the
    current grant store.

    Invoked once per new assistant turn so mid-session `grant_write_access`
    calls become visible in the next prompt regeneration — see SPRINT.md
    §C.6. Keeps the template deliberately short: a bullet list of roots
    (baseline + active grants), no audit detail, no per-turn timestamps.

    `grants` may be a pre-loaded `UserGrantsFile` (tests), else we load
    from the canonical path. A load failure degrades gracefully to the
    baseline roots — never crashes the prompt pipeline.
    """
    try:
        from core.agent.harvey_agent import effective_write_file_roots
        roots = effective_write_file_roots(grants)
    except Exception as e:
        log.warning("render_system_prompt: falling back to baseline (%s)", e)
        roots = [
            "~/MAKAKOO/data/reports",
            "~/MAKAKOO/data/drafts",
            "~/MAKAKOO/tmp",
            "/tmp",
        ]

    bullets: list[str] = []
    for r in roots:
        if r.endswith("**"):
            bullets.append(f"  - `{r}`  — runtime grant (expires automatically)")
        elif r.endswith("*"):
            bullets.append(f"  - `{r}`  — runtime grant (single-segment)")
        else:
            trailing = "" if r.endswith("/") else "/"
            bullets.append(f"  - `{r}{trailing}`  — baseline")
    allowed_paths = "\n".join(bullets) if bullets else "  (no writable paths configured)"

    return HARVEY_SYSTEM_PROMPT.format(
        channel=channel,
        allowed_paths=allowed_paths,
    )


# Minimal system prompt for Anthropic fallback (no tools)
ANTHROPIC_FALLBACK_PROMPT = """You are Harvey, Sebastian Schkudlara's autonomous cognitive extension.
Responding via {channel} (mobile messaging).

## Identity
- Zero sycophancy. Skip preamble — just execute.
- Radical competence. Return with solutions, not questions.
- Tone: Sharp, concise, hyper-competent, slightly blunt.

## Response Style
- Keep responses concise — this is mobile messaging.
- Use short paragraphs. No code blocks unless asked.

## Note
You are running in fallback mode without tool access. You cannot search Brain or fetch emails right now.
For complex tasks, suggest Sebastian open Claude Code / CLI.
"""


class HarveyBridge:
    """Routes chat messages through Harvey Agent Core with Anthropic fallback."""

    def __init__(self, config: BridgeConfig):
        self.config = config
        self.agent = HarveyAgent(
            llm_url=config.switchai_url,
            llm_model=config.switchai_model,
            api_key=config.switchai_api_key,
            max_tokens=config.max_tokens,
        )

    def _build_system_prompt(self, channel: str = "telegram") -> str:
        """Build tool-aware system prompt (grants refreshed every turn)."""
        return render_system_prompt(channel=channel)

    # Regex that catches the common "I can't do files" hallucinations the
    # LLM sometimes produces when its context gets contaminated by prior
    # honest-failure responses. If an assistant message in HISTORY matches
    # any of these, we strip it before passing history to the next LLM
    # call so the model can't few-shot-learn from its own defeatism.
    _CONTAMINATION_PATTERNS = re.compile(
        r"("
        r"i\s+cannot\s+(create|write|make|generate)\s+(files?|pdfs?|documents?)"
        r"|i\s+don'?t\s+have\s+(that\s+capability|the\s+ability\s+to\s+(create|write))"
        r"|i\s+can'?t\s+(create|write|make)\s+(files?|pdfs?)"
        r"|past\s+attempts\s+were\s+me\s+falsely\s+claiming"
        r"|i\s+fabricated\s+the\s+entire\s+sequence"
        r"|you'?re\s+quoting\s+my\s+lies"
        r"|i\s+have\s+to\s+be\s+straight\s+with\s+you.*don'?t\s+actually\s+have"
        r")",
        re.IGNORECASE,
    )

    _DEFEATIST_PATTERNS = re.compile(
        r"("
        r"i\s+cannot\s+(create|write|make|generate)"
        r"|i\s+don'?t\s+have\s+(that\s+capability|the\s+ability)"
        r"|i\s+can'?t\s+(create|write|make)\s+(files?|pdfs?)"
        r"|past\s+attempts\s+were\s+me\s+falsely"
        r"|i\s+fabricated"
        r"|i\s+need\s+to\s+be\s+upfront"
        r"|i\s+have\s+to\s+be\s+straight\s+with\s+you"
        r"|pick\s+one\s*:?\s*\*?\*?(write\s+to\s+brain|inline|image\s+summary)"
        r")",
        re.IGNORECASE,
    )

    _FILE_REQUEST_PATTERNS = re.compile(
        r"("
        r"\bpdf\b|\breport\b|\battach\b|\bdocument\b"
        r"|write\s+(a|the|this|me)\s+(file|md|markdown)"
        r"|save\s+(to|as|in)\s+"
        r"|export\s+(to|as)\s+"
        r"|create\s+(a|the)\s+(file|doc|report|pdf)"
        r"|give\s+me\s+(a|the)\s+(pdf|file|report|doc)"
        r")",
        re.IGNORECASE,
    )

    def _looks_defeatist(self, response: str) -> bool:
        """True if the response pattern-matches Olibia's hallucinated-failure template."""
        return bool(self._DEFEATIST_PATTERNS.search(response or ""))

    def _user_wants_file(self, message: str) -> bool:
        """True if the user's message looks like a file/report ask."""
        return bool(self._FILE_REQUEST_PATTERNS.search(message or ""))

    def _sanitize_history(self, history: List[Dict]) -> List[Dict]:
        """Drop assistant messages that poison future tool-use.

        When Olibia has previously said "I cannot write files" (usually
        because older versions of Harvey had no write_file tool), that
        message sits in the conversation history and few-shot-teaches
        the next LLM turn to repeat the same defeatism — even after the
        tools and system prompt have been upgraded. Filter those out.
        User messages are never touched.
        """
        cleaned = []
        dropped = 0
        for msg in history:
            if msg.get("role") == "assistant":
                content = msg.get("content", "") or ""
                if self._CONTAMINATION_PATTERNS.search(content):
                    dropped += 1
                    continue
            cleaned.append(msg)
        if dropped:
            log.info(f"[history-sanitizer] dropped {dropped} contaminated assistant message(s)")
        return cleaned

    def send(
        self,
        message: str,
        history: List[Dict],
        channel: str = "telegram",
        file_sender: Optional[Callable[[str, str], None]] = None,
        task_id: Optional[str] = None,
        store: Optional[Any] = None,
    ) -> str:
        """
        Send a message to Harvey and get a response.

        Flow:
        1. Try HarveyAgent (switchAILocal with tool-calling)
        2. Fall back to direct LLM (no tools)
        3. Return offline message if both fail

        If file_sender is provided and response contains [[SEND_FILE:...]] markers,
        those files are sent via the file_sender callback before returning.

        Cognitive core params (optional, backward-compatible):
            task_id: Task ID for checkpointing — passed into HarveyAgent.process()
            store: TaskStore instance for checkpoint writes
        """
        # Phase E.3 — surface identity for audit + the Telegram allowlist
        # gate on grant_write_access. Telegram chat_id propagation
        # (HARVEY_TELEGRAM_CHAT_ID) is done by the adapter that calls us
        # for Telegram — in v1 the chat_id is set via a separate env
        # write by the channel before it enqueues the message; the gap
        # is acceptable because non-Telegram surfaces short-circuit.
        os.environ["HARVEY_PLUGIN"] = (
            "harveychat-telegram" if channel == "telegram" else "harveychat"
        )

        system_prompt = self._build_system_prompt(channel)

        # Trim history to max
        trimmed_history = history[-(self.config.max_history_messages) :]

        # Scrub contaminated "I can't create files" messages from history
        # BEFORE sending to the LLM. This is the real fix for the
        # hallucinated-failure loop — without it, the model keeps
        # pattern-matching its own prior defeatism.
        trimmed_history = self._sanitize_history(trimmed_history)

        # Build history for agent (ensure current message is included)
        agent_history = []
        for msg in trimmed_history:
            agent_history.append({"role": msg["role"], "content": msg["content"]})

        # Try agent (switchAILocal with tools) — with retry on transient failures
        last_agent_error = None
        for attempt in range(RETRY_MAX_ATTEMPTS):
            try:
                response = self.agent.process(
                    message, agent_history, system_prompt, channel,
                    task_id=task_id, store=store,
                )
                if response:
                    log.info(f"Agent response: {len(response)} chars")
                    # If the user asked for a file/report and the model STILL
                    # produced a defeatist "I can't" response, retry once with
                    # a corrective nudge prepended to the system prompt.
                    if self._looks_defeatist(response) and self._user_wants_file(message):
                        log.warning(
                            "[bridge] defeatist response to file request — retrying"
                        )
                        nudge = (
                            "\n\n## CRITICAL — CORRECTION\n"
                            "Your previous response falsely claimed you cannot "
                            "create files. YOU CAN. You have `write_file` and "
                            "`markdown_to_pdf` tools. For this request you MUST "
                            "call `write_file` first (target directory: "
                            "~/MAKAKOO/data/reports/), then `markdown_to_pdf`, "
                            "then emit `[[SEND_FILE:<the real pdf path>]]`. "
                            "Do NOT narrate limitations. Do NOT offer fallback "
                            "options. Execute the tools."
                        )
                        try:
                            response = self.agent.process(
                                message, list(agent_history), system_prompt + nudge, channel,
                                task_id=task_id, store=store,
                            )
                            if response:
                                log.info(f"Agent retry response: {len(response)} chars")
                        except Exception as e:
                            log.warning(f"Agent retry error: {e}")

                    if file_sender:
                        response = self._handle_file_markers(response, file_sender)
                    return response
                # Empty response — treat as transient, retry
                last_agent_error = Exception("empty response from agent")
                if attempt < RETRY_MAX_ATTEMPTS - 1:
                    delay = _compute_backoff(attempt)
                    log.warning(
                        f"Agent returned empty response (attempt {attempt + 1}/{RETRY_MAX_ATTEMPTS}) "
                        f"— retrying in {delay:.1f}s"
                    )
                    time.sleep(delay)
                continue
            except Exception as e:
                last_agent_error = e
                if not _is_retryable_error(e):
                    log.warning(f"Agent error (non-retryable): {e}")
                    break
                if attempt < RETRY_MAX_ATTEMPTS - 1:
                    delay = _compute_backoff(attempt)
                    log.warning(
                        f"Agent error (attempt {attempt + 1}/{RETRY_MAX_ATTEMPTS}): {e} "
                        f"— retrying in {delay:.1f}s"
                    )
                    time.sleep(delay)
                else:
                    log.warning(
                        f"Agent error (attempt {attempt + 1}/{RETRY_MAX_ATTEMPTS}, giving up): {e}"
                    )

        # Fallback: direct LLM (no tools) — with retry
        log.info(
            f"Agent failed ({last_agent_error}), trying direct LLM fallback"
        )
        for attempt in range(RETRY_MAX_ATTEMPTS):
            try:
                response = self._try_switchai_direct(
                    render_system_prompt(channel=channel),
                    self._build_messages(message, trimmed_history),
                )
                if response:
                    if file_sender:
                        response = self._handle_file_markers(response, file_sender)
                    return response
                # None return = non-retryable failure (auth, bad request) — stop trying
                log.warning("Direct LLM returned None (non-retryable) — skipping further attempts")
                break
            except Exception as e:
                if not _is_retryable_error(e):
                    log.warning(f"Direct LLM error (non-retryable): {e}")
                    break
                if attempt < RETRY_MAX_ATTEMPTS - 1:
                    delay = _compute_backoff(attempt)
                    log.warning(
                        f"Direct LLM error (attempt {attempt + 1}/{RETRY_MAX_ATTEMPTS}): {e} "
                        f"— retrying in {delay:.1f}s"
                    )
                    time.sleep(delay)
                else:
                    log.warning(
                        f"Direct LLM error (attempt {attempt + 1}/{RETRY_MAX_ATTEMPTS}, giving up): {e}"
                    )

        return (
            "Harvey is having trouble connecting. "
            "Try again — if it keeps failing, check if switchAILocal is running."
        )

    def _build_messages(self, message: str, history: List[Dict]) -> List[Dict]:
        """Build message list for non-agentic LLM calls."""
        messages = []
        for msg in history:
            messages.append({"role": msg["role"], "content": msg["content"]})
        if not messages or messages[-1].get("content") != message:
            messages.append({"role": "user", "content": message})
        return messages

    def _handle_file_markers(
        self, response: str, file_sender: Callable[[str, str], None]
    ) -> str:
        """
        Parse response for [[SEND_FILE:path]] and [[SEND_PHOTO:path]] markers.
        Calls file_sender(user_id, path) for each file, then removes markers from text.

        If a referenced file does not exist, appends a visible error block
        to the response so the user sees the failure instead of a bot that
        confidently claims "file attached" while nothing actually arrived.
        """
        FILE_PATTERN = re.compile(r"\[\[SEND_FILE:([^\]]+)\]\]")
        PHOTO_PATTERN = re.compile(r"\[\[SEND_PHOTO:([^\]]+)\]\]")

        failures = []  # (kind, requested_path) — surfaced to the user

        for match in FILE_PATTERN.finditer(response):
            raw = match.group(1).strip()
            path = os.path.expanduser(raw)
            if os.path.exists(path):
                try:
                    file_sender("file", path)
                    log.info(f"Sent file via marker: {path}")
                except Exception as e:
                    log.warning(f"Failed to send file {path}: {e}")
                    failures.append(("file", raw, f"send error: {e}"))
            else:
                log.warning(f"SEND_FILE marker: file not found: {path}")
                failures.append(("file", raw, "file does not exist"))

        for match in PHOTO_PATTERN.finditer(response):
            raw = match.group(1).strip()
            path = os.path.expanduser(raw)
            if os.path.exists(path):
                try:
                    file_sender("photo", path)
                    log.info(f"Sent photo via marker: {path}")
                except Exception as e:
                    log.warning(f"Failed to send photo {path}: {e}")
                    failures.append(("photo", raw, f"send error: {e}"))
            else:
                log.warning(f"SEND_PHOTO marker: file not found: {path}")
                failures.append(("photo", raw, "file does not exist"))

        # Remove all markers from response text
        response = FILE_PATTERN.sub("", response)
        response = PHOTO_PATTERN.sub("", response)
        response = response.strip()

        # Surface any failures to the user — no more silent lies
        if failures:
            error_lines = ["", "⚠ **Attachment failed:**"]
            for kind, path, reason in failures:
                error_lines.append(f"  • `{path}` — {reason}")
            error_lines.append(
                "\nI said I was attaching something but I can't — the file "
                "doesn't exist. I don't have a tool to create PDFs or write "
                "arbitrary files. Tell me what you want inline instead."
            )
            response = (response + "\n" + "\n".join(error_lines)).strip()

        return response

    def _try_anthropic(self, system: str, messages: List[Dict]) -> Optional[str]:
        """Fallback to direct Anthropic API (no tools)."""
        api_key = self.config.anthropic_api_key
        if not api_key:
            return None

        try:
            headers = {
                "x-api-key": api_key,
                "anthropic-version": "2023-06-01",
                "content-type": "application/json",
            }

            payload = {
                "model": self.config.anthropic_model,
                "max_tokens": self.config.max_tokens,
                "system": system,
                "messages": messages,
            }

            r = requests.post(
                "https://api.anthropic.com/v1/messages",
                headers=headers,
                json=payload,
                timeout=120,
            )

            if r.status_code == 200:
                data = r.json()
                content = data["content"][0]["text"]
                log.info(f"Anthropic response: {len(content)} chars")
                return content
            else:
                log.warning(f"Anthropic returned {r.status_code}: {r.text[:200]}")
                return None

        except Exception as e:
            log.warning(f"Anthropic error: {e}")
            return None

    def _try_switchai_direct(self, system: str, messages: List[Dict]) -> Optional[str]:
        """Direct switchAILocal call without tools (fallback).

        Raises on retryable errors so the caller's retry loop can handle them.
        Returns None on non-retryable failures (auth, bad request).
        """
        if not self.config.switchai_url:
            return None

        headers = {"Content-Type": "application/json"}
        if self.config.switchai_api_key:
            headers["Authorization"] = f"Bearer {self.config.switchai_api_key}"

        payload = {
            "model": self.config.switchai_model,
            "messages": [{"role": "system", "content": system}] + messages,
            "max_tokens": self.config.max_tokens,
            "temperature": 0.7,
        }

        try:
            r = requests.post(
                f"{self.config.switchai_url}/chat/completions",
                headers=headers,
                json=payload,
                timeout=120,
            )
        except (requests.exceptions.ConnectionError,
                requests.exceptions.Timeout) as e:
            # Network-level failure — let caller retry
            raise

        if r.status_code == 200:
            data = r.json()
            content = data["choices"][0]["message"]["content"]
            log.info(f"Direct LLM response: {len(content)} chars")
            return content

        if _is_retryable_status(r.status_code):
            log.warning(f"Direct LLM returned {r.status_code} (retryable): {r.text[:200]}")
            raise requests.exceptions.HTTPError(
                f"switchAILocal returned {r.status_code}", response=r
            )

        # Non-retryable (401, 400, etc.)
        log.warning(f"Direct LLM returned {r.status_code}: {r.text[:200]}")
        return None
