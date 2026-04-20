#!/usr/bin/env python3
"""
Harvey MCP Server — The parasite's nervous system.

This is Harvey's PRIMARY integration surface. One MCP server that exposes
Harvey's Brain, skills, sub-agents, and outbound chokepoint to ANY
MCP-capable CLI (Claude Code, Cursor, Gemini CLI, etc.).

Once this server is registered in a CLI's MCP config, that CLI becomes
a Harvey body. Zero per-CLI driver code. The parasite uses the body's
muscles — it just decides where they go.

What this exposes:
  - Brain: search, recent, write_journal, context_for, entity_neighbors
  - Wiki: lint, compile, index, save_answer
  - Skills: discover, invoke (170+ skills)
  - Sub-agents: career_manager, coordinator swarm
  - Companion: buddy status, nursery roll call, psych level
  - SANCHO: tick, status, dream
  - Outbound: draft only. Never sends. One throat to choke.
  - Costs: session and historical cost tracking

Install into Claude Code:
  claude mcp add harvey -- python3 ~/MAKAKOO/harvey-os/core/mcp/harvey_mcp.py

Install into any MCP client:
  {
    "mcpServers": {
      "harvey": {
        "command": "python3",
        "args": ["~/MAKAKOO/harvey-os/core/mcp/harvey_mcp.py"]
      }
    }
  }
"""

import json
import logging
import os
import sys
import time
from datetime import date, datetime
from pathlib import Path

# Add harvey-os to path
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
sys.path.insert(0, os.path.join(HARVEY_HOME, "harvey-os"))

log = logging.getLogger("harvey.mcp")


# ═══════════════════════════════════════════════════════════════
#  First-boot Olibia welcome (Phase 6 polish)
# ═══════════════════════════════════════════════════════════════
#
# On the very first MCP server boot per HARVEY_HOME, print an owl
# banner to stderr (visible in MCP host logs) and append a journal
# entry. Idempotent — gated by a flag file at data/.harvey_first_boot.
# Bulletproof against any error so server boot never fails on this.


def _first_boot_welcome() -> None:
    try:
        flag_dir = os.path.join(HARVEY_HOME, "data")
        flag_path = os.path.join(flag_dir, ".harvey_first_boot")
        if os.path.exists(flag_path):
            return
        os.makedirs(flag_dir, exist_ok=True)

        banner = (
            "\n"
            "  ╔══════════════════════════════════════════════════╗\n"
            "  ║   🦉  Olibia: Harvey MCP plugin first boot.       ║\n"
            "  ║       Brain warmed. Swarm staged. Ready.         ║\n"
            "  ╚══════════════════════════════════════════════════╝\n"
        )
        sys.stderr.write(banner)
        sys.stderr.flush()

        # Append to today's brain journal — best-effort
        try:
            from datetime import date
            today = date.today().strftime("%Y_%m_%d")
            journal_path = os.path.join(
                HARVEY_HOME, "data", "Brain", "journals", f"{today}.md",
            )
            os.makedirs(os.path.dirname(journal_path), exist_ok=True)
            with open(journal_path, "a") as f:
                f.write(
                    "\n- 🦉 [[Harvey MCP Plugin]] first boot — "
                    "Olibia online, brain + swarm wired into the host CLI\n"
                )
        except Exception as e:
            log.debug(f"first-boot journal write failed: {e}")

        # Touch the flag so we never re-emit
        with open(flag_path, "w") as f:
            f.write(str(int(time.time())))
    except Exception as e:
        log.debug(f"first-boot welcome failed: {e}")


_first_boot_welcome()


# ═══════════════════════════════════════════════════════════════
#  Swarm gateway (Phase 1 of MCP plugin sprint)
# ═══════════════════════════════════════════════════════════════
#
# A single module-level Subagent instance so every harvey_* tool call
# routes through AgentAccessControl + AuditLog + structured logging
# exactly like native swarm step handlers. Lazy-instantiated to keep
# MCP server init fast — the Subagent import pulls in artifact_store
# and the event bus.

_gateway_cache = None


def _gateway():
    """Return the lazy-loaded mcp_gateway Subagent."""
    global _gateway_cache
    if _gateway_cache is None:
        from core.subagents.subagent import Subagent
        _gateway_cache = Subagent(name="mcp_gateway")
    return _gateway_cache


_olibia_cache = None


def _olibia():
    """Return the lazy-loaded OlibiaAgent instance."""
    global _olibia_cache
    if _olibia_cache is None:
        from core.subagents.olibia_agent import OlibiaAgent
        _olibia_cache = OlibiaAgent(name="olibia")
    return _olibia_cache


_swarm_state_cache = None


def _swarm_state():
    """Return the lazy-loaded swarm runtime stack (engine, store, bus, executor, coordinator, router).

    Uses persistent SQLite databases under HARVEY_HOME/data so workflows
    survive MCP server restarts. One global stack per MCP server process.
    """
    global _swarm_state_cache
    if _swarm_state_cache is not None:
        return _swarm_state_cache

    from core.workflow.engine import WorkflowEngine
    from core.orchestration.artifact_store import ArtifactStore
    from core.orchestration.persistent_event_bus import PersistentEventBus
    from core.workflow.async_dag_executor import AsyncDAGExecutor
    from core.orchestration.agent_coordinator import AgentCoordinator
    from core.orchestration.intelligent_router import IntelligentRouter

    data_dir = os.path.join(HARVEY_HOME, "data")
    os.makedirs(data_dir, exist_ok=True)

    engine = WorkflowEngine(db_path=os.path.join(data_dir, "mcp_workflows.db"))
    store = ArtifactStore(db_path=os.path.join(data_dir, "artifacts.db"))
    bus = PersistentEventBus(db_path=os.path.join(data_dir, "events.db"))
    executor = AsyncDAGExecutor(
        engine=engine, artifact_store=store, event_bus=bus,
        max_concurrent_steps=16,
    )
    coordinator = AgentCoordinator(
        executor=executor, artifact_store=store, event_bus=bus,
    )
    coordinator.register_all_default()
    router = IntelligentRouter(default_parallelism=3)

    _swarm_state_cache = {
        "engine": engine,
        "store": store,
        "bus": bus,
        "executor": executor,
        "coordinator": coordinator,
        "router": router,
    }
    return _swarm_state_cache


# ═══════════════════════════════════════════════════════════════
#  Phase 2 helpers — audit shortcut + swarm + telegram
# ═══════════════════════════════════════════════════════════════


