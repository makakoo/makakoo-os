"""
Harvey Agent Core — Reusable agentic loop with tool-calling.

Every Harvey body (Telegram, WhatsApp, Discord, Robot) uses this.
The agent calls switchAILocal with OpenAI function-calling format,
executes tool calls, and loops until it has a final text response.
"""

import json
import logging
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

import requests

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

log = logging.getLogger("harvey.agent")

MAX_TOOL_ROUNDS = 40
# Safe whitelist of directories write_file + markdown_to_pdf can touch.
# Keeps the write surface contained — the agent cannot overwrite source
# code, configs, or Sebastian's personal files. Extend cautiously.
WRITE_FILE_ROOTS = (
    os.path.join(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")), "data", "reports"),
    os.path.join(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")), "data", "drafts"),
    os.path.join(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")), "tmp"),
    "/tmp",
)

# ═══════════════════════════════════════════════════════════════
#  Tool Definitions (OpenAI function-calling format)
# ═══════════════════════════════════════════════════════════════

HARVEY_TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "brain_search",
            "description": "Search Harvey's Brain (Brain knowledge graph) for any topic — projects, people, decisions, skills, past work",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to search for"}
                },
                "required": ["query"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "brain_write",
            "description": "Write an entry to today's Brain journal (Brain format). Use for logging decisions, tasks, notes.",
            "parameters": {
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Content to write. Each line should start with '- ' for outliner. Use [[entity]] for links.",
                    }
                },
                "required": ["content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "skill_discover",
            "description": "Find relevant Harvey skills by semantic search. Harvey has 170+ skills across AI, dev, research, productivity, etc.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What capability or skill to search for",
                    }
                },
                "required": ["query"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_emails",
            "description": "Fetch recent Gmail messages. Returns sender, subject, and preview for each.",
            "parameters": {
                "type": "object",
                "properties": {
                    "count": {
                        "type": "integer",
                        "description": "Number of emails to fetch (default 5, max 10)",
                    }
                },
                "required": [],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_calendar",
            "description": "Fetch today's calendar events from Google Calendar.",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": [],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "superbrain_status",
            "description": "Check Superbrain health — vector count, FTS5 index size, systems online.",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": [],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "generate_image",
            "description": "Generate an image from a text prompt using MiniMax image-01 model via switchAILocal. The image is saved to a file and sent to the user via Telegram. Use this when the user asks you to create, draw, or generate an image, picture, or photo.",
            "parameters": {
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Detailed text description of the image to generate. Be specific and descriptive.",
                    },
                    "aspect_ratio": {
                        "type": "string",
                        "description": "Aspect ratio: '1:1', '16:9', '9:16', '4:3', '3:4'. Default: '1:1'",
                    },
                    "save_path": {
                        "type": "string",
                        "description": "Full path where to save the generated image, e.g. '~/pics/generated.png'",
                    },
                },
                "required": ["prompt", "save_path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "run_command",
            "description": "Execute a safe read-only shell command on the local machine. ONLY informational commands allowed — no file writes, no network calls, no git pushes, no package installs. Allowed: ps, launchctl, crontab, git status, ls, pwd, uptime, df, free, top, who, hostname, uname.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute (no sudo, no redirects, no pipes, no cd)",
                    }
                },
                "required": ["command"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "browse_url",
            "description": "Fetch and summarize a web page. Extracts the main content from a URL. Use this when you need current information from the web, or to read an article, documentation, or any public URL.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The full URL to fetch (must start with http:// or https://)",
                    },
                    "query": {
                        "type": "string",
                        "description": "What specifically to look for on the page, or 'summary' for a general summary",
                    },
                },
                "required": ["url"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "set_telegram_profile_photo",
            "description": "Set the Telegram bot's profile photo. The bot must own the chat (you must be admin of the channel/bot).",
            "parameters": {
                "type": "object",
                "properties": {
                    "photo_path": {
                        "type": "string",
                        "description": "Full path to the image file on disk (must be a valid image file, PNG or JPEG)",
                    },
                },
                "required": ["photo_path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "superbrain_vector_search",
            "description": "Semantic search using vector embeddings. Finds conceptually related facts from Harvey's memory using AI embeddings. Better than keyword search for understanding meaning and intent.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to search for semantically (e.g., 'authentication decisions', 'superbrain improvements', 'recent decisions')",
                    }
                },
                "required": ["query"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_consolidated_insights",
            "description": "Get today's automatically consolidated insights from the memory system. Returns key decisions, learnings, and project updates consolidated from the day's journal.",
            "parameters": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Number of top insights to return (default 5)",
                    }
                },
                "required": [],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "Write text content to a file inside Harvey's sandboxed write directories (HARVEY_HOME/data/reports, HARVEY_HOME/data/drafts, HARVEY_HOME/tmp, /tmp). Use this to create reports, drafts, markdown documents, or any text file the user asks for. Returns the absolute path written. REQUIRED before markdown_to_pdf or before emitting [[SEND_FILE:...]] markers.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Target file path. Must be inside ~/MAKAKOO/data/reports/, ~/MAKAKOO/data/drafts/, ~/MAKAKOO/tmp/, or /tmp/. Example: '~/MAKAKOO/data/reports/diffusion_research.md'",
                    },
                    "content": {
                        "type": "string",
                        "description": "The full text content to write. UTF-8.",
                    },
                },
                "required": ["path", "content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "markdown_to_pdf",
            "description": "Convert an existing markdown file to PDF using pandoc + pdflatex. Use this AFTER write_file to turn a markdown report into a PDF the user can receive as a Telegram attachment. Both input and output paths must be inside the sandboxed write directories.",
            "parameters": {
                "type": "object",
                "properties": {
                    "md_path": {
                        "type": "string",
                        "description": "Path to the source markdown file (must exist, must be inside the sandbox)",
                    },
                    "pdf_path": {
                        "type": "string",
                        "description": "Optional target PDF path. If omitted, derived from md_path by swapping the extension.",
                    },
                },
                "required": ["md_path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "spawn_subagent",
            "description": (
                "Delegate a focused task to one of Harvey's specialized in-process subagents. "
                "Use this when a user request benefits from a specialist: 'researcher' for deep "
                "Brain+vector search, 'synthesizer' for composing long-form text from findings, "
                "'storage' for durable write operations, 'image_gen' for image generation, "
                "'task_master' for task management, 'olibia' for persona-flavored commentary. "
                "Returns the subagent's result as a summary string — incorporate that into your "
                "next response or chain a follow-up spawn_subagent call."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "agent_name": {
                        "type": "string",
                        "description": "One of: researcher, synthesizer, storage, image_gen, task_master, olibia",
                    },
                    "task": {
                        "type": "string",
                        "description": "The task text the subagent should work on (becomes ctx['query']/'topic')",
                    },
                    "action": {
                        "type": "string",
                        "description": "Optional specific action — defaults to the subagent's first declared action",
                    },
                },
                "required": ["agent_name", "task"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "get_emails_filtered",
            "description": "Fetch recent Gmail messages with intelligent filtering. Prioritizes job-related emails (offers, recruiter outreach, client responses) and filters out noise (promotions, notifications).",
            "parameters": {
                "type": "object",
                "properties": {
                    "count": {
                        "type": "integer",
                        "description": "Number of emails to fetch (default 5, max 10)",
                    }
                },
                "required": [],
            },
        },
    },
]


# ═══════════════════════════════════════════════════════════════
#  Tool Execution Functions
# ═══════════════════════════════════════════════════════════════


def _get_subprocess_env() -> dict:
    """Build environment for subprocess calls (gws, etc.)."""
    env = os.environ.copy()
    extra_paths = [
        "/usr/local/bin",
        "/opt/homebrew/bin",
        os.path.expanduser("~/.nvm/versions/node/v22.17.0/bin"),
        os.path.expanduser("~/bin"),
    ]
    env["PATH"] = ":".join(extra_paths) + ":" + env.get("PATH", "")
    return env


def tool_brain_search(query: str) -> str:
    """Search Superbrain for knowledge."""
    try:
        from core.superbrain.superbrain import Superbrain

        sb = Superbrain()
        result = sb.query(query, synthesize=False, top_k=5)
        if result.sources:
            parts = []
            for s in result.sources[:5]:
                parts.append(f"- {s.title}: {s.text[:400]}")
            return f"Brain search results for '{query}':\n" + "\n".join(parts)
        return f"No results found in Brain for '{query}'."
    except Exception as e:
        log.warning(f"brain_search failed: {e}")
        return f"Brain search error: {e}"


def _resolve_write_path(path: str) -> Optional[str]:
    """Resolve a write target and verify it's inside a whitelisted root.

    Returns the real absolute path on success, None if the path escapes
    the sandbox. Prevents the agent from writing to arbitrary filesystem
    locations even if it's tricked with `..`, a full alternate path, or
    a symlink pointing outside the sandbox. `realpath` resolves symlinks
    on both sides of the `commonpath` check so a link like
    `~/MAKAKOO/tmp/escape -> /etc` cannot smuggle a write to /etc/passwd.
    """
    if not path:
        return None
    expanded = os.path.realpath(os.path.expanduser(path))
    for root in WRITE_FILE_ROOTS:
        root_abs = os.path.realpath(os.path.expanduser(root))
        try:
            if os.path.commonpath([expanded, root_abs]) == root_abs:
                return expanded
        except ValueError:
            # Different drives on Windows — can't happen on macOS/Linux
            continue
    return None


def tool_write_file(path: str, content: str) -> str:
    """Write text content to a file inside a sandboxed directory.

    Allowed roots are HARVEY_HOME/data/reports, HARVEY_HOME/data/drafts,
    HARVEY_HOME/tmp, and /tmp. Any other path is rejected.
    """
    resolved = _resolve_write_path(path)
    if resolved is None:
        allowed = ", ".join(WRITE_FILE_ROOTS)
        return (
            f"write_file rejected: {path!r} is outside the allowed "
            f"directories ({allowed})."
        )
    try:
        os.makedirs(os.path.dirname(resolved), exist_ok=True)
        with open(resolved, "w", encoding="utf-8") as f:
            f.write(content)
        size = os.path.getsize(resolved)
        return f"Wrote {size} bytes to {resolved}"
    except Exception as e:
        log.warning(f"write_file failed: {e}")
        return f"write_file error: {e}"


def tool_markdown_to_pdf(md_path: str, pdf_path: str = "") -> str:
    """Convert a markdown file to PDF via pandoc + pdflatex.

    Both input and output paths must land inside the write-file sandbox.
    If pdf_path is empty, derives it from the markdown path by swapping
    the extension.
    """
    pandoc = shutil.which("pandoc")
    if not pandoc:
        return (
            "markdown_to_pdf error: pandoc not installed. Install with "
            "`brew install pandoc basictex` and retry."
        )

    md_resolved = _resolve_write_path(md_path)
    if md_resolved is None:
        return (
            f"markdown_to_pdf rejected: input {md_path!r} is outside "
            f"the allowed directories."
        )
    if not os.path.exists(md_resolved):
        return f"markdown_to_pdf error: input file not found: {md_resolved}"

    if not pdf_path:
        pdf_path = os.path.splitext(md_resolved)[0] + ".pdf"
    pdf_resolved = _resolve_write_path(pdf_path)
    if pdf_resolved is None:
        return (
            f"markdown_to_pdf rejected: output {pdf_path!r} is outside "
            f"the allowed directories."
        )

    try:
        os.makedirs(os.path.dirname(pdf_resolved), exist_ok=True)
        result = subprocess.run(
            [
                pandoc,
                md_resolved,
                "-o", pdf_resolved,
                "--pdf-engine=pdflatex",
                "-V", "geometry:margin=1in",
                "-V", "fontsize=11pt",
            ],
            capture_output=True, text=True, timeout=60,
        )
        if result.returncode != 0:
            # Retry without pdflatex if LaTeX isn't installed — use
            # pandoc's default (wkhtmltopdf / weasyprint / etc. if any)
            fallback = subprocess.run(
                [pandoc, md_resolved, "-o", pdf_resolved],
                capture_output=True, text=True, timeout=60,
            )
            if fallback.returncode != 0:
                err = (result.stderr or fallback.stderr or "unknown")[:300]
                return f"markdown_to_pdf failed: {err}"
        size = os.path.getsize(pdf_resolved)
        return f"Converted {md_resolved} → {pdf_resolved} ({size} bytes)"
    except subprocess.TimeoutExpired:
        return "markdown_to_pdf error: pandoc timed out after 60s"
    except Exception as e:
        log.warning(f"markdown_to_pdf failed: {e}")
        return f"markdown_to_pdf error: {e}"


def tool_brain_write(content: str) -> str:
    """Write to today's Brain journal and sync it into Superbrain.

    Structural invariant: if the write lands on disk, sync_file is ALWAYS
    called. The only way to skip sync is to never write — any path that
    writes must pass through the sync block. The tool_brain_write orphan
    bug that made every entry since 2026-04-09 invisible to search came
    from a structural gap here; don't reintroduce it.
    """
    journal_path: Optional[Path] = None
    wrote_file = False
    try:
        today = time.strftime("%Y_%m_%d")
        journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
        journal_path.parent.mkdir(parents=True, exist_ok=True)

        # Ensure every line starts with "- " (Brain outliner format)
        lines = content.strip().split("\n")
        formatted = []
        for line in lines:
            stripped = line.strip()
            if stripped and not stripped.startswith("- "):
                stripped = f"- {stripped}"
            formatted.append(stripped)

        entry = "\n".join(formatted) + "\n"

        with open(journal_path, "a") as f:
            f.write(entry)
        wrote_file = True
    except Exception as e:
        log.warning(f"brain_write failed: {e}")
        return f"Brain write error: {e}"

    # Sync to Superbrain (FTS5 + entity graph + embedding) so the entry
    # is immediately searchable. This block is reached IF AND ONLY IF
    # the write succeeded. Sync failure must not lose the write.
    sync_status = ""
    if wrote_file and journal_path is not None:
        try:
            from core.superbrain.superbrain import Superbrain
            sb = Superbrain()
            if sb.sync_file(str(journal_path), embed=True):
                sync_status = " [synced]"
            else:
                sync_status = " [sync skipped]"
        except Exception as sync_err:
            log.warning(f"brain_write sync failed (file was written): {sync_err}")
            sync_status = " [sync failed]"

    return f"Written to journal ({time.strftime('%Y_%m_%d')}){sync_status}: {content[:100]}..."


def tool_skill_discover(query: str) -> str:
    """Find skills matching a query."""
    try:
        from core.registry.skill_registry import SkillRegistry

        reg = SkillRegistry()
        results = reg.find_skills(query, top_k=3)
        if results:
            parts = []
            for r in results:
                skill = r["skill"]
                name = skill.get("name", "?")
                desc = skill.get("description", "")[:150]
                category = skill.get("category", "?")
                parts.append(f"- {name} ({category}): {desc}")
            return f"Skills matching '{query}':\n" + "\n".join(parts)
        return f"No skills found matching '{query}'."
    except Exception as e:
        log.warning(f"skill_discover failed: {e}")
        return f"Skill discovery error: {e}"


def tool_get_emails(count: int = 5) -> str:
    """Fetch recent emails via gws CLI."""
    count = min(count or 5, 10)
    try:
        env = _get_subprocess_env()
        gws_cmd = shutil.which("gws") or "gws"

        result = subprocess.run(
            [
                gws_cmd,
                "gmail",
                "users",
                "messages",
                "list",
                "--params",
                json.dumps({"userId": "me", "maxResults": count}),
                "--format",
                "json",
            ],
            capture_output=True,
            text=True,
            timeout=15,
            env=env,
        )
        if result.returncode != 0:
            return f"Gmail fetch failed: {result.stderr[:200]}"

        data = json.loads(result.stdout)
        messages = data.get("messages", [])
        if not messages:
            return "No recent emails found."

        emails = []
        for msg in messages[:count]:
            msg_id = msg["id"]
            subresult = subprocess.run(
                [
                    gws_cmd,
                    "gmail",
                    "users",
                    "messages",
                    "get",
                    "--params",
                    json.dumps({"userId": "me", "id": msg_id}),
                    "--format",
                    "json",
                ],
                capture_output=True,
                text=True,
                timeout=15,
                env=env,
            )
            if subresult.returncode != 0:
                continue
            try:
                m = json.loads(subresult.stdout)
                m = m.get("message", m)
                headers = {
                    h["name"].lower(): h["value"]
                    for h in m.get("payload", {}).get("headers", [])
                }
                sender = headers.get("from", "Unknown")
                subject = headers.get("subject", "(no subject)")
                snippet = m.get("snippet", "")[:150]
                emails.append(
                    f"- From: {sender}\n  Subject: {subject}\n  Preview: {snippet}"
                )
            except Exception:
                continue

        if emails:
            return f"Recent emails ({len(emails)}):\n" + "\n".join(emails)
        return "Could not parse any emails."
    except FileNotFoundError:
        return "gws CLI not found. Cannot fetch emails."
    except subprocess.TimeoutExpired:
        return "Gmail fetch timed out."
    except Exception as e:
        log.warning(f"get_emails failed: {e}")
        return f"Email fetch error: {e}"


def tool_get_calendar() -> str:
    """Fetch today's calendar events via gws CLI."""
    try:
        env = _get_subprocess_env()
        gws_cmd = shutil.which("gws") or "gws"
        today_start = time.strftime("%Y-%m-%dT00:00:00Z")
        today_end = time.strftime("%Y-%m-%dT23:59:59Z")

        result = subprocess.run(
            [
                gws_cmd,
                "calendar",
                "events",
                "list",
                "primary",
                "--time-min",
                today_start,
                "--time-max",
                today_end,
                "--format",
                "json",
            ],
            capture_output=True,
            text=True,
            timeout=15,
            env=env,
        )
        if result.returncode != 0:
            return f"Calendar fetch failed: {result.stderr[:200]}"

        data = json.loads(result.stdout)
        events = data.get("items", data.get("events", []))
        if not events:
            return "No calendar events today."

        parts = []
        for ev in events:
            summary = ev.get("summary", "(no title)")
            start = ev.get("start", {})
            start_time = start.get("dateTime", start.get("date", "?"))
            parts.append(f"- {start_time}: {summary}")

        return f"Today's calendar ({len(parts)} events):\n" + "\n".join(parts)
    except FileNotFoundError:
        return "gws CLI not found. Cannot fetch calendar."
    except subprocess.TimeoutExpired:
        return "Calendar fetch timed out."
    except Exception as e:
        log.warning(f"get_calendar failed: {e}")
        return f"Calendar fetch error: {e}"


# ═══════════════════════════════════════════════════════════════
#  Allowed Commands (whitelist — no writes, no network, no git push)
# ═══════════════════════════════════════════════════════════════

SAFE_COMMANDS = frozenset(
    {
        "ps",
        "ps aux",
        "ps auxww",
        "uptime",
        "who",
        "hostname",
        "uname",
        "df",
        "df -h",
        "free",
        "top",
        "top -l 5",
        "ls",
        "ls -la",
        "ls -lh",
        "pwd",
        "cd",
        "launchctl list",
        "crontab -l",
        "git status",
        "git log --oneline -5",
        "git branch",
        "git remote -v",
        "ps aux | grep",
        "pmset -g",
        "caffeinate -u",
        "curl -s http://localhost:18080/health",
        "curl -s http://localhost:12315/health",
    }
)

ALLOWED_PREFIXES = (
    "ps ",
    "uptime",
    "who",
    "hostname",
    "uname -a",
    "df ",
    "free",
    "top ",
    "ls ",
    "pwd",
    "launchctl",
    "crontab",
    "git status",
    "git log",
    "git branch",
    "git remote",
    "pmset",
    "caffeinate",
    "ps aux | grep",
    "curl -s http://",
    "curl http://",
)


def _is_safe_command(cmd: str) -> bool:
    """Check if a command is safe to execute (read-only, no side effects)."""
    if not cmd or len(cmd) > 500:
        return False

    # Check for dangerous patterns
    dangerous = [
        ">",
        "|",
        ";",
        "&",
        "&&",
        "||",
        "sudo",
        "chmod",
        "chown",
        "rm ",
        "rm -",
        "mv ",
        "cp ",
        "mkdir",
        "touch",
        "ln ",
        "pip install",
        "npm install",
        "cargo install",
        "git push",
        "git commit",
        "git add",
        "kill ",
        "killall",
        "--help",  # prevent banner-based exploits
    ]
    cmd_lower = cmd.lower()
    for d in dangerous:
        if d in cmd_lower:
            return False

    # Allow exact match of safe commands
    if cmd in SAFE_COMMANDS:
        return True

    # Allow safe prefixes
    for prefix in ALLOWED_PREFIXES:
        if cmd.startswith(prefix):
            return True

    return False


def tool_run_command(command: str) -> str:
    """Run a safe read-only shell command."""
    if not _is_safe_command(command):
        return f"Command rejected — '{command[:50]}' is not in the allowed list."

    try:
        env = _get_subprocess_env()
        result = subprocess.run(
            command,
            shell=True,
            capture_output=True,
            text=True,
            timeout=15,
            env=env,
        )
        output = result.stdout.strip() or result.stderr.strip()
        if not output:
            output = "(command produced no output)"
        if len(output) > 3000:
            output = output[:3000] + "\n... (truncated)"
        return output
    except subprocess.TimeoutExpired:
        return "Command timed out."
    except Exception as e:
        return f"Command error: {e}"


def tool_superbrain_status() -> str:
    """Check Superbrain health."""
    try:
        from core.superbrain.superbrain import Superbrain

        sb = Superbrain()
        status = sb.status()
        parts = []
        for k, v in status.items():
            if isinstance(v, dict):
                parts.append(f"{k}:")
                for sk, sv in v.items():
                    parts.append(f"  {sk}: {sv}")
            else:
                parts.append(f"{k}: {v}")
        return "Superbrain status:\n" + "\n".join(parts)
    except Exception as e:
        log.warning(f"superbrain_status failed: {e}")
        return f"Superbrain status error: {e}"


def tool_browse_url(url: str, query: str = "summary") -> str:
    """Fetch and summarize a web page."""
    if not url.startswith(("http://", "https://")):
        return "Invalid URL — must start with http:// or https://"

    try:
        import httpx

        headers = {
            "User-Agent": "Mozilla/5.0 (compatible; Harvey/1.0; +http://harvey.ai)",
            "Accept": "text/html,application/xhtml+xml",
        }
        with httpx.Client(timeout=30.0, follow_redirects=True) as client:
            resp = client.get(url, headers=headers)
            if resp.status_code != 200:
                return f"Failed to fetch URL: HTTP {resp.status_code}"

            text = resp.text

        from html.parser import HTMLParser

        class TextExtractor(HTMLParser):
            def __init__(self):
                super().__init__()
                self.text = []
                self.skip = False
                self.ignore_tags = {"script", "style", "nav", "footer", "header"}

            def handle_starttag(self, tag, attrs):
                if tag in self.ignore_tags:
                    self.skip = True

            def handle_endtag(self, tag):
                if tag in self.ignore_tags:
                    self.skip = False

            def handle_data(self, data):
                if not self.skip:
                    stripped = data.strip()
                    if stripped:
                        self.text.append(stripped)

        extractor = TextExtractor()
        try:
            extractor.feed(text)
        except Exception:
            pass

        content = " ".join(extractor.text)
        content = " ".join(content.split())

        if not content:
            return f"Could not extract content from {url}"

        if query and query != "summary":
            if query.lower() in content.lower():
                idx = content.lower().find(query.lower())
                snippet = content[max(0, idx - 200) : idx + 500]
                return f"Found '{query}' on {url}:\n\n{snippet}\n\n[...]\n\nFull page: {url}"

        preview = content[:2000]
        more = "[... content truncated ...]" if len(content) > 2000 else ""
        return f"Content from {url}:\n\n{preview}\n\n{more}\n\nURL: {url}"

    except httpx.TimeoutException:
        # httpx.Timeout is a CONFIG class, not an exception — catching
        # it raises TypeError at runtime. The actual exception class is
        # TimeoutException (or the generic HTTPError). This bug was
        # silently killing browse_url for months until the empty-except
        # below masked it as "Failed to fetch".
        return f"Timed out fetching {url}"
    except httpx.HTTPError as e:
        return f"HTTP error fetching {url}: {type(e).__name__}: {e}"
    except Exception as e:
        return f"Failed to fetch {url}: {type(e).__name__}: {e}"


def tool_set_telegram_profile_photo(photo_path: str) -> str:
    """Set a chat's photo (group/channel). Note: Telegram bots CANNOT change their own profile photo via API — only @BotFather can."""
    import json

    photo_path = os.path.expanduser(photo_path)
    if not os.path.exists(photo_path):
        return f"File not found: {photo_path}"

    try:
        from core.chat.config import load_config

        cfg = load_config()
        bot_token = cfg.telegram.bot_token
        if not bot_token:
            return "No Telegram bot token configured."
    except Exception:
        return "Could not load Telegram config."

    try:
        import httpx

        with open(photo_path, "rb") as f:
            photo_data = f.read()

        ext = os.path.splitext(photo_path)[1].lower()
        mime_types = {".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".png": "image/png"}
        mime = mime_types.get(ext, "image/png")

        with httpx.Client(timeout=60.0) as client:
            form = {"photo": (os.path.basename(photo_path), photo_data, mime)}
            resp = client.post(
                f"https://api.telegram.org/bot{bot_token}/setChatPhoto",
                files=form,
                data={"chat_id": cfg.telegram.allowed_user_ids[0]},
            )

        if resp.status_code == 200 and resp.json().get("ok"):
            return f"Chat photo set successfully from {photo_path}"
        else:
            err = resp.json().get("description", resp.text[:200])
            return f"Failed to set chat photo: {err}"

    except Exception as e:
        return f"Error setting chat photo: {e}"


def tool_generate_image(
    prompt: str, save_path: str = "", aspect_ratio: str = "1:1"
) -> str:
    """Generate an image using MiniMax image-01 via switchAILocal."""
    import base64

    if not save_path:
        harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        save_path = f"{harvey_home}/data/chat/generated.png"

    save_path = os.path.expanduser(save_path)
    dir_path = os.path.dirname(save_path)
    if dir_path and not os.path.exists(dir_path):
        os.makedirs(dir_path, exist_ok=True)

    switchai_url = os.environ.get("SWITCHAI_URL", "http://localhost:18080/v1")
    switchai_key = os.environ.get("SWITCHAI_KEY", "sk-test-123")

    payload = {
        "model": "minimax:image-01",
        "prompt": prompt,
        "aspect_ratio": aspect_ratio,
        "response_format": "base64",
    }

    headers = {
        "Authorization": f"Bearer {switchai_key}",
        "Content-Type": "application/json",
    }

    try:
        r = requests.post(
            f"{switchai_url}/images/generations",
            headers=headers,
            json=payload,
            timeout=120,
        )

        if r.status_code != 200:
            return f"Image generation failed: HTTP {r.status_code} — {r.text[:200]}"

        data = r.json()
        resp_data = data.get("data", {})

        if isinstance(resp_data, dict):
            image_b64 = resp_data.get("image_base64", [None])[0]
        elif isinstance(resp_data, list):
            image_b64 = resp_data[0].get("base64_image", "") if resp_data else ""
        else:
            image_b64 = ""

        if not image_b64:
            return f"No image in response: {r.text[:200]}"

        image_data = base64.b64decode(image_b64)
        Path(save_path).write_bytes(image_data)

        size_kb = len(image_data) // 1024
        return f"Image saved to {save_path} ({size_kb}KB). Add [[SEND_PHOTO:{save_path}]] to your response to send it."

    except Exception as e:
        return f"Image generation error: {e}"


def tool_superbrain_vector_search(query: str) -> str:
    """Semantic search using vector embeddings from auto-memory indexing."""
    try:
        from core.superbrain.superbrain import Superbrain

        sb = Superbrain()
        result = sb.query(query, synthesize=False, top_k=5)
        if result.sources:
            parts = ["Semantic search results (vector-based):"]
            for i, s in enumerate(result.sources[:5], 1):
                score = getattr(s, 'score', 'n/a')
                parts.append(f"{i}. {s.title} (relevance: {score:.2f}): {s.text[:300]}")
            return "\n".join(parts)
        return f"No semantic results found for '{query}'."
    except Exception as e:
        log.warning(f"superbrain_vector_search failed: {e}")
        return f"Semantic search error: {e}"


def tool_get_consolidated_insights(limit: int = 5) -> str:
    """Get today's consolidated insights from the memory system."""
    try:
        from pathlib import Path
        from datetime import datetime

        journals_dir = Path(HARVEY_HOME) / "data" / "Brain" / "journals"
        today = datetime.now().strftime("%Y_%m_%d")
        journal_path = journals_dir / f"{today}.md"

        if not journal_path.exists():
            return "No journal entries found for today yet."

        content = journal_path.read_text()
        lines = content.split("\n")

        # Look for consolidation entries (from SANCHO)
        consolidation_entries = []
        for i, line in enumerate(lines):
            if "consolidation" in line.lower() or "insight" in line.lower():
                consolidation_entries.append(line.strip())

        # Also look for key decision/milestone entries
        key_entries = []
        for line in lines:
            if any(keyword in line.lower() for keyword in ["decided", "decision", "learned", "milestone", "completed", "shipped"]):
                if line.strip().startswith("- "):
                    key_entries.append(line.strip())

        all_insights = consolidation_entries + key_entries[:limit]

        if all_insights:
            return "Today's consolidated insights:\n" + "\n".join(all_insights[:limit])
        return "No consolidated insights recorded yet today."

    except Exception as e:
        log.warning(f"get_consolidated_insights failed: {e}")
        return f"Insights retrieval error: {e}"


def tool_get_emails_filtered(count: int = 5) -> str:
    """Fetch emails with intelligent filtering for job-related content."""
    count = min(count or 5, 10)

    # Keywords that indicate important (job-related) emails
    IMPORTANT_KEYWORDS = [
        "job", "offer", "position", "recruiter", "interview",
        "salary", "compensation", "client", "project", "proposal",
        "opportunity", "hiring", "applications", "resume", "cv",
        "contract", "engagement", "consulting"
    ]

    # Keywords to filter out (noise)
    FILTER_OUT_KEYWORDS = [
        "unsubscribe", "promotional", "marketing", "newsletter",
        "notification", "alert", "update", "reminder", "automated",
        "noreply", "do-not-reply"
    ]

    try:
        env = _get_subprocess_env()
        gws_cmd = shutil.which("gws") or "gws"

        # Fetch emails with some noise filtering
        result = subprocess.run(
            [
                gws_cmd,
                "gmail",
                "users",
                "messages",
                "list",
                "--params",
                json.dumps({
                    "userId": "me",
                    "maxResults": count * 2,  # Fetch more to filter
                    "q": "-from:noreply -from:notifications -label:promotions"
                }),
                "--format",
                "json",
            ],
            capture_output=True,
            text=True,
            timeout=15,
            env=env,
        )

        if result.returncode != 0:
            return f"Gmail fetch failed: {result.stderr[:200]}"

        data = json.loads(result.stdout)
        messages = data.get("messages", [])

        if not messages:
            return "No recent emails found."

        # Process and score emails
        scored_emails = []
        for msg in messages[:count * 2]:
            msg_id = msg["id"]
            subresult = subprocess.run(
                [
                    gws_cmd,
                    "gmail",
                    "users",
                    "messages",
                    "get",
                    "--params",
                    json.dumps({"userId": "me", "id": msg_id}),
                    "--format",
                    "json",
                ],
                capture_output=True,
                text=True,
                timeout=15,
                env=env,
            )

            if subresult.returncode != 0:
                continue

            try:
                m = json.loads(subresult.stdout)
                m = m.get("message", m)
                headers = {
                    h["name"].lower(): h["value"]
                    for h in m.get("payload", {}).get("headers", [])
                }
                sender = headers.get("from", "Unknown")
                subject = headers.get("subject", "(no subject)").lower()
                snippet = m.get("snippet", "")[:150]

                # Calculate importance score
                score = 0
                subject_text = subject.lower()
                snippet_text = snippet.lower()
                combined_text = subject_text + " " + snippet_text

                # Boost score for important keywords
                for keyword in IMPORTANT_KEYWORDS:
                    if keyword in combined_text:
                        score += 2

                # Reduce score for filter keywords
                for keyword in FILTER_OUT_KEYWORDS:
                    if keyword in combined_text:
                        score -= 3

                # Boost for known senders (learning from history)
                if "@linkedin.com" in sender or "@github.com" in sender:
                    score += 1

                scored_emails.append({
                    "sender": sender,
                    "subject": subject,
                    "snippet": snippet,
                    "score": score
                })
            except Exception:
                continue

        # Sort by score (descending)
        scored_emails.sort(key=lambda x: x["score"], reverse=True)

        # Return top N emails
        result_lines = []
        for email in scored_emails[:count]:
            importance = "🔴" if email["score"] >= 2 else "🟡" if email["score"] >= 0 else "⚪"
            result_lines.append(
                f"{importance} From: {email['sender']}\n"
                f"   Subject: {email['subject']}\n"
                f"   Preview: {email['snippet']}"
            )

        if result_lines:
            return f"Recent emails (filtered & prioritized):\n" + "\n".join(result_lines)
        return "No emails found after filtering."

    except FileNotFoundError:
        return "gws CLI not found. Cannot fetch emails."
    except subprocess.TimeoutExpired:
        return "Gmail fetch timed out."
    except Exception as e:
        log.warning(f"get_emails_filtered failed: {e}")
        return f"Email fetch error: {e}"


# Tool dispatch table
# ═══════════════════════════════════════════════════════════════
#  spawn_subagent — in-process delegation to the existing swarm
# ═══════════════════════════════════════════════════════════════
#
# This tool is Phase 6 of the cognitive core sprint. It connects
# harvey_agent.py's tool loop (what Telegram users actually talk to)
# to the existing AgentCoordinator + 6 built-in subagents
# (researcher/synthesizer/storage/image_gen/task_master/olibia).
#
# Key design:
#   - In-process, NOT subprocess (per sprint A.1: openclaw + pi-mono
#     both keep subagents in-process with session-key isolation)
#   - 120s wall-clock timeout per invocation (per sprint D.10)
#   - Best-effort — subagent failures are caught and returned as
#     tool result strings so the LLM can decide to retry / give up
#   - No access control enforcement at this layer — the individual
#     Subagent.tool() method already consults the AgentAccessControl
#     singleton before any inner tool call

_SUBAGENT_TIMEOUT_SECONDS = 120


class _SpawnStep:
    """Duck-typed Step stub passed to Subagent.handle() — emulates the
    shape the DAG executor normally produces."""
    __slots__ = ("id", "action", "agent", "task")

    def __init__(self, action: str, session_key: str):
        self.id = session_key
        self.action = action
        self.agent = ""
        self.task = ""


def tool_spawn_subagent(agent_name: str, task: str, action: str = "") -> str:
    """Delegate a task to a named in-process subagent.

    Args:
        agent_name: Name of a registered subagent (researcher / synthesizer /
                    storage / image_gen / task_master / olibia). Discovered
                    via AgentRegistry which reads core.subagents class attrs.
        task: The task text for the subagent (becomes ctx['query']/'topic').
        action: Optional subagent action. Defaults to the subagent's first
                declared ACTION.

    Returns a human-readable string summarizing what the subagent returned.
    Never raises — errors are formatted into the return string so the LLM
    loop can react.
    """
    import concurrent.futures
    import uuid as _uuid

    if not agent_name:
        return "Tool spawn_subagent error: agent_name is required"
    if not task:
        return "Tool spawn_subagent error: task is required"

    # Lazy singleton lookup — avoids import cycles and lets the rest of
    # Harvey boot without the orchestration package when it's not needed
    try:
        from core.orchestration.agent_coordinator import AgentCoordinator
        from core.orchestration.artifact_store import get_default_store
        from core.orchestration.persistent_event_bus import get_default_bus
    except Exception as e:
        return f"Tool spawn_subagent error: agent coordinator not available: {e}"

    # Module-level cached coordinator so we don't rebuild on every call
    global _SPAWN_SUBAGENT_COORDINATOR
    coordinator = globals().get("_SPAWN_SUBAGENT_COORDINATOR")
    if coordinator is None:
        try:
            coordinator = AgentCoordinator(
                artifact_store=get_default_store(),
                event_bus=get_default_bus(),
            )
            coordinator.register_all_default()
            globals()["_SPAWN_SUBAGENT_COORDINATOR"] = coordinator
        except Exception as e:
            return f"Tool spawn_subagent error: coordinator init failed: {e}"

    subagent = coordinator.get(agent_name)
    if subagent is None:
        available = ", ".join(coordinator.list_agents()) or "(none)"
        return (
            f"Tool spawn_subagent error: unknown agent '{agent_name}'. "
            f"Available: {available}"
        )

    # Resolve action — use explicit, else the subagent's first ACTION
    actions = list(getattr(subagent, "ACTIONS", []) or subagent.actions())
    chosen_action = action or (actions[0] if actions else "default")
    if action and action not in actions:
        return (
            f"Tool spawn_subagent error: agent '{agent_name}' does not "
            f"support action '{action}'. Supported: {actions}"
        )

    session_key = f"spawn:{agent_name}:{_uuid.uuid4().hex[:8]}"
    step = _SpawnStep(action=chosen_action, session_key=session_key)
    ctx = {
        "query": task,
        "topic": task,
        "task": task,
        "workflow_id": session_key,
    }

    log.info(
        f"[spawn_subagent] invoking {agent_name}/{chosen_action} "
        f"session={session_key} task={task[:80]!r}"
    )

    # SPRINT-HARVEY-TICKETING Phase 5: wrap subagent.handle so every
    # TaskEntry appended on the worker thread inherits the agent name as
    # its `actor` via the store's thread-local context. The wrapper sets
    # the thread-local on entry and clears it in a `finally` so crashes
    # don't leak the actor into unrelated work on thread-pool reuse.
    try:
        from core.tasks.store import clear_current_actor, set_current_actor
    except Exception:
        # If core.tasks isn't importable (e.g. early boot), just don't
        # tag actor — the rest of spawn_subagent still works fine.
        set_current_actor = None  # type: ignore
        clear_current_actor = None  # type: ignore

    def _run_with_actor(_sub=subagent, _step=step, _ctx=ctx, _name=agent_name):
        if set_current_actor is not None:
            set_current_actor(_name)
        try:
            return _sub.handle(_step, _ctx)
        finally:
            if clear_current_actor is not None:
                clear_current_actor()

    # Run in a thread with a wall-clock timeout so a hung subagent cannot
    # block the LLM tool loop forever. Subagent.handle() is sync so this
    # is the simplest safe approach.
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as ex:
            future = ex.submit(_run_with_actor)
            try:
                result = future.result(timeout=_SUBAGENT_TIMEOUT_SECONDS)
            except concurrent.futures.TimeoutError:
                return (
                    f"Tool spawn_subagent timeout: {agent_name}/{chosen_action} "
                    f"exceeded {_SUBAGENT_TIMEOUT_SECONDS}s"
                )
    except Exception as e:
        log.error(f"[spawn_subagent] {agent_name} crashed: {e}", exc_info=True)
        return (
            f"Tool spawn_subagent error: {agent_name}/{chosen_action} crashed: "
            f"{type(e).__name__}: {e}"
        )

    # Summarize the result dict for the LLM — keep it concise, don't
    # blow up the context window with huge findings blobs
    if not isinstance(result, dict):
        summary = f"[OK] {str(result)[:1500]}"
    else:
        ok = result.get("ok", True)
        # Try to extract the most useful field
        for key in ("findings", "summary", "result", "output", "plan", "message"):
            if key in result:
                val = result[key]
                if isinstance(val, (dict, list)):
                    import json as _json
                    summary = _json.dumps(val, default=str)[:1500]
                else:
                    summary = str(val)[:1500]
                break
        else:
            import json as _json
            summary = _json.dumps(result, default=str)[:1500]
        prefix = "OK" if ok else "ERROR"
        summary = f"[{prefix}] {summary}"

    log.info(f"[spawn_subagent] {agent_name} returned {len(summary)} chars")
    return summary


TOOL_DISPATCH = {
    "brain_search": lambda args: tool_brain_search(args.get("query", "")),
    "brain_write": lambda args: tool_brain_write(args.get("content", "")),
    "skill_discover": lambda args: tool_skill_discover(args.get("query", "")),
    "get_emails": lambda args: tool_get_emails(args.get("count", 5)),
    "get_calendar": lambda args: tool_get_calendar(),
    "superbrain_status": lambda args: tool_superbrain_status(),
    "run_command": lambda args: tool_run_command(args.get("command", "")),
    "browse_url": lambda args: tool_browse_url(
        args.get("url", ""), args.get("query", "summary")
    ),
    "set_telegram_profile_photo": lambda args: tool_set_telegram_profile_photo(
        args.get("photo_path", "")
    ),
    "generate_image": lambda args: tool_generate_image(
        args.get("prompt", ""),
        args.get("save_path", "~/pics/generated.png"),
        args.get("aspect_ratio", "1:1"),
    ),
    "superbrain_vector_search": lambda args: tool_superbrain_vector_search(args.get("query", "")),
    "get_consolidated_insights": lambda args: tool_get_consolidated_insights(args.get("limit", 5)),
    "get_emails_filtered": lambda args: tool_get_emails_filtered(args.get("count", 5)),
    "write_file": lambda args: tool_write_file(
        args.get("path", ""), args.get("content", "")
    ),
    "markdown_to_pdf": lambda args: tool_markdown_to_pdf(
        args.get("md_path", ""), args.get("pdf_path", "")
    ),
    "spawn_subagent": lambda args: tool_spawn_subagent(
        args.get("agent_name", ""),
        args.get("task", ""),
        args.get("action", ""),
    ),
}


def execute_tool(name: str, arguments: dict) -> str:
    """Execute a tool by name with given arguments. Always returns a string."""
    fn = TOOL_DISPATCH.get(name)
    if not fn:
        return f"Unknown tool: {name}"
    try:
        return fn(arguments)
    except Exception as e:
        log.error(f"Tool {name} crashed: {e}", exc_info=True)
        return f"Tool {name} error: {e}"


# ═══════════════════════════════════════════════════════════════
#  Harvey Agent — Agentic Loop
# ═══════════════════════════════════════════════════════════════


class HarveyAgent:
    """
    Reusable Harvey agent core with tool-calling loop.

    Every Harvey body (Telegram, WhatsApp, CLI, Robot) instantiates this
    and calls .process() to get agentic responses.
    """

    def __init__(
        self,
        llm_url: str = "http://localhost:18080/v1",
        llm_model: str = "auto",
        api_key: str = "",
        max_tokens: int = 4096,
    ):
        self.llm_url = llm_url.rstrip("/")
        self.llm_model = llm_model
        self.api_key = (
            api_key
            or os.environ.get("SWITCHAI_KEY", "")
            or os.environ.get("LLM_API_KEY", "")
        )
        self.max_tokens = max_tokens

    def process(
        self,
        message: str,
        history: List[Dict],
        system_prompt: str = "",
        channel: str = "unknown",
        task_id: Optional[str] = None,
        store: Optional[Any] = None,
    ) -> str:
        """
        Process a message through the agentic tool-calling loop.

        1. Build messages with system prompt + history + tools
        2. Call LLM with tools
        3. If LLM returns tool_calls -> execute them -> feed results back
        4. Repeat up to MAX_TOOL_ROUNDS
        5. Return final text response

        Cognitive core params (optional, backward-compatible):
            task_id: If provided, all turns + tool calls are checkpointed
                     to the given task via `store.append_entry()`.
            store:   A TaskStore instance used for checkpointing and
                     artifact recording. Ignored if task_id is None.
        """
        # Build message list
        messages = []
        if system_prompt:
            messages.append({"role": "system", "content": system_prompt})

        # Add history
        for msg in history:
            messages.append({"role": msg["role"], "content": msg["content"]})

        # Add current message if not already last
        if not history or history[-1].get("content") != message:
            messages.append({"role": "user", "content": message})

        # Try tool-calling loop
        response = self._tool_calling_loop(messages, task_id=task_id, store=store)
        if response is not None:
            return response

        # Fallback: prompt-based approach (no tool-calling support)
        log.info("Tool-calling unavailable, falling back to prompt-based approach")
        return self._prompt_based_fallback(message, messages)

    def _tool_calling_loop(
        self,
        messages: List[Dict],
        task_id: Optional[str] = None,
        store: Optional[Any] = None,
    ) -> Optional[str]:
        """
        Run the agentic loop with OpenAI function-calling format.
        Returns None if tool-calling is not supported by the provider.
        """
        # Work on a copy so we can append tool results
        msgs = [m.copy() for m in messages]

        # Lazy import so we don't force the tasks package on paths that
        # never use checkpointing (legacy HarveyChat, unit tests that
        # pass task_id=None, etc.)
        checkpoint_enabled = task_id is not None and store is not None
        _TaskEntry = None
        _TaskArtifact = None
        _event_bus = None
        if checkpoint_enabled:
            try:
                from core.tasks.models import TaskEntry as _TE, TaskArtifact as _TA
                _TaskEntry = _TE
                _TaskArtifact = _TA
            except Exception as e:
                log.warning(f"[agent] task checkpoint imports failed, disabling: {e}")
                checkpoint_enabled = False
            # Event bus — best-effort. Failure here does NOT disable
            # checkpointing; it just disables event publishing.
            try:
                from core.orchestration.persistent_event_bus import get_default_bus
                _event_bus = get_default_bus()
            except Exception as e:
                log.warning(f"[agent] event bus unavailable, publishing disabled: {e}")
                _event_bus = None

        def _publish(topic: str, **payload) -> None:
            """Best-effort event publish. Never raises."""
            if _event_bus is None:
                return
            try:
                _event_bus.publish(topic, source="harvey-agent", task_id=task_id, **payload)
            except Exception as e:
                log.warning(f"[agent] publish({topic}) failed: {e}")

        # Tool loop detection (OpenClaw-inspired): abort if agent repeats
        # the same tool call 5+ times — it's stuck in a loop.
        _recent_tool_calls: List[str] = []
        _LOOP_THRESHOLD = 5

        for round_num in range(MAX_TOOL_ROUNDS):
            log.info(f"Agent loop round {round_num + 1}/{MAX_TOOL_ROUNDS}")

            # Heartbeat: let the cron resumer know we're still alive
            if checkpoint_enabled:
                try:
                    store.touch(task_id)
                except Exception as e:
                    log.warning(f"[agent] store.touch failed (non-fatal): {e}")

            _publish("agent.turn_start", round=round_num + 1, max_rounds=MAX_TOOL_ROUNDS)

            # Last-ditch coaching: when we're close to exhausting rounds,
            # inject a system message telling the model to STOP gathering
            # and START producing a deliverable with whatever it has. This
            # prevents the "spent all 12 rounds browsing, no file written"
            # failure mode. Only fires on the last 3 rounds.
            rounds_left = MAX_TOOL_ROUNDS - round_num
            if rounds_left <= 5 and not any(
                m.get("role") == "system" and "STOP GATHERING" in m.get("content", "")
                for m in msgs
            ):
                msgs.append({
                    "role": "system",
                    "content": (
                        "STOP GATHERING. You have used most of your tool rounds. "
                        "Do NOT call browse_url or brain_search again. "
                        "With whatever you have gathered so far, IMMEDIATELY call "
                        "`write_file` with a markdown report at "
                        "`~/MAKAKOO/data/reports/report_<topic>.md`, then "
                        "`markdown_to_pdf` on that file, then emit "
                        "`[[SEND_FILE:<the pdf path>]]`. Do not respond with text "
                        "unless it's after SEND_FILE. Execute the tools."
                    ),
                })

            response_data = self._call_llm(msgs, include_tools=True)
            if response_data is None:
                return None  # LLM unreachable — caller should try fallback

            choice = response_data.get("choices", [{}])[0]
            msg = choice.get("message", {})
            finish_reason = choice.get("finish_reason", "")

            # Check if model returned tool calls
            tool_calls = msg.get("tool_calls")

            if not tool_calls or finish_reason == "stop":
                # No tool calls — return text response
                content = msg.get("content", "")
                # Checkpoint the final assistant message (no tool calls)
                if checkpoint_enabled and content:
                    try:
                        store.append_entry(
                            _TaskEntry.message(task_id, "assistant", content)
                        )
                    except Exception as e:
                        log.warning(f"[agent] final assistant checkpoint failed: {e}")
                _publish(
                    "agent.turn_end",
                    round=round_num + 1,
                    stop_reason=finish_reason or "stop",
                    content_length=len(content or ""),
                )
                if content:
                    return content
                # Empty content with no tool calls — something went wrong
                if not tool_calls:
                    return content or "(No response generated)"

            # Execute tool calls
            # First, append the assistant message with tool calls
            assistant_content = msg.get("content") or ""
            assistant_msg = {"role": "assistant", "content": assistant_content}
            if tool_calls:
                assistant_msg["tool_calls"] = tool_calls
            msgs.append(assistant_msg)

            # Checkpoint the assistant turn (may be empty when it's just tool calls)
            if checkpoint_enabled:
                try:
                    store.append_entry(
                        _TaskEntry.message(task_id, "assistant", assistant_content)
                    )
                except Exception as e:
                    log.warning(f"[agent] assistant turn checkpoint failed: {e}")

            for tc in tool_calls:
                tool_name = tc.get("function", {}).get("name", "")
                tool_call_id = tc.get("id", f"call_{tool_name}_{round_num}")

                # Parse arguments
                raw_args = tc.get("function", {}).get("arguments", "{}")
                try:
                    args = (
                        json.loads(raw_args) if isinstance(raw_args, str) else raw_args
                    )
                except json.JSONDecodeError:
                    args = {}

                # Tool loop detection: abort if repeating same call
                call_sig = f"{tool_name}:{json.dumps(args, sort_keys=True, default=str)[:200]}"
                _recent_tool_calls.append(call_sig)
                if len(_recent_tool_calls) >= _LOOP_THRESHOLD:
                    recent = _recent_tool_calls[-_LOOP_THRESHOLD:]
                    if len(set(recent)) == 1:
                        log.warning(f"Tool loop detected: {tool_name} called {_LOOP_THRESHOLD}x with same args — aborting")
                        return f"I got stuck in a loop calling {tool_name}. Here's what I found so far: {assistant_content or '(no partial result)'}"

                # Checkpoint the tool call BEFORE execution — so a crash mid-tool
                # leaves a reconstructable trail. Captures args even if the tool
                # hangs or never returns.
                tc_entry_id: Optional[str] = None
                if checkpoint_enabled:
                    try:
                        tc_entry = store.append_entry(
                            _TaskEntry.tool_call(task_id, tool_name, args)
                        )
                        tc_entry_id = tc_entry.id
                    except Exception as e:
                        log.warning(f"[agent] tool_call checkpoint failed: {e}")

                _publish(
                    "tool.called",
                    tool=tool_name,
                    args_preview=json.dumps(args, default=str)[:200],
                )

                log.info(f"Executing tool: {tool_name}({json.dumps(args)[:100]})")
                result = execute_tool(tool_name, args)
                log.info(f"Tool {tool_name} result: {len(result)} chars")

                # Checkpoint tool result + artifact detection
                is_error = False
                if checkpoint_enabled:
                    is_error = result.startswith(f"Tool {tool_name} error:") or \
                               result.startswith("Unknown tool:")
                    try:
                        store.append_entry(
                            _TaskEntry.tool_result(
                                task_id,
                                tool_name,
                                result,
                                parent_entry_id=tc_entry_id or "",
                                is_error=is_error,
                            )
                        )
                    except Exception as e:
                        log.warning(f"[agent] tool_result checkpoint failed: {e}")

                    # Artifact detection — record the file the tool just produced
                    # so it's durably associated with the task even if the LLM
                    # never emits [[SEND_FILE:...]]
                    if not is_error:
                        artifact_path = self._extract_artifact_path(tool_name, result)
                        if artifact_path:
                            try:
                                kind = "pdf" if artifact_path.endswith(".pdf") else "file"
                                mime = "application/pdf" if kind == "pdf" else ""
                                size = os.path.getsize(artifact_path) if os.path.exists(artifact_path) else None
                                if kind == "pdf":
                                    art = _TaskArtifact.pdf(task_id, artifact_path, size)
                                else:
                                    art = _TaskArtifact.file(task_id, artifact_path, mime=mime, size_bytes=size)
                                store.record_artifact(art)
                                log.info(f"[agent] recorded artifact: {artifact_path}")
                                _publish(
                                    "task.artifact_created",
                                    tool=tool_name,
                                    kind=kind,
                                    path=artifact_path,
                                    size_bytes=size,
                                )
                            except Exception as e:
                                log.warning(f"[agent] record_artifact failed: {e}")

                _publish(
                    "tool.error" if is_error else "tool.result",
                    tool=tool_name,
                    ok=not is_error,
                    result_length=len(result),
                    summary=result[:200],
                )

                # Truncate oversized tool results before adding to context
                # (OpenClaw caps at 40k; we cap at 4k for smaller model context)
                if len(result) > 4000:
                    head = result[:3000]
                    tail = result[-500:]
                    result = (
                        f"{head}\n\n[... {len(result) - 3500} chars truncated ...]\n\n{tail}"
                    )

                # Append tool result to messages for the next LLM turn
                msgs.append(
                    {
                        "role": "tool",
                        "content": result,
                        "tool_call_id": tool_call_id,
                    }
                )

        # Exhausted rounds — get final response without tools
        log.warning("Max tool rounds exhausted, getting final response")
        response_data = self._call_llm(msgs, include_tools=False)
        if response_data:
            content = (
                response_data.get("choices", [{}])[0]
                .get("message", {})
                .get("content", "")
            )
            final = content or "(Agent exhausted tool rounds without a final answer)"
            if checkpoint_enabled:
                try:
                    store.append_entry(_TaskEntry.message(task_id, "assistant", final))
                except Exception as e:
                    log.warning(f"[agent] exhaustion checkpoint failed: {e}")
            return final
        return "(Agent loop failed)"

    @staticmethod
    def _extract_artifact_path(tool_name: str, result: str) -> Optional[str]:
        """Pull the output path out of a write_file or markdown_to_pdf result.

        tool_write_file returns: 'Wrote <N> bytes to <absolute-path>'
        tool_markdown_to_pdf returns: 'Converted <md> → <pdf> (<N> bytes)'

        Returns None if the tool_name is not file-producing or the format
        doesn't match. Silent on failure — artifact recording is best-effort.
        """
        if tool_name == "write_file":
            m = re.search(r"Wrote\s+\d+\s+bytes\s+to\s+(.+?)\s*$", result)
            if m:
                return m.group(1).strip()
        elif tool_name == "markdown_to_pdf":
            m = re.search(r"Converted\s+\S+\s+→\s+(.+?)\s*\(\d+\s*bytes\)", result)
            if m:
                return m.group(1).strip()
        elif tool_name == "generate_image":
            # Real format: "Image saved to /path/to/img.png (42KB). Add
            # [[SEND_PHOTO:...]] to your response to send it."
            # Capture only non-whitespace / non-paren / non-bracket chars so
            # we stop at the first space or opening paren after the path.
            m = re.search(r"[Ss]aved\s+(?:to|at)\s+([^\s()\[\]]+)", result)
            if m:
                return m.group(1).strip()
        return None

    def _call_llm(
        self, messages: List[Dict], include_tools: bool = True
    ) -> Optional[dict]:
        """Make a single LLM call. Returns parsed JSON response or None."""
        try:
            headers = {"Content-Type": "application/json"}
            if self.api_key:
                headers["Authorization"] = f"Bearer {self.api_key}"

            payload = {
                "model": self.llm_model,
                "messages": messages,
                "max_tokens": self.max_tokens,
                "temperature": 0.7,
                "stream": False,
            }

            if include_tools:
                payload["tools"] = HARVEY_TOOLS

            r = requests.post(
                f"{self.llm_url}/chat/completions",
                headers=headers,
                json=payload,
                timeout=120,
            )

            if r.status_code == 200:
                data = r.json()
                model = data.get("model", "?")
                log.info(f"LLM response: model={model}, status=200")
                return data
            else:
                log.warning(f"LLM returned {r.status_code}: {r.text[:300]}")
                # Some providers return 400 when they don't support tools
                if include_tools and r.status_code in (400, 422):
                    log.info("Provider may not support tool-calling, will fall back")
                return None

        except requests.exceptions.ConnectionError:
            log.warning("LLM not reachable at %s", self.llm_url)
            return None
        except Exception as e:
            log.warning(f"LLM call error: {e}")
            return None

    def _prompt_based_fallback(
        self, user_message: str, messages: List[Dict]
    ) -> Optional[str]:
        """
        Fallback when provider doesn't support tool-calling.

        Proactively gather context and inject it into the prompt,
        similar to the old bridge approach but smarter.
        """
        # Gather context proactively
        context_parts = []

        # Always search Brain for the user's message
        brain_result = tool_brain_search(user_message)
        if "No results" not in brain_result:
            context_parts.append(brain_result)

        # Check if email-related
        email_pattern = re.compile(
            r"(emails?|inbox|gmail|clients?|recruiters?|responses?|replies?|mail)",
            re.IGNORECASE,
        )
        if email_pattern.search(user_message):
            email_result = tool_get_emails(5)
            context_parts.append(email_result)

        # Check if calendar-related
        cal_pattern = re.compile(
            r"(calendar|schedule|meetings?|events?|today|agenda)",
            re.IGNORECASE,
        )
        if cal_pattern.search(user_message):
            cal_result = tool_get_calendar()
            context_parts.append(cal_result)

        # Inject context into system prompt
        if context_parts:
            context_block = "\n\n## Context (auto-fetched)\n" + "\n\n".join(
                context_parts
            )
            # Find system message and append context
            for i, m in enumerate(messages):
                if m["role"] == "system":
                    messages[i] = {
                        "role": "system",
                        "content": m["content"] + context_block,
                    }
                    break

        # Call without tools
        response_data = self._call_llm(messages, include_tools=False)
        if response_data:
            content = (
                response_data.get("choices", [{}])[0]
                .get("message", {})
                .get("content", "")
            )
            return content or None
        return None