def _audit(agent: str, tool: str, outcome: str, detail: str = "", error: str = ""):
    """Best-effort audit log record. Silent if audit log not configured."""
    try:
        from core.security.audit_log import get_default_audit_log
        audit = get_default_audit_log()
        if audit is None:
            return
        kwargs = {"agent": agent, "tool": tool, "outcome": outcome}
        if detail:
            kwargs["detail"] = detail
        if error:
            kwargs["error"] = error
        audit.record_tool_call(**kwargs)
    except Exception as e:
        log.debug(f"audit record failed: {e}")


def _run_swarm(args: dict) -> str:
    """Execute harvey_swarm_run. Classifies → builds workflow → runs → returns summary."""
    import asyncio
    from core.orchestration.agent_team import build_workflow_from_team

    request = args["request"]
    plan_only = args.get("plan_only", False)
    timeout_s = float(args.get("timeout_s", 120))
    parallelism_override = args.get("parallelism")

    _audit("mcp_gateway", "harvey_swarm_run", "started",
           detail=f"plan_only={plan_only} timeout={timeout_s}")

    state = _swarm_state()
    router = state["router"]
    engine = state["engine"]
    executor = state["executor"]
    store = state["store"]

    cls, team = router.classify_and_route(request)
    if parallelism_override:
        team.default_parallelism = max(1, int(parallelism_override))

    wf = build_workflow_from_team(
        engine, team,
        context={"query": request, "initial_request": request},
    )

    plan_summary = {
        "classification": {
            "intent": cls.intent,
            "confidence": cls.confidence,
            "keywords_hit": cls.keywords_hit,
            "rationale": cls.rationale,
        },
        "team": {
            "name": team.name,
            "description": team.description,
            "parallelism": team.default_parallelism,
            "members": [
                {"agent": m.agent, "action": m.action, "role": m.role, "count": m.count}
                for m in team.members
            ],
        },
        "workflow": {
            "id": wf.id,
            "step_count": len(wf.steps),
            "steps": [
                {"id": s.id, "agent": s.agent, "action": s.action}
                for s in wf.steps
            ],
        },
    }

    if plan_only:
        _audit("mcp_gateway", "harvey_swarm_run", "ok", detail="plan_only")
        return json.dumps({"mode": "plan_only", **plan_summary}, indent=2, default=str)

    async def _run():
        return await asyncio.wait_for(
            executor.run_workflow(wf), timeout=timeout_s
        )

    try:
        asyncio.run(_run())
    except asyncio.TimeoutError:
        _audit("mcp_gateway", "harvey_swarm_run", "error",
               error=f"timeout after {timeout_s}s")
        return json.dumps({
            "mode": "timeout",
            "workflow_id": wf.id,
            "timeout_s": timeout_s,
            **plan_summary,
        }, indent=2, default=str)
    except Exception as e:
        _audit("mcp_gateway", "harvey_swarm_run", "error",
               error=f"{type(e).__name__}: {e}")
        return json.dumps({
            "mode": "error",
            "workflow_id": wf.id,
            "error": f"{type(e).__name__}: {e}",
            **plan_summary,
        }, indent=2, default=str)

    # Collect artifact summaries
    artifacts = {}
    for step in wf.steps:
        art = store.get(f"{wf.id}:{step.id}")
        if art is None:
            artifacts[step.id] = None
        else:
            payload = art.payload
            if isinstance(payload, dict):
                summary = {
                    k: (str(v)[:200] if not isinstance(v, (int, float, bool, type(None))) else v)
                    for k, v in payload.items()
                }
            else:
                summary = str(payload)[:400]
            artifacts[step.id] = summary

    _audit("mcp_gateway", "harvey_swarm_run", "ok",
           detail=f"wf={wf.id} state={wf.state}")

    return json.dumps({
        "mode": "executed",
        "workflow_id": wf.id,
        "workflow_state": str(wf.state),
        "step_states": {s.id: str(s.state) for s in wf.steps},
        "artifacts": artifacts,
        **plan_summary,
    }, indent=2, default=str)


def _swarm_status(workflow_id=None) -> str:
    """Return coordinator + breaker + recent workflows state."""
    state = _swarm_state()
    coordinator = state["coordinator"]
    engine = state["engine"]

    out = {
        "agents": sorted(coordinator._agents.keys()) if hasattr(coordinator, "_agents") else [],
        "agent_count": len(getattr(coordinator, "_agents", {})),
    }

    # Optional: circuit breaker state
    try:
        from core.orchestration.failure_recovery import FailureRecovery
        # FailureRecovery is a per-executor singleton; we don't wire it in
        # by default yet, so just report empty unless someone has hooked it.
        out["breakers"] = "not-wired-in-mcp-server"
    except Exception:
        pass

    # Recent workflows from the engine
    try:
        if workflow_id:
            wf = engine.load_workflow(workflow_id)
            if wf is None:
                out["workflow"] = None
            else:
                out["workflow"] = {
                    "id": wf.id,
                    "state": str(wf.state),
                    "steps": {s.id: str(s.state) for s in wf.steps},
                }
        else:
            recent = engine.list_workflows(limit=10) if hasattr(engine, "list_workflows") else []
            out["recent_workflows"] = [
                {"id": w.id, "state": str(w.state)} for w in recent
            ]
    except Exception as e:
        out["engine_error"] = str(e)

    return json.dumps(out, indent=2, default=str)


def _omni_describe(kind: str, args: dict) -> str:
    """Shared handler for harvey_describe_{image,audio,video} MCP tools.

    Routes through `core.llm.omni` to Xiaomi MiMo omni via switchAILocal.
    `source` may be a URL, a `data:` URI, or a local file path (which
    will be read + base64-encoded automatically). Returns a JSON string
    with the model's text answer + usage stats so MCP clients can parse.
    """
    from core.llm.omni import (
        OmniError,
        describe_audio,
        describe_image,
        describe_video,
        omni_chat,
        OmniClient,
    )

    source = args.get("source") or args.get("url") or args.get("path")
    prompt = args.get("prompt", "Describe the content in detail.")
    max_tokens = int(args.get("max_completion_tokens", 1024))

    tool_name = "harvey_describe_{}".format(kind)
    if not source:
        _audit("mcp_gateway", tool_name, "error", error="missing source")
        return json.dumps({"ok": False, "error": "source (url or path) is required"})

    try:
        if kind == "image":
            text = describe_image(source, prompt, max_completion_tokens=max_tokens)
        elif kind == "audio":
            text = describe_audio(source, prompt, max_completion_tokens=max_tokens)
        elif kind == "video":
            fps = args.get("fps")
            media_resolution = args.get("media_resolution")
            text = describe_video(
                source,
                prompt,
                fps=int(fps) if fps is not None else None,
                media_resolution=media_resolution,
                max_completion_tokens=max_tokens,
            )
        else:
            _audit("mcp_gateway", tool_name, "error", error="unknown kind")
            return json.dumps({"ok": False, "error": "unknown kind: {}".format(kind)})
    except OmniError as exc:
        _audit("mcp_gateway", tool_name, "error", error=str(exc)[:200])
        return json.dumps({"ok": False, "error": "{}: {}".format(type(exc).__name__, exc)})

    _audit(
        "mcp_gateway",
        tool_name,
        "ok",
        detail="source={} len={}".format(str(source)[:80], len(text or "")),
    )
    return json.dumps({"ok": True, "kind": kind, "text": text}, indent=2)


def _telegram_send(args: dict) -> str:
    """Send a Telegram message via Harvey's configured bot token."""
    from core.chat.config import load_config as _load_chat_config
    import requests as _req

    cfg = _load_chat_config()
    if not cfg.telegram.bot_token:
        _audit("mcp_gateway", "harvey_telegram_send", "error", error="bot_token missing")
        return json.dumps({"ok": False, "error": "Telegram bot_token not configured"})

    chat_id = args.get("chat_id")
    if chat_id in (None, ""):
        # Fall back to first allowed_chat_id / allowed_user_id
        if cfg.telegram.allowed_chat_ids:
            chat_id = cfg.telegram.allowed_chat_ids[0]
        elif cfg.telegram.allowed_user_ids:
            chat_id = cfg.telegram.allowed_user_ids[0]
        else:
            _audit("mcp_gateway", "harvey_telegram_send", "error", error="no chat_id")
            return json.dumps({"ok": False, "error": "No chat_id and no configured fallback"})

    text = args["text"]
    parse_mode = args.get("parse_mode", "Markdown")
    url = f"https://api.telegram.org/bot{cfg.telegram.bot_token}/sendMessage"
    try:
        r = _req.post(url, json={"chat_id": chat_id, "text": text, "parse_mode": parse_mode}, timeout=10)
        ok = r.status_code == 200
        _audit(
            "mcp_gateway", "harvey_telegram_send",
            "ok" if ok else "error",
            detail=f"chat_id={chat_id}",
            error="" if ok else f"status={r.status_code}",
        )
        return json.dumps({"ok": ok, "chat_id": chat_id, "status": r.status_code})
    except Exception as e:
        _audit("mcp_gateway", "harvey_telegram_send", "error", error=f"{type(e).__name__}: {e}")
        return json.dumps({"ok": False, "error": f"{type(e).__name__}: {e}"})


# ═══════════════════════════════════════════════════════════════
#  MCP Protocol (stdio JSON-RPC 2.0)
# ═══════════════════════════════════════════════════════════════

def send_response(id, result):
    """Send a JSON-RPC response over stdio MCP transport.

    MCP stdio framing: one JSON object per line, terminated with '\\n'.
    NOT LSP-style Content-Length headers — that's a different protocol.
    Real MCP clients (Claude Code, Codex, Gemini CLI, OpenCode) all
    expect newline-delimited JSON.
    """
    msg = json.dumps({"jsonrpc": "2.0", "id": id, "result": result})
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()

def send_error(id, code, message):
    """Send a JSON-RPC error over stdio MCP transport (newline-framed)."""
    msg = json.dumps({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": code, "message": message},
    })
    sys.stdout.write(msg + "\n")
    sys.stdout.flush()

def read_message():
    """Read one newline-delimited JSON-RPC message from stdin.

    Returns None on EOF. Skips blank lines (some clients pad).
    """
    while True:
        line = sys.stdin.readline()
        if not line:
            return None  # EOF
        line = line.strip()
        if not line:
            continue  # tolerate blank padding
        try:
            return json.loads(line)
        except json.JSONDecodeError as e:
            # Send a parse error if we have any way to identify the request
            log.warning(f"parse error on line: {line[:120]!r} ({e})")
            # Return None to keep the loop alive — caller will read next line
            return None


# ═══════════════════════════════════════════════════════════════
#  Tool Definitions
# ═══════════════════════════════════════════════════════════════

TOOLS = [
    # ── Phase-1 swarm-wired tools (harvey_* namespace) ────
    # These route through AgentAccessControl + AuditLog + structured
    # logging via the mcp_gateway Subagent. Prefer these over the
    # legacy flat-name aliases below.
    {
        "name": "harvey_brain_search",
        "description": "Search Harvey's Brain (Brain journals + pages) with access control and audit logging. Returns hits with snippets. Prefer this over legacy brain_search.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "top_k": {"type": "integer", "description": "Max results", "default": 10},
            },
            "required": ["query"],
        },
    },
    {
        "name": "harvey_brain_write",
        "description": "Append a line to today's Brain journal with access control and audit logging. Input is auto-prefixed with '- ' for outliner format. Use [[wikilinks]] for entities.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "entry": {"type": "string", "description": "Journal line (leading '- ' added if missing)"},
                "tags": {"type": "array", "items": {"type": "string"}, "description": "Optional entity tags to append as wikilinks"},
            },
            "required": ["entry"],
        },
    },
    {
        "name": "harvey_olibia_speak",
        "description": "Invoke Harvey's guardian owl mascot Olibia 🦉 for commentary. Tones: greet, announce, celebrate, warn. Routes through the Olibia subagent so commentary is published to the event bus.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "What Olibia should comment on"},
                "tone": {
                    "type": "string",
                    "enum": ["greet", "announce", "celebrate", "warn"],
                    "description": "Owl tone",
                    "default": "announce",
                },
            },
            "required": ["message"],
        },
    },

    # ── Phase-2 swarm-wired tools (harvey_* namespace) ────
    {
        "name": "harvey_superbrain_query",
        "description": "Ask Harvey's Superbrain a natural-language question. Searches all knowledge (FTS5 + vector + entity graph) and synthesizes an answer with sources via LLM.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "Natural-language question"},
                "synthesize": {"type": "boolean", "description": "Run LLM synthesis (default true)", "default": True},
            },
            "required": ["question"],
        },
    },
    {
        "name": "harvey_superbrain_vector_search",
        "description": "Semantic vector search across Harvey's knowledge stores. Returns top-k hits ranked by embedding similarity. Use when FTS keyword search is too narrow.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "k": {"type": "integer", "description": "Number of hits", "default": 5},
            },
            "required": ["query"],
        },
    },
    {
        "name": "harvey_generate_image",
        "description": "Generate an image from a text prompt via MiniMax image-01. Returns the saved file path. Routes through mcp_gateway so the call is audited.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Image description"},
                "aspect_ratio": {"type": "string", "description": "e.g. 1:1, 16:9, 9:16", "default": "1:1"},
                "save_path": {"type": "string", "description": "Optional target path; default is data/chat/generated.png"},
            },
            "required": ["prompt"],
        },
    },
    {
        "name": "harvey_swarm_run",
        "description": "Run Harvey's multi-agent swarm on a request. Classifies intent (research/creative/archive/minimal) via IntelligentRouter, composes a team, and executes the DAG synchronously. Returns the workflow id, state, and per-step artifact summaries. Default timeout 120s.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "request": {"type": "string", "description": "Natural-language user request"},
                "parallelism": {"type": "integer", "description": "Override default team parallelism"},
                "timeout_s": {"type": "number", "description": "Max wall time before cancelling", "default": 120},
                "plan_only": {"type": "boolean", "description": "If true, classify + build workflow but don't execute", "default": False},
            },
            "required": ["request"],
        },
    },
    {
        "name": "harvey_swarm_status",
        "description": "Return the current swarm runtime status: registered agents, recent workflows, circuit breaker state per agent, and coordinator health.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string", "description": "Optional — return only this workflow's state"},
            },
        },
    },
    {
        "name": "harvey_telegram_send",
        "description": "Send a Telegram message to a configured chat id via Harvey's bot. Respects TelegramConfig allowlist and records an audit entry. Returns {ok, chat_id}.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "chat_id": {"type": ["integer", "string"], "description": "Telegram chat id (group or user)"},
                "text": {"type": "string", "description": "Message body (Markdown supported)"},
                "parse_mode": {"type": "string", "description": "Markdown or HTML", "default": "Markdown"},
            },
            "required": ["text"],
        },
    },
    {
        "name": "harvey_journal_entry",
        "description": "Append a summary to today's Brain journal with explicit entity tags. Wrapper over harvey_brain_write with wikilink formatting. Use this for 'remember that…' style logging.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "What happened (1-2 sentences)"},
                "tags": {"type": "array", "items": {"type": "string"}, "description": "Entity tags → [[wikilinks]]"},
            },
            "required": ["summary"],
        },
    },

    # ── Omni multimodal understanding (Xiaomi MiMo via switchAILocal) ────
    # Any agent that needs to look at an image, listen to audio, or
    # watch a video clip can call these three tools. `source` accepts a
    # URL, a data: URI, or a local file path — local paths are
    # base64-encoded automatically. Routes to xiaomi-tp:mimo-v2-omni.
    {
        "name": "harvey_describe_image",
        "description": "Look at an image and return a text description or answer. Source accepts a URL, a data: URI, or a local file path (auto base64-encoded). Routes through switchAILocal to xiaomi-tp:mimo-v2-omni. Use this when a user message contains a photo, screenshot, chart, or diagram.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "source": {"type": "string", "description": "Image URL, data: URI, or local file path"},
                "prompt": {"type": "string", "description": "What to ask about the image", "default": "Describe the content in detail."},
                "max_completion_tokens": {"type": "integer", "description": "Max tokens in the reply", "default": 1024},
            },
            "required": ["source"],
        },
    },
    {
        "name": "harvey_describe_audio",
        "description": "Listen to an audio clip and return a text answer — transcribe speech, analyze tone, or answer questions about what was said. Source accepts a URL, a data: URI, or a local file path (auto base64-encoded). Routes through switchAILocal to xiaomi-tp:mimo-v2-omni. Use this for voice messages, meeting recordings, or any audio a user drops on Harvey.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "source": {"type": "string", "description": "Audio URL, data: URI, or local file path (wav/mp3/flac/ogg/m4a)"},
                "prompt": {"type": "string", "description": "What to ask about the audio", "default": "Transcribe this audio and summarize its content."},
                "max_completion_tokens": {"type": "integer", "description": "Max tokens in the reply", "default": 1024},
            },
            "required": ["source"],
        },
    },
    {
        "name": "harvey_describe_video",
        "description": "Watch a video clip and return a text description or answer. Source accepts a URL, a data: URI, or a local file path (auto base64-encoded). Supports optional fps (default Xiaomi 2, max 10) and media_resolution (default|max). Routes through switchAILocal to xiaomi-tp:mimo-v2-omni. Use this for short clips, screen recordings, or any video a user wants Harvey to understand.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "source": {"type": "string", "description": "Video URL, data: URI, or local file path (mp4/mov/webm/mkv)"},
                "prompt": {"type": "string", "description": "What to ask about the video", "default": "Describe the main action and any text visible in this video."},
                "fps": {"type": "integer", "description": "Frames per second to sample (default 2, max 10)"},
                "media_resolution": {"type": "string", "enum": ["default", "max"], "description": "Spatial detail level"},
                "max_completion_tokens": {"type": "integer", "description": "Max tokens in the reply", "default": 1024},
            },
            "required": ["source"],
        },
    },

    # ── Brain ─────────────────────────────────────────────
    {
        "name": "brain_search",
        "description": "Search Harvey's Brain (Brain knowledge graph) using full-text search. Returns relevant pages and journal entries. Use this to find what Harvey knows about any topic.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query"},
                "top_k": {"type": "integer", "description": "Max results (default 10)", "default": 10},
            },
            "required": ["query"],
        },
    },
    {
        "name": "brain_query",
        "description": "Ask Harvey's Brain a question. Searches all knowledge systems (FTS5 + entity graph) and synthesizes an answer using LLM. Returns a comprehensive answer with sources.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "Natural language question"},
            },
            "required": ["question"],
        },
    },
    {
        "name": "brain_recent",
        "description": "Get Harvey's recent journal entries. Shows what Harvey has been working on.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "days": {"type": "integer", "description": "How many days back (default 3)", "default": 3},
            },
        },
    },
    {
        "name": "brain_write_journal",
        "description": "Write an entry to today's Brain journal. Use this to log significant work, decisions, or discoveries. Format: outliner (lines start with '- '). Use [[entity]] for wikilinks.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "entry": {"type": "string", "description": "Journal entry text (outliner format)"},
            },
            "required": ["entry"],
        },
    },
    {
        "name": "brain_context",
        "description": "Get Harvey's compact memory context for a task. Returns the most relevant knowledge (~800 tokens) to inject into a prompt. Includes identity, today's work, and topic-relevant pages.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Task description for context relevance"},
            },
        },
    },
    {
        "name": "brain_entities",
        "description": "Get top entities (people, projects, companies) from Harvey's knowledge graph, ranked by mention count.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "top_n": {"type": "integer", "description": "How many (default 10)", "default": 10},
            },
        },
    },

    # ── Wiki ──────────────────────────────────────────────
    {
        "name": "wiki_lint",
        "description": "Run a health check on Harvey's Brain wiki. Returns orphan pages, missing pages, dead links, and high-value gaps.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "wiki_compile",
        "description": "Compile today's journal entries into wiki pages. Extracts [[entities]] and updates or creates their pages. Detects contradictions.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "date": {"type": "string", "description": "Date to compile (YYYY_MM_DD). Default: today."},
            },
        },
    },
    {
        "name": "wiki_save",
        "description": "Save a synthesized answer or analysis as a permanent wiki page in the Brain. Use this when you produce valuable analysis that should persist.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Page title"},
                "content": {"type": "string", "description": "Page content"},
                "query": {"type": "string", "description": "Original question that produced this"},
            },
            "required": ["title", "content"],
        },
    },

    # ── SANCHO ────────────────────────────────────────────
    {
        "name": "sancho_tick",
        "description": "Run one SANCHO proactive tick. Evaluates all gated tasks (dream, wiki lint, index rebuild, daily briefing) and runs eligible ones.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "sancho_status",
        "description": "Get SANCHO proactive engine status. Shows all tasks with gate states (time/session/lock).",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "dream",
        "description": "Run Harvey's memory consolidation dream cycle. Four phases: Orient → Gather → Consolidate → Prune. Updates wiki pages, resolves contradictions, prunes stale content.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "force": {"type": "boolean", "description": "Force dream even if gates don't pass", "default": False},
            },
        },
    },

    # ── Coordinator ───────────────────────────────────────
    {
        "name": "swarm",
        "description": "Launch a multi-agent swarm to tackle a complex objective. Four phases: Research (parallel workers) → Synthesis (coordinator reads findings) → Implementation → Verification. Anti-lazy-delegation enforced.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "objective": {"type": "string", "description": "What the swarm should accomplish"},
            },
            "required": ["objective"],
        },
    },

    # ── Companion ─────────────────────────────────────────
    {
        "name": "buddy_status",
        "description": "Get Harvey's companion status. Shows the buddy's mood, stats, and greeting.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "nursery_status",
        "description": "Get Harvey's mascot nursery status. Shows all mascots with moods and the family psych level (0-100). Harvey's psychological health mirror.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "nursery_hatch",
        "description": "Hatch a new mascot! Born from real events. Each one is unique with procedural species, personality, and stats.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "trigger": {"type": "string", "description": "What caused this hatch (shipped_code, fixed_bug, milestone, etc.)", "default": "random"},
                "context": {"type": "string", "description": "Description of what happened"},
            },
        },
    },

    # ── Outbound Chokepoint ───────────────────────────────
    {
        "name": "outbound_draft",
        "description": "Draft an outbound message (email, LinkedIn, Slack). NEVER sends automatically. Creates a draft that Sebastian must approve out-of-band. This is the ONLY path for outbound from any Harvey body.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "channel": {"type": "string", "enum": ["email", "linkedin", "slack", "other"], "description": "Communication channel"},
                "recipient": {"type": "string", "description": "Who to send to"},
                "subject": {"type": "string", "description": "Subject/topic"},
                "body": {"type": "string", "description": "Message body (draft)"},
            },
            "required": ["channel", "recipient", "body"],
        },
    },

    # ── Skills ────────────────────────────────────────────
    {
        "name": "skill_discover",
        "description": "Discover Harvey skills by semantic search. 170+ skills across 19 categories. Returns matching skills with their descriptions and paths.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What capability are you looking for?"},
            },
            "required": ["query"],
        },
    },

    # ── Agent Management ───────────────────────────────────
    {
        "name": "agent_install",
        "description": "Install a Harvey agent from a GitHub URL. Clones the repo, installs dependencies, creates state directory. Say 'Harvey install this agent' with a GitHub URL.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "source": {"type": "string", "description": "GitHub URL (e.g. https://github.com/user/my-agent) or local path"},
                "name": {"type": "string", "description": "Custom name for the agent (optional, defaults to repo name)"},
            },
            "required": ["source"],
        },
    },
    {
        "name": "agent_uninstall",
        "description": "Uninstall a Harvey agent. Removes code and optionally keeps state data.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Agent name to uninstall"},
                "keep_data": {"type": "boolean", "description": "Keep state data in data/ directory", "default": False},
            },
            "required": ["name"],
        },
    },
    {
        "name": "agent_list",
        "description": "List all installed Harvey agents with their status, pattern, and description.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "agent_create",
        "description": "Scaffold a new Harvey agent from scratch. Generates agent.py, AGENT.md, and state directory with proper boilerplate for the chosen pattern (daemon, cron, or cli).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Agent name (lowercase, alphanumeric + dashes)"},
                "description": {"type": "string", "description": "What the agent does"},
                "pattern": {"type": "string", "enum": ["daemon", "cron", "cli"], "description": "Execution pattern: daemon (continuous), cron (scheduled), cli (manual)", "default": "cli"},
                "interval": {"type": "string", "description": "For cron pattern: schedule interval (e.g. 5m, 1h, daily)"},
            },
            "required": ["name"],
        },
    },
    {
        "name": "agent_info",
        "description": "Show detailed information about an installed Harvey agent. Reads AGENT.md or README.md.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Agent name"},
            },
            "required": ["name"],
        },
    },

    # ── HarveyChat ────────────────────────────────────────
    {
        "name": "chat_status",
        "description": "Get HarveyChat gateway status. Shows if running, channels active, message counts, and switchAILocal connectivity.",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "chat_history",
        "description": "Get recent chat conversation history from external channels (Telegram, etc.).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "channel": {"type": "string", "description": "Channel name (telegram, etc.)", "default": "telegram"},
                "limit": {"type": "integer", "description": "Max messages to return", "default": 20},
            },
        },
    },
    {
        "name": "chat_send",
        "description": "Send a proactive message to Sebastian via an external channel (Telegram). Use for alerts, reminders, or proactive updates.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "Message text to send"},
                "channel": {"type": "string", "description": "Channel to send on", "default": "telegram"},
            },
            "required": ["message"],
        },
    },
    {
        "name": "chat_stats",
        "description": "Get HarveyChat conversation statistics — total messages, sessions, active channels.",
        "inputSchema": {"type": "object", "properties": {}},
    },

    # ── Costs ─────────────────────────────────────────────
    {
        "name": "costs_summary",
        "description": "Get token usage and API cost summary for the current session or historical analysis.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "history_days": {"type": "integer", "description": "If set, show historical costs for N days"},
            },
        },
    },
]


# ═══════════════════════════════════════════════════════════════
#  Tool Handlers
# ═══════════════════════════════════════════════════════════════

def _maybe_gimmick_text(context: str, force: bool = False) -> str:
    """Roll the gimmick dice and return art string (empty if suppressed)."""
    try:
        from core.terminal.gimmicks import render_gimmick
        return render_gimmick(context, force=force) or ""
    except Exception:
        return ""


def handle_tool(name: str, args: dict) -> str:
    """Execute a tool and return the result as text."""

    # ── Phase-1 swarm-wired tools (harvey_* namespace) ────
    if name == "harvey_brain_search":
        gw = _gateway()
        results = gw.tool("brain_search", {
            "query": args["query"],
            "top_k": args.get("top_k", 10),
        })
        # brain_search from TOOL_DISPATCH returns a list of dicts
        if not results:
            return "No results found."
        if isinstance(results, str):
            return results
        lines = []
        for r in results if isinstance(results, list) else [results]:
            if isinstance(r, dict):
                title = r.get("name") or r.get("title") or r.get("path") or "?"
                snippet = (r.get("content") or r.get("snippet") or "")[:280]
                score = r.get("score")
                head = f"[{r.get('doc_type', '?')}] {title}"
                if score is not None:
                    head += f" (score: {score:.2f})" if isinstance(score, (int, float)) else f" (score: {score})"
                lines.append(f"{head}\n{snippet}")
            else:
                lines.append(str(r))
        result_text = "\n\n".join(lines)
        gimmick = _maybe_gimmick_text("search")
        return result_text + "\n" + gimmick if gimmick else result_text

    elif name == "harvey_brain_write":
        gw = _gateway()
        entry = args["entry"]
        if not entry.startswith("- "):
            entry = f"- {entry}"
        tags = args.get("tags") or []
        if tags:
            links = " ".join(f"[[{t}]]" for t in tags)
            entry = f"{entry} {links}"
        # TOOL_DISPATCH['brain_write'] expects args["content"]
        result = gw.tool("brain_write", {"content": entry})
        if isinstance(result, dict):
            return json.dumps(result, indent=2, default=str)
        return str(result or "Written to today's journal.")

    elif name == "harvey_olibia_speak":
        # Olibia is a Subagent, not a TOOL_DISPATCH tool, so we invoke
        # her directly via a synthetic step. Audit log entry recorded
        # manually so every invocation is traceable.
        message = args["message"]
        tone = args.get("tone", "announce")

        # Record audit entry for the MCP-facing call
        try:
            from core.security.audit_log import get_default_audit_log
            audit = get_default_audit_log()
            if audit is not None:
                audit.record_tool_call(
                    agent="mcp_gateway",
                    tool="harvey_olibia_speak",
                    outcome="ok",
                    detail=f"tone={tone}",
                )
        except Exception as e:
            log.debug(f"audit record failed: {e}")

        olibia = _olibia()

        class _Step:
            action = tone
            id = f"mcp_olibia_{int(time.time())}"

        out = olibia.execute(_Step(), {"message": message})
        return out.get("message", "")

    # ── Phase-2 swarm-wired tools (harvey_* namespace) ────
    elif name == "harvey_superbrain_query":
        _audit("mcp_gateway", "harvey_superbrain_query", "ok",
               detail=args.get("question", "")[:60])
        from core.superbrain.superbrain import Superbrain
        sb = Superbrain()
        result = sb.query(
            args["question"],
            synthesize=args.get("synthesize", True),
        )
        sources = "\n".join(
            f"  - [{s.source}] {s.title}"
            for s in (result.sources or [])[:5]
        )
        answer = getattr(result, "answer", "") or ""
        result_text = f"{answer}\n\nSources:\n{sources}".strip()
        gimmick = _maybe_gimmick_text("search")
        return result_text + "\n" + gimmick if gimmick else result_text

    elif name == "harvey_superbrain_vector_search":
        gw = _gateway()
        out = gw.tool("superbrain_vector_search", {
            "query": args["query"],
            "k": args.get("k", 5),
        })
        if isinstance(out, (dict, list)):
            return json.dumps(out, indent=2, default=str)
        return str(out)

    elif name == "harvey_generate_image":
        gw = _gateway()
        out = gw.tool("generate_image", {
            "prompt": args["prompt"],
            "aspect_ratio": args.get("aspect_ratio", "1:1"),
            "save_path": args.get("save_path", ""),
        })
        return str(out)

    elif name == "harvey_swarm_run":
        return _run_swarm(args)

    elif name == "harvey_swarm_status":
        return _swarm_status(args.get("workflow_id"))

    elif name == "harvey_telegram_send":
        return _telegram_send(args)

    elif name == "harvey_journal_entry":
        # Thin wrapper: delegate to harvey_brain_write with tags
        return handle_tool("harvey_brain_write", {
            "entry": args["summary"],
            "tags": args.get("tags", []),
        })

    # ── Omni multimodal understanding ─────────────────────
    elif name == "harvey_describe_image":
        return _omni_describe("image", args)

    elif name == "harvey_describe_audio":
        return _omni_describe("audio", args)

    elif name == "harvey_describe_video":
        return _omni_describe("video", args)

    # ── Brain tools ───────────────────────────────────────
    if name == "brain_search":
        from core.superbrain.store import SuperbrainStore
        store = SuperbrainStore()
        results = store.search(args["query"], top_k=args.get("top_k", 10))
        output = []
        for r in results:
            output.append(f"[{r['doc_type']}] {r['name']} (score: {r['score']:.2f})\n{r['content'][:300]}\n")
        result_text = "\n".join(output) if output else "No results found."
        gimmick = _maybe_gimmick_text("search")
        return result_text + "\n" + gimmick if gimmick else result_text

    elif name == "brain_query":
        from core.superbrain.superbrain import Superbrain
        sb = Superbrain()
        result = sb.query(args["question"], synthesize=True)
        sources = "\n".join(f"  - [{s.source}] {s.title}" for s in result.sources[:5])
        result_text = f"{result.answer}\n\nSources:\n{sources}"
        gimmick = _maybe_gimmick_text("search")
        return result_text + "\n" + gimmick if gimmick else result_text

    elif name == "brain_recent":
        days = args.get("days", 3)
        journals_dir = Path(HARVEY_HOME) / "data" / "Brain" / "journals"
        entries = []
        for i in range(days):
            d = date.today() - __import__("datetime").timedelta(days=i)
            path = journals_dir / f"{d.strftime('%Y_%m_%d')}.md"
            if path.exists():
                content = path.read_text(encoding="utf-8", errors="replace")
                entries.append(f"=== {d.isoformat()} ===\n{content[:2000]}")
        return "\n\n".join(entries) if entries else "No recent journal entries."

    elif name == "brain_write_journal":
        entry = args["entry"]
        if not entry.startswith("- "):
            entry = f"- {entry}"
        today = date.today().strftime("%Y_%m_%d")
        journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
        journal_path.parent.mkdir(parents=True, exist_ok=True)
        with open(journal_path, "a") as f:
            f.write(f"\n{entry}\n")
        return f"Written to {today} journal."

    elif name == "brain_context":
        from core.superbrain.memory_stack import MemoryStack
        ms = MemoryStack()
        task = args.get("task", "")
        if task:
            return ms.for_query(task)
        return ms.compact()

    elif name == "brain_entities":
        from core.superbrain.store import SuperbrainStore
        store = SuperbrainStore()
        gods = store.god_nodes(top_n=args.get("top_n", 10))
        return "\n".join(f"  {g['name']}: {g['mentions']} mentions" for g in gods)

    # ── Wiki tools ────────────────────────────────────────
    elif name == "wiki_lint":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        report = wiki.lint()
        return json.dumps(report, indent=2, default=str)

    elif name == "wiki_compile":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        d = args.get("date")
        result = wiki.compile_journal(d)
        return json.dumps(result, indent=2, default=str)

    elif name == "wiki_save":
        from core.superbrain.wiki import WikiOps
        wiki = WikiOps()
        path = wiki.save_answer(args["title"], args["content"], query=args.get("query", ""))
        return f"Saved to: {path}"

    # ── SANCHO tools ──────────────────────────────────────
    elif name == "sancho_tick":
        from core.sancho.engine import Sancho
        k = Sancho()
        results = k.tick()
        if not results:
            return "No eligible tasks this tick."
        result_text = json.dumps(results, indent=2, default=str)
        gimmick = _maybe_gimmick_text("sancho")
        return result_text + "\n" + gimmick if gimmick else result_text

    elif name == "sancho_status":
        from core.sancho.engine import Sancho
        k = Sancho()
        status = {}
        for name_t, task in k.registry.tasks.items():
            status[name_t] = {
                "enabled": task.enabled,
                "interval_min": task.interval_minutes,
                "gates": task.gates.status(),
            }
        return json.dumps(status, indent=2, default=str)

    elif name == "dream":
        from core.dreams.consolidator import DreamEngine
        engine = DreamEngine()
        report = engine.dream(force=args.get("force", False))
        return json.dumps(report.__dict__, indent=2, default=str)

    # ── Coordinator tools ─────────────────────────────────
    elif name == "swarm":
        from core.coordinator.coordinator import Coordinator
        coord = Coordinator()
        task = coord.execute(args["objective"])
        return json.dumps(task.__dict__, indent=2, default=str)

    # ── Companion tools ───────────────────────────────────
    elif name == "buddy_status":
        from core.buddy.buddy import Buddy
        buddy = Buddy()
        buddy.interact()
        return f"{buddy.greet()}\n\n{buddy.stat_card()}"

    elif name == "nursery_status":
        from core.buddy.nursery import Nursery
        nursery = Nursery()
        return nursery.family_portrait()

    elif name == "nursery_hatch":
        from core.buddy.nursery import Nursery
        nursery = Nursery()
        baby = nursery.hatch(
            trigger=args.get("trigger", "random"),
            context=args.get("context", ""),
        )
        result_text = nursery.show_mascot(baby.mascot_id)
        gimmick = _maybe_gimmick_text("celebrate", force=True)
        return result_text + "\n" + gimmick if gimmick else result_text

    # ── Outbound chokepoint ───────────────────────────────
    elif name == "outbound_draft":
        draft_id = f"draft-{int(time.time())}"
        draft = {
            "id": draft_id,
            "channel": args["channel"],
            "recipient": args["recipient"],
            "subject": args.get("subject", ""),
            "body": args["body"],
            "status": "PENDING_APPROVAL",
            "created_at": datetime.now().isoformat(),
        }
        # Save to drafts file
        drafts_dir = Path(HARVEY_HOME) / "data" / "outbound"
        drafts_dir.mkdir(parents=True, exist_ok=True)
        draft_path = drafts_dir / f"{draft_id}.json"
        draft_path.write_text(json.dumps(draft, indent=2))

        # Log to journal
        today = date.today().strftime("%Y_%m_%d")
        journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
        with open(journal_path, "a") as f:
            f.write(f"\n- Outbound draft created: [{args['channel']}] to {args['recipient']} — {draft_id}\n")

        return f"Draft saved: {draft_id}\nChannel: {args['channel']}\nTo: {args['recipient']}\nStatus: PENDING_APPROVAL\n\nSebastian must approve this before it sends."

    # ── Agent Management ───────────────────────────────────
    elif name == "agent_install":
        from core.agents.scaffold import install_agent
        result = install_agent(args["source"], name=args.get("name"))
        if "error" in result:
            return f"Installation failed: {result['error']}"
        return (
            f"Agent '{result['name']}' installed!\n"
            f"Code:  {result['agent_dir']}\n"
            f"State: {result['state_dir']}\n"
            f"Deps:  {'installed' if result['deps_installed'] else 'none found'}\n"
            f"\nReady to use. Run: superbrain agent info {result['name']}"
        )

    elif name == "agent_uninstall":
        from core.agents.scaffold import uninstall_agent
        result = uninstall_agent(args["name"], keep_data=args.get("keep_data", False))
        if "error" in result:
            return f"Uninstall failed: {result['error']}"
        return f"Agent '{args['name']}' uninstalled. Data {'kept' if result.get('data_kept') else 'removed'}."

    elif name == "agent_list":
        from core.agents.scaffold import list_agents
        agents = list_agents()
        if not agents:
            return "No agents installed. Use agent_install with a GitHub URL to add one."
        lines = []
        for a in agents:
            lines.append(f"  {a['name']:<25} {a.get('pattern', '?'):<10} {a.get('status', '?'):<10} {a.get('description', '')[:50]}")
        return "Installed agents:\n" + "\n".join(lines)

    elif name == "agent_create":
        from core.agents.scaffold import scaffold_agent
        result = scaffold_agent(
            args["name"],
            description=args.get("description", ""),
            pattern=args.get("pattern", "cli"),
            interval=args.get("interval", ""),
        )
        if "error" in result:
            return f"Creation failed: {result['error']}"
        steps = "\n".join(f"  {s}" for s in result.get("next_steps", []))
        return (
            f"Agent '{result['name']}' created!\n"
            f"Code:  {result['agent_dir']}\n"
            f"State: {result['state_dir']}\n"
            f"Pattern: {result['pattern']}\n\n"
            f"Next steps:\n{steps}"
        )

    elif name == "agent_info":
        from core.agents.scaffold import agent_info
        return agent_info(args["name"])

    # ── Skills ────────────────────────────────────────────
    elif name == "skill_discover":
        try:
            from core.registry.skill_registry import SkillRegistry
            registry = SkillRegistry()
            matches = registry.search(args["query"], top_k=5)
            return "\n".join(
                f"  [{m['category']}] {m['name']}: {m['description'][:100]}"
                for m in matches
            )
        except Exception as e:
            return f"Skill discovery error: {e}"

    # ── Costs ─────────────────────────────────────────────
    elif name == "costs_summary":
        from core.telemetry.cost_tracker import CostTracker
        if args.get("history_days"):
            summary = CostTracker.historical_summary(args["history_days"])
        else:
            tracker = CostTracker()
            summary = tracker.session_summary()
        return json.dumps(summary, indent=2, default=str)

    # ── HarveyChat tools ────────────────────────────────
    elif name == "chat_status":
        import subprocess
        result = subprocess.run(
            [sys.executable, "-m", "core.chat", "status"],
            capture_output=True, text=True,
            cwd=os.path.join(HARVEY_HOME, "harvey-os"),
        )
        return result.stdout or result.stderr or "Could not get status"

    elif name == "chat_history":
        from core.chat.store import ChatStore
        from core.chat.config import load_config as load_chat_config
        cfg = load_chat_config()
        store = ChatStore(cfg.db_path)
        channel = args.get("channel", "telegram")
        limit = args.get("limit", 20)
        # Get all recent messages across users
        rows = store.db.execute(
            "SELECT channel, channel_user_id, role, content, created_at FROM messages "
            "WHERE channel = ? ORDER BY created_at DESC LIMIT ?",
            (channel, limit)
        ).fetchall()
        store.close()
        if not rows:
            return "No chat history yet."
        output = []
        for r in reversed(rows):
            ts = datetime.fromtimestamp(r["created_at"]).strftime("%H:%M")
            output.append(f"[{ts}] {r['role']}: {r['content'][:200]}")
        return "\n".join(output)

    elif name == "chat_send":
        # Proactive outbound via Telegram API (direct, no gateway needed)
        from core.chat.config import load_config as load_chat_config
        cfg = load_chat_config()
        if not cfg.telegram.bot_token:
            return "ERROR: Telegram not configured"
        if not cfg.telegram.allowed_user_ids:
            return "ERROR: No allowed_user_ids configured — can't determine recipient"
        import requests as _req
        user_id = cfg.telegram.allowed_user_ids[0]  # Send to primary user
        r = _req.post(
            f"https://api.telegram.org/bot{cfg.telegram.bot_token}/sendMessage",
            json={"chat_id": user_id, "text": args["message"], "parse_mode": "Markdown"},
            timeout=10,
        )
        if r.status_code == 200:
            return f"Message sent to Telegram user {user_id}"
        return f"Send failed: {r.text[:200]}"

    elif name == "chat_stats":
        from core.chat.store import ChatStore
        from core.chat.config import load_config as load_chat_config
        cfg = load_chat_config()
        store = ChatStore(cfg.db_path)
        stats = store.get_stats()
        store.close()
        return json.dumps(stats, indent=2)

    else:
        return f"Unknown tool: {name}"


# ═══════════════════════════════════════════════════════════════
#  MCP Server Main Loop
# ═══════════════════════════════════════════════════════════════

def main():
    """Run the Harvey MCP server (stdio transport)."""
    log.info("Harvey MCP server starting...")

    while True:
        try:
            msg = read_message()
            if msg is None:
                break

            method = msg.get("method", "")
            id = msg.get("id")
            params = msg.get("params", {})

            if method == "initialize":
                send_response(id, {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {"listChanged": False},
                    },
                    "serverInfo": {
                        "name": "harvey",
                        "version": "0.1.0",
                    },
                })

            elif method == "notifications/initialized":
                pass  # Acknowledgment, no response needed

            elif method == "tools/list":
                send_response(id, {"tools": TOOLS})

            elif method == "tools/call":
                tool_name = params.get("name", "")
                tool_args = params.get("arguments", {})
                try:
                    result = handle_tool(tool_name, tool_args)
                    send_response(id, {
                        "content": [{"type": "text", "text": result}],
                    })
                except Exception as e:
                    send_response(id, {
                        "content": [{"type": "text", "text": f"Error: {e}"}],
                        "isError": True,
                    })

            elif method == "ping":
                send_response(id, {})

            else:
                if id is not None:
                    send_error(id, -32601, f"Method not found: {method}")

        except json.JSONDecodeError:
            continue
        except EOFError:
            break
        except KeyboardInterrupt:
            break

    log.info("Harvey MCP server stopped.")


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(name)s | %(message)s", stream=sys.stderr)
    main()
