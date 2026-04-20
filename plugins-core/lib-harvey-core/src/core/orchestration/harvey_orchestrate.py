#!/usr/bin/env python3
"""
Harvey Orchestration Core — Task Graph + Artifact Registry + Agent Registry
v0.1.0

Usage:
    harvey orchestrate init              # Initialize workspace
    harvey orchestrate task create ...   # Create task
    harvey orchestrate task list        # List tasks
    harvey orchestrate task status <id>  # Task status
    harvey orchestrate task assign <id> <agent>  # Assign to agent
    harvey orchestrate task complete <id> --artifact ...  # Complete + produce artifact
    harvey orchestrate artifact list     # List artifacts
    harvey orchestrate artifact get <id> # Get artifact
    harvey orchestrate agents list       # List agents
    harvey orchestrate agents register <agent>  # Register agent
    harvey orchestrate status            # Full dashboard
    harvey orchestrate tree [root_id]    # Visualize DAG
"""

import argparse
import json
import os
import sys
import uuid
import hashlib
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

# Auto-improve: background review
sys.path.insert(0, str(Path(__file__).parent.parent.parent))
from core.improve.background_review import spawn_background_review
from core.improve.review_triggers import get_default_trigger

HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", Path.home() / "harvey"))
WORKSPACE = HARVEY_HOME / "workspace"
TASKS_DIR = WORKSPACE / "tasks"
ARTIFACTS_DIR = WORKSPACE / "artifacts"
AGENTS_DIR = WORKSPACE / "agents"
MEMORY_DIR = WORKSPACE / "memory"
INBOX_DIR = WORKSPACE / "inbox"


def ensure_dirs():
    for d in [TASKS_DIR, ARTIFACTS_DIR, AGENTS_DIR, MEMORY_DIR, INBOX_DIR]:
        d.mkdir(parents=True, exist_ok=True)


def load_json(path, default=None):
    if path.exists():
        return json.loads(path.read_text())
    return default if default is not None else {}


def save_json(path, data):
    path.write_text(json.dumps(data, indent=2))


# ───────────────────────────────────────────────────────────────
# TASKS
# ───────────────────────────────────────────────────────────────


def task_status(status: str) -> str:
    colors = {
        "PENDING": "\033[33m",  # yellow
        "READY": "\033[32m",  # green
        "RUNNING": "\033[34m",  # blue
        "BLOCKED": "\033[31m",  # red
        "COMPLETE": "\033[32m",  # green
        "FAILED": "\033[31m",  # red
    }
    reset = "\033[0m"
    return f"{colors.get(status, '')}{status}{reset}"


def cmd_task_create(args):
    task_id = f"task_{uuid.uuid4().hex[:8]}"
    now = datetime.utcnow().isoformat() + "Z"

    task = {
        "id": task_id,
        "name": args.name,
        "description": getattr(args, "description", ""),
        "status": "PENDING",
        "assigned_to": None,
        "session_id": None,
        "created_at": now,
        "started_at": None,
        "completed_at": None,
        "blocked_by": args.depends_on or [],
        "produces_artifacts": args.produces or [],
        "consumes_artifacts": [],
        "parent_task": getattr(args, "parent", None),
        "child_tasks": [],
        "progress": 0.0,
        "last_heartbeat": None,
        "error": None,
    }

    # Inherit consumes from blocking tasks
    for dep_id in args.depends_on or []:
        dep_task = load_task(dep_id)
        if dep_task:
            for art in dep_task.get("produces_artifacts", []):
                if art not in task["consumes_artifacts"]:
                    task["consumes_artifacts"].append(art)

    # Update parent-child relationships
    for dep_id in args.depends_on or []:
        dep_task = load_task(dep_id)
        if dep_task:
            if task_id not in dep_task.get("child_tasks", []):
                dep_task.setdefault("child_tasks", []).append(task_id)
            save_task(dep_id, dep_task)

    save_task(task_id, task)
    _update_task_index_add(task_id)

    # Check if task is actually READY (no blocking tasks)
    if not task["blocked_by"]:
        task["status"] = "READY"
        save_task(task_id, task)

    print(f"Created {task_id}: {args.name}")
    print(f"  Status: {task['status']}")
    if task["blocked_by"]:
        print(f"  Blocked by: {', '.join(task['blocked_by'])}")
    if task["consumes_artifacts"]:
        print(f"  Consumes: {', '.join(task['consumes_artifacts'])}")
    if task["produces_artifacts"]:
        print(f"  Produces: {', '.join(task['produces_artifacts'])}")

    return task_id


def load_task(task_id: str) -> Optional[dict]:
    path = TASKS_DIR / f"{task_id}.json"
    return load_json(path) if path.exists() else None


def save_task(task_id: str, task: dict):
    (TASKS_DIR / f"{task_id}.json").write_text(json.dumps(task, indent=2))


def cmd_task_list(args):
    index = load_json(TASKS_DIR / "task_index.json", {})
    if not index:
        print("No tasks found. Run `harvey orchestrate task create` first.")
        return

    for task_id in index.get("tasks", []):
        t = load_task(task_id)
        if not t:
            continue
        status = task_status(t["status"])
        assigned = t.get("assigned_to") or "—"
        print(f"{task_id}  {status}  {assigned:15s}  {t['name']}")


def cmd_task_status(args):
    t = load_task(args.task_id)
    if not t:
        print(f"Task {args.task_id} not found.")
        return

    print(f"Task: {t['name']}")
    print(f"ID: {t['id']}")
    print(f"Status: {task_status(t['status'])}")
    print(f"Assigned to: {t.get('assigned_to') or '—'}")
    print(f"Created: {t['created_at']}")
    if t.get("started_at"):
        print(f"Started: {t['started_at']}")
    if t.get("completed_at"):
        print(f"Completed: {t['completed_at']}")
    if t.get("blocked_by"):
        print(f"Blocked by: {', '.join(t['blocked_by'])}")
    if t.get("consumes_artifacts"):
        print(f"Consumes: {', '.join(t['consumes_artifacts'])}")
    if t.get("produces_artifacts"):
        print(f"Produces: {', '.join(t['produces_artifacts'])}")
    if t.get("child_tasks"):
        print(f"Children: {', '.join(t['child_tasks'])}")
    if t.get("error"):
        print(f"Error: {t['error']}")
    print(f"Progress: {t.get('progress', 0) * 100:.0f}%")


def cmd_task_assign(args):
    t = load_task(args.task_id)
    if not t:
        print(f"Task {args.task_id} not found.")
        return

    if t["status"] != "READY":
        print(f"Task is {t['status']}, not READY. Cannot assign.")
        return

    now = datetime.utcnow().isoformat() + "Z"
    t["assigned_to"] = args.agent
    t["status"] = "RUNNING"
    t["started_at"] = now
    save_task(args.task_id, t)

    # Update agent registry
    _update_agent_status(args.agent, "BUSY", args.task_id)

    print(f"Assigned {args.task_id} to {args.agent}")
    print(f"Status: RUNNING")


def cmd_task_complete(args):
    t = load_task(args.task_id)
    if not t:
        print(f"Task {args.task_id} not found.")
        return

    now = datetime.utcnow().isoformat() + "Z"

    # If producing an artifact, store it
    if args.artifact:
        artifact_id = args.artifact
        content = args.content or ""
        artifact_type = args.type or "text/plain"

        _save_artifact(
            artifact_id, content, artifact_type, args.task_id, t.get("assigned_to")
        )

        if artifact_id not in t["produces_artifacts"]:
            t["produces_artifacts"].append(artifact_id)

    t["status"] = "COMPLETE"
    t["completed_at"] = now
    t["progress"] = 1.0
    save_task(args.task_id, t)

    # Update agent
    if t.get("assigned_to"):
        _update_agent_status(t["assigned_to"], "IDLE", None)

    # Check if any child tasks should become READY
    _unblock_children(args.task_id)

    print(f"Task {args.task_id} marked COMPLETE")
    if args.artifact:
        print(f"Artifact stored: {args.artifact}")

    # Check newly unblocked tasks
    _print_ready_tasks()


def cmd_task_fail(args):
    t = load_task(args.task_id)
    if not t:
        print(f"Task {args.task_id} not found.")
        return

    now = datetime.utcnow().isoformat() + "Z"
    t["status"] = "FAILED"
    t["completed_at"] = now
    t["error"] = args.reason
    save_task(args.task_id, t)

    if t.get("assigned_to"):
        _update_agent_status(t["assigned_to"], "IDLE", None)

    # Fail children too
    for child_id in t.get("child_tasks", []):
        cmd_task_fail(
            argparse.Namespace(
                task_id=child_id, reason=f"Parent task failed: {args.reason}"
            )
        )

    print(f"Task {args.task_id} marked FAILED: {args.reason}")


def cmd_task_tree(args):
    index = load_json(TASKS_DIR / "task_index.json", {})
    root_id = args.root_id

    if not root_id:
        # Find root tasks (no parent)
        roots = []
        for tid in index.get("tasks", []):
            t = load_task(tid)
            if t and not t.get("parent_task"):
                roots.append(tid)
        if not roots:
            print("No root tasks found.")
            return
        root_id = roots[0]

    def print_tree(task_id, indent=0):
        t = load_task(task_id)
        if not t:
            return
        prefix = "  " * indent
        status = task_status(t["status"])
        assigned = t.get("assigned_to") or "—"
        print(f"{prefix}└─ {t['name']} [{task_id}] {status} ({assigned})")
        for child in t.get("child_tasks", []):
            print_tree(child, indent + 1)

    print_tree(root_id)


# ───────────────────────────────────────────────────────────────
# ARTIFACTS
# ───────────────────────────────────────────────────────────────


def _save_artifact(
    artifact_id: str,
    content: str,
    content_type: str,
    task_id: str,
    agent_id: Optional[str],
):
    art_dir = ARTIFACTS_DIR / artifact_id.replace("artifact://harvey/", "").replace(
        "/", "_"
    )
    art_dir.mkdir(parents=True, exist_ok=True)

    now = datetime.utcnow()
    ttl = int(
        os.environ.get("HARVEY_ARTIFACT_TTL", str(7 * 24 * 3600))
    )  # 7 days default

    # Determine extension
    ext_map = {"text/markdown": "md", "text/plain": "txt", "application/json": "json"}
    ext = ext_map.get(content_type, "txt")
    content_path = art_dir / f"content.{ext}"

    content_path.write_text(content)

    meta = {
        "id": artifact_id,
        "type": content_type,
        "producer": {
            "task_id": task_id,
            "agent_id": agent_id,
        },
        "content_path": str(content_path),
        "content_size_bytes": len(content.encode()),
        "content_hash": hashlib.sha256(content.encode()).hexdigest(),
        "depends_on": [],
        "consumed_by": [],
        "created_at": now.isoformat() + "Z",
        "expires_at": (now + timedelta(seconds=ttl)).isoformat() + "Z",
        "pinned": False,
    }

    (art_dir / "meta.json").write_text(json.dumps(meta, indent=2))
    _update_artifact_index(artifact_id, meta)

    return meta


def _update_artifact_index(artifact_id: str, meta: dict):
    idx_path = ARTIFACTS_DIR / "artifacts_index.json"
    idx = load_json(idx_path, {"artifacts": {}})
    idx["artifacts"][artifact_id] = {
        "type": meta["type"],
        "producer": meta["producer"],
        "created_at": meta["created_at"],
        "expires_at": meta["expires_at"],
        "pinned": meta["pinned"],
    }
    save_json(idx_path, idx)


def cmd_artifact_list(args):
    idx_path = ARTIFACTS_DIR / "artifacts_index.json"
    idx = load_json(idx_path, {"artifacts": {}})

    if not idx.get("artifacts"):
        print("No artifacts found.")
        return

    for art_id, info in idx["artifacts"].items():
        pinned = "📌" if info.get("pinned") else "  "
        created = info.get("created_at", "")[:10]
        producer = info.get("producer", {}).get("agent_id", "?")
        print(f"{pinned} {art_id}")
        print(f"     type={info['type']}  produced_by={producer}  created={created}")


def cmd_artifact_get(args):
    # Convert artifact://harvey/... to path
    art_name = args.artifact_id.replace("artifact://harvey/", "")
    art_dir = ARTIFACTS_DIR / art_name.replace("/", "_")

    if not art_dir.exists():
        print(f"Artifact {args.artifact_id} not found.")
        return

    meta = load_json(art_dir / "meta.json", {})

    # Find content file
    content_files = list(art_dir.glob("content.*"))
    if content_files:
        content = content_files[0].read_text()
        print(f"=== {args.artifact_id} ===")
        print(f"Type: {meta.get('type')}")
        print(f"Producer: {meta.get('producer', {}).get('agent_id', '?')}")
        print(f"Created: {meta.get('created_at', '?')}")
        print()
        print(content[: args.max_content or 4000])
        if len(content) > (args.max_content or 4000):
            print(f"\n... [truncated, full size: {len(content)} bytes]")


def cmd_artifact_pin(args):
    art_name = args.artifact_id.replace("artifact://harvey/", "")
    art_dir = ARTIFACTS_DIR / art_name.replace("/", "_")
    meta_path = art_dir / "meta.json"
    meta = load_json(meta_path, {})
    meta["pinned"] = True
    save_json(meta_path, meta)
    print(f"Pinned {args.artifact_id}")


# ───────────────────────────────────────────────────────────────
# AGENTS
# ───────────────────────────────────────────────────────────────

AGENT_REGISTRY_PATH = AGENTS_DIR / "agent_registry.json"


def load_agent_registry() -> dict:
    return load_json(AGENT_REGISTRY_PATH, {"agents": {}})


def save_agent_registry(reg: dict):
    save_json(AGENT_REGISTRY_PATH, reg)


def _update_agent_status(agent_id: str, status: str, current_task: Optional[str]):
    reg = load_agent_registry()
    if agent_id in reg["agents"]:
        reg["agents"][agent_id]["status"] = status
        reg["agents"][agent_id]["last_seen"] = datetime.utcnow().isoformat() + "Z"
        if current_task is not None:
            reg["agents"][agent_id]["current_task_id"] = current_task
    save_agent_registry(reg)


def cmd_agents_list(args):
    reg = load_agent_registry()
    if not reg.get("agents"):
        print("No agents registered.")
        print(
            "Run `harvey orchestrate agents register <name> --type local --command claude`"
        )
        return

    for agent_id, info in reg["agents"].items():
        status_color = {
            "IDLE": "\033[32m",
            "BUSY": "\033[34m",
            "OFFLINE": "\033[31m",
            "ONLINE": "\033[32m",
        }
        reset = "\033[0m"
        status_str = f"{status_color.get(info['status'], '')}{info['status']}{reset}"
        current = info.get("current_task_id") or "—"
        last_seen = info.get("last_seen", "never")[:16]
        print(f"{agent_id}")
        print(f"  status: {status_str}  task: {current}  last_seen: {last_seen}")
        print(
            f"  type: {info.get('type')}  capabilities: {', '.join(info.get('capabilities', []))}"
        )


def cmd_agents_register(args):
    reg = load_agent_registry()
    if args.agent_id in reg["agents"]:
        print(f"Agent {args.agent_id} already registered. Updating.")
    else:
        reg["agents"][args.agent_id] = {
            "type": args.type,
            "command": getattr(args, "command", None),
            "endpoint": getattr(args, "endpoint", None),
            "status": "IDLE",
            "current_task_id": None,
            "last_seen": datetime.utcnow().isoformat() + "Z",
            "capabilities": args.capabilities or [],
        }
    save_agent_registry(reg)
    print(f"Registered agent: {args.agent_id}")


# ───────────────────────────────────────────────────────────────
# STATUS DASHBOARD
# ───────────────────────────────────────────────────────────────


def cmd_status(args):
    reg = load_agent_registry()
    idx = load_json(TASKS_DIR / "task_index.json", {})
    art_idx = load_json(ARTIFACTS_DIR / "artifacts_index.json", {"artifacts": {}})

    print()
    print("╔══════════════════════════════════════════════════════════════╗")
    print("║  HARVEY ORCHESTRATION STATUS                                 ║")
    print("╠══════════════════════════════════════════════════════════════╣")

    # Tasks
    tasks = idx.get("tasks", [])
    active = [
        t for t in tasks if load_task(t).get("status") not in ("COMPLETE", "FAILED")
    ]
    print(
        f"║  TASKS ({len(active)} active, {len(tasks)} total)                        ║"
    )
    for task_id in tasks:
        t = load_task(task_id)
        if not t:
            continue
        status = t["status"]
        if status in ("COMPLETE", "FAILED"):
            continue
        assigned = t.get("assigned_to") or "—"
        prog = t.get("progress", 0)
        bar = "▓" * int(prog * 10) + "░" * (10 - int(prog * 10))
        print(f"║  ├── {t['name'][:30]:30s} {status:8s} [{bar}]      ║")
    print("╠══════════════════════════════════════════════════════════════╣")

    # Artifacts
    arts = art_idx.get("artifacts", {})
    ready = [a for a, i in arts.items() if "READY" not in str(i)]
    print(f"║  ARTIFACTS ({len(arts)} total)                                        ║")
    for art_id, info in list(arts.items())[:5]:
        pinned = "📌" if info.get("pinned") else "  "
        producer = info.get("producer", {}).get("agent_id", "?")
        print(f"║  └── {pinned} {art_id[:50]}            ║")
        print(f"║       type={info['type']}  by={producer}                      ║")
    print("╠══════════════════════════════════════════════════════════════╣")

    # Agents
    agents = reg.get("agents", {})
    print(
        f"║  AGENTS ({len(agents)} registered)                                      ║"
    )
    for agent_id, info in agents.items():
        status = info["status"]
        current = info.get("current_task_id") or "—"
        print(f"║  ├── {agent_id:20s}  {status:8s}  task: {current[:20]}    ║")
    print("╚══════════════════════════════════════════════════════════════╝")
    print()


# ───────────────────────────────────────────────────────────────
# INTERNAL HELPERS
# ───────────────────────────────────────────────────────────────


def _update_task_index_add(task_id: str):
    idx_path = TASKS_DIR / "task_index.json"
    idx = load_json(idx_path, {"tasks": []})
    if task_id not in idx["tasks"]:
        idx["tasks"].append(task_id)
    save_json(idx_path, idx)


def _unblock_children(parent_task_id: str):
    """Check if any blocked children are now ready."""
    t = load_task(parent_task_id)
    if not t:
        return

    for child_id in t.get("child_tasks", []):
        child = load_task(child_id)
        if not child or child["status"] != "BLOCKED":
            continue

        # Check if all dependencies are complete
        all_deps_done = all(
            load_task(dep_id).get("status") == "COMPLETE"
            for dep_id in child.get("blocked_by", [])
        )

        if all_deps_done:
            child["status"] = "READY"
            save_task(child_id, child)
            print(f"\n[Harvey] Task {child_id} ({child['name']}) is now READY!")


def _print_ready_tasks():
    idx = load_json(TASKS_DIR / "task_index.json", {})
    for task_id in idx.get("tasks", []):
        t = load_task(task_id)
        if t and t.get("status") == "READY" and not t.get("assigned_to"):
            print(
                f"[Harvey] Task {task_id} ({t['name']}) is READY and awaiting assignment."
            )


# ───────────────────────────────────────────────────────────────
# WORKSPACE INIT
# ───────────────────────────────────────────────────────────────


def cmd_init(args):
    ensure_dirs()

    # Create default agent registry
    if not AGENT_REGISTRY_PATH.exists():
        save_agent_registry(
            {
                "agents": {
                    "claude-code": {
                        "type": "local",
                        "command": "claude",
                        "endpoint": "cli",
                        "status": "IDLE",
                        "current_task_id": None,
                        "last_seen": datetime.utcnow().isoformat() + "Z",
                        "capabilities": ["code", "research", "write", "analysis"],
                    },
                    "gemini-cli": {
                        "type": "local",
                        "command": "gemini",
                        "endpoint": "cli",
                        "status": "IDLE",
                        "current_task_id": None,
                        "last_seen": datetime.utcnow().isoformat() + "Z",
                        "capabilities": ["research", "general", "writing"],
                    },
                    "goose": {
                        "type": "local",
                        "command": "goose",
                        "endpoint": "cli",
                        "status": "IDLE",
                        "current_task_id": None,
                        "last_seen": datetime.utcnow().isoformat() + "Z",
                        "capabilities": ["code", "implementation", "automation"],
                    },
                }
            }
        )
        print("Registered default agents: claude-code, gemini-cli, goose")

    print(f"Workspace initialized at {WORKSPACE}")
    print(f"  Tasks: {TASKS_DIR}")
    print(f"  Artifacts: {ARTIFACTS_DIR}")
    print(f"  Agents: {AGENTS_DIR}")
    print(f"  Memory: {MEMORY_DIR}")
    print(f"  Inbox: {INBOX_DIR}")


# ───────────────────────────────────────────────────────────────
# BACKGROUND REVIEW
# ───────────────────────────────────────────────────────────────


def cmd_review(args):
    """
    Trigger background review of a conversation.

    Loads messages from session DB or parses --messages-json, then spawns
    a background review thread that saves learnings to memory/skills.
    """
    messages = None

    # Load messages
    if args.messages_json:
        try:
            import json as _json

            messages = _json.loads(args.messages_json)
            if not isinstance(messages, list):
                print("Error: --messages-json must be a JSON array of messages")
                return
        except Exception as e:
            print(f"Error parsing --messages-json: {e}")
            return
    elif args.session_id:
        # Load from session DB
        try:
            from core.session.session_db import SessionDB

            db = SessionDB()
            messages = db.get_messages_as_conversation(args.session_id)
            if not messages:
                print(f"No messages found for session {args.session_id}")
                return
        except Exception as e:
            print(f"Error loading session: {e}")
            return
    else:
        print("Error: must provide --session-id or --messages-json")
        return

    review_memory = not args.skills_only
    review_skills = not args.memory_only

    if args.force:
        trigger = get_default_trigger()
        should_fire = True
    else:
        trigger = get_default_trigger()
        should_fire, review_memory, review_skills = trigger.record_conversation(
            num_messages=len(messages)
        )

    if not should_fire:
        print("Review skipped (trigger policy did not fire). Use --force to override.")
        return

    # Initialize memory store
    memory_store = None
    if review_memory:
        try:
            from core.memory.frozen_memory import MemoryStore

            memory_store = MemoryStore()
            memory_store.load_from_disk()
        except Exception as e:
            print(f"Warning: could not initialize memory store: {e}")

    print(f"Starting background review of {len(messages)} messages...")
    print("  Memory review: " + ("enabled" if review_memory else "disabled"))
    print("  Skills review: " + ("enabled" if review_skills else "disabled"))
    print("(Background review is running. It will save learnings quietly.)")

    import time

    spawn_background_review(
        messages=messages,
        memory_store=memory_store,
        review_memory=review_memory,
        review_skills=review_skills,
    )
    time.sleep(15)  # Give background thread time to complete


# ───────────────────────────────────────────────────────────────
# SKILLS GUARD
# ───────────────────────────────────────────────────────────────


def cmd_skills(args):
    """
    Scan a skill for security threats using Skills Guard.

    Usage:
        harvey orchestrate skills scan /path/to/skill --source community
    """
    if args.skills_cmd == "scan":
        from pathlib import Path as _Path

        skill_path = _Path(args.skill_path).expanduser().resolve()
        if not skill_path.exists():
            print(f"Error: skill path does not exist: {skill_path}")
            return

        import core.improve.skills_guard as sg

        print(f"Scanning: {skill_path}")
        print(f"Source:   {args.source}")
        print()

        result = sg.scan_skill(skill_path, source=args.source)

        if args.llm_audit:
            print("(Running LLM audit — this may take a moment...)")
            result = sg.llm_audit_skill(skill_path, result)

        print(sg.format_scan_report(result))

        allowed, reason = sg.should_allow_install(result, force=args.force)
        if allowed:
            print(f"\n✓ {reason}")
        elif allowed is None:
            print(f"\n? {reason}")
        else:
            print(f"\n✗ {reason}")


# ───────────────────────────────────────────────────────────────
# CHECKPOINT MANAGER
# ───────────────────────────────────────────────────────────────


def cmd_checkpoint(args):
    """Filesystem snapshot management via CheckpointManager."""
    from pathlib import Path as _Path

    from core.improve.checkpoint_manager import (
        CheckpointManager,
        format_checkpoint_list,
    )

    cm = CheckpointManager(enabled=True)

    if args.ck_cmd == "list":
        directory = str(_Path(args.directory).resolve())
        cps = cm.list_checkpoints(directory)
        print(format_checkpoint_list(cps, directory))

    elif args.ck_cmd == "diff":
        directory = str(_Path(args.directory).resolve())
        result = cm.diff(directory, args.hash)
        if result["success"]:
            if result.get("stat"):
                print(result["stat"])
            if result.get("diff"):
                print(result["diff"])
        else:
            print(f"Error: {result.get('error', 'unknown error')}")

    elif args.ck_cmd == "restore":
        directory = str(_Path(args.directory).resolve())
        file_arg = args.file if hasattr(args, "file") and args.file else None
        result = cm.restore(directory, args.hash, file_arg)
        if result["success"]:
            what = (
                f"file '{result.get('file', 'all files')}'"
                if result.get("file")
                else "all files"
            )
            print(f"Restored {what} to {result['restored_to']} ({result['reason']})")
        else:
            print(f"Error: {result.get('error', 'unknown error')}")


# ───────────────────────────────────────────────────────────────
# DELEGATE TOOL
# ───────────────────────────────────────────────────────────────


def cmd_delegate(args):
    """
    Delegate Tool CLI: spawn subagents, check status, list active sessions.
    """
    from core.improve.delegate_tool import (
        delegate_task,
        get_session_status,
        list_active_sessions,
        spawn_delegate,
    )

    if args.delegate_cmd == "spawn":
        tools = None
        if args.allowed_tools:
            tools = [t.strip() for t in args.allowed_tools.split(",") if t.strip()]

        print(f"Spawning delegate subagent (depth={args.depth})...")
        print(f"  Goal: {args.task[:80]}{'...' if len(args.task) > 80 else ''}")
        if tools:
            print(f"  Allowed tools: {', '.join(tools)}")
        print(f"  Max iterations: {args.max_iterations}")
        print()

        result = spawn_delegate(
            goal=args.task,
            toolsets=tools,
            max_iterations=args.max_iterations,
            depth=args.depth,
        )

        print(f"Session: {result.session_id}")
        print(f"Status: {result.status}")
        print(f"Duration: {result.duration_seconds}s")
        if result.exit_reason:
            print(f"Exit reason: {result.exit_reason}")
        if result.error:
            print(f"Error: {result.error}")
        print()
        print("Summary:")
        print(result.summary or "(no summary)")

    elif args.delegate_cmd == "status":
        status = get_session_status(args.session_id)
        if not status:
            print(f"Session {args.session_id} not found.")
            return
        print(f"Session: {args.session_id}")
        print(f"Status: {status.get('status', 'unknown')}")
        print(f"Duration: {status.get('duration_seconds', 0)}s")
        if status.get("summary"):
            print(f"\nSummary:\n{status['summary']}")
        if status.get("error"):
            print(f"\nError: {status['error']}")

    elif args.delegate_cmd == "list-active":
        sessions = list_active_sessions()
        if not sessions:
            print("No active delegate sessions.")
            return
        print(f"Active sessions: {len(sessions)}\n")
        for s in sessions:
            print(
                f"  {s.get('session_id', '?')}  status={s.get('status', '?')}  duration={s.get('duration_seconds', 0)}s"
            )

    else:
        from argparse import ArgumentParser

        args_parsed = ArgumentParser(prog="harvey orchestrate delegate")
        args_parsed.parse_args([])


# ───────────────────────────────────────────────────────────────
# MAIN
# ───────────────────────────────────────────────────────────────


def main():
    parser = argparse.ArgumentParser(
        prog="harvey orchestrate", description="Harvey Multi-Agent Orchestration"
    )
    sub = parser.add_subparsers(dest="cmd")

    # Workspace
    p_init = sub.add_parser("init", help="Initialize workspace")

    # Task commands
    p_task = sub.add_parser("task", help="Task management")
    task_sub = p_task.add_subparsers(dest="task_cmd")

    tc = task_sub.add_parser("create", help="Create a task")
    tc.add_argument("name", help="Task name")
    tc.add_argument("--description", help="Task description")
    tc.add_argument("--produces", action="append", help="Artifact this task produces")
    tc.add_argument(
        "--depends-on",
        action="append",
        dest="depends_on",
        help="Task IDs this depends on",
    )
    tc.add_argument("--assign-to", dest="assign_to", help="Agent ID to assign")
    tc.add_argument("--parent", help="Parent task ID")

    tl = task_sub.add_parser("list", help="List tasks")
    tstatus = task_sub.add_parser("status", help="Task status")
    tstatus.add_argument("task_id")

    tassign = task_sub.add_parser("assign", help="Assign task to agent")
    tassign.add_argument("task_id")
    tassign.add_argument("agent")

    tcomplete = task_sub.add_parser("complete", help="Mark task complete")
    tcomplete.add_argument("task_id")
    tcomplete.add_argument("--artifact", help="Artifact produced by this task")
    tcomplete.add_argument("--content", help="Artifact content")
    tcomplete.add_argument("--type", default="text/markdown", help="Artifact MIME type")

    tfail = task_sub.add_parser("fail", help="Mark task failed")
    tfail.add_argument("task_id")
    tfail.add_argument("--reason", required=True)

    ttree = task_sub.add_parser("tree", help="Visualize task DAG")
    ttree.add_argument("root_id", nargs="?", help="Root task ID")

    # Artifact commands
    p_art = sub.add_parser("artifact", help="Artifact management")
    art_sub = p_art.add_subparsers(dest="art_cmd")
    al = art_sub.add_parser("list", help="List artifacts")
    ag = art_sub.add_parser("get", help="Get artifact content")
    ag.add_argument("artifact_id")
    ag.add_argument("--max-content", type=int, dest="max_content", default=4000)
    apin = art_sub.add_parser("pin", help="Pin artifact (prevent TTL cleanup)")
    apin.add_argument("artifact_id")

    # Agent commands
    p_agents = sub.add_parser("agents", help="Agent management")
    agents_sub = p_agents.add_subparsers(dest="agent_cmd")
    alist = agents_sub.add_parser("list", help="List agents")
    reg = agents_sub.add_parser("register", help="Register agent")
    reg.add_argument("agent_id")
    reg.add_argument("--type", default="local", help="Agent type (local/remote)")
    reg.add_argument("--command", help="CLI command (for local agents)")
    reg.add_argument("--endpoint", help="API endpoint (for remote agents)")
    reg.add_argument("--capabilities", action="append", help="Agent capabilities")

    # Status
    p_status = sub.add_parser("status", help="Full orchestration dashboard")

    # Review commands (background review after conversations)
    p_review = sub.add_parser(
        "review", help="Trigger background review of a conversation"
    )
    p_review.add_argument(
        "--session-id",
        dest="session_id",
        help="Session ID to review (loads from session DB)",
    )
    p_review.add_argument(
        "--messages-json",
        dest="messages_json",
        help="JSON array of messages to review (alternative to --session-id)",
    )
    p_review.add_argument(
        "--memory-only",
        dest="memory_only",
        action="store_true",
        help="Only review for memory saves (skip skill review)",
    )
    p_review.add_argument(
        "--skills-only",
        dest="skills_only",
        action="store_true",
        help="Only review for skill saves (skip memory review)",
    )
    p_review.add_argument(
        "--force",
        action="store_true",
        help="Fire review immediately, bypassing trigger cooldown",
    )

    # Skills commands (Skills Guard security scanner)
    p_skills = sub.add_parser("skills", help="Skill management and security scanning")
    skills_sub = p_skills.add_subparsers(dest="skills_cmd")
    p_scan = skills_sub.add_parser("scan", help="Scan a skill for security threats")
    p_scan.add_argument(
        "skill_path",
        help="Path to skill directory or SKILL.md file",
    )
    p_scan.add_argument(
        "--source",
        default="community",
        help="Source identifier (e.g. openai/skills, community, agent-created)",
    )
    p_scan.add_argument(
        "--force",
        action="store_true",
        help="Force install even if blocked",
    )
    p_scan.add_argument(
        "--llm-audit",
        action="store_true",
        help="Also run LLM-based security analysis (slower but deeper)",
    )

    # Checkpoint commands (filesystem snapshot safety net)
    p_ck = sub.add_parser("checkpoint", help="Filesystem snapshot management")
    ck_sub = p_ck.add_subparsers(dest="ck_cmd")

    p_ck_list = ck_sub.add_parser("list", help="List checkpoints for a directory")
    p_ck_list.add_argument(
        "directory", help="Working directory to list checkpoints for"
    )

    p_ck_diff = ck_sub.add_parser(
        "diff", help="Show diff between checkpoint and current state"
    )
    p_ck_diff.add_argument("directory", help="Working directory")
    p_ck_diff.add_argument("hash", help="Commit hash (or short hash)")

    p_ck_restore = ck_sub.add_parser("restore", help="Restore files from a checkpoint")
    p_ck_restore.add_argument("directory", help="Working directory")
    p_ck_restore.add_argument("hash", help="Commit hash (or short hash)")
    p_ck_restore.add_argument(
        "file", nargs="?", help="Optional: restore only this file"
    )

    # Delegate commands (subagent spawning)
    p_delegate = sub.add_parser("delegate", help="Delegate Tool — spawn subagents")
    delegate_sub = p_delegate.add_subparsers(dest="delegate_cmd")

    p_delegate_spawn = delegate_sub.add_parser(
        "spawn", help="Spawn a delegate subagent"
    )
    p_delegate_spawn.add_argument(
        "task", help="Task description / goal for the subagent"
    )
    p_delegate_spawn.add_argument(
        "--allowed-tools",
        dest="allowed_tools",
        help="Comma-separated list of allowed toolsets (e.g. terminal,file,web)",
    )
    p_delegate_spawn.add_argument(
        "--depth",
        type=int,
        default=0,
        help="Delegation depth (default 0, max 2)",
    )
    p_delegate_spawn.add_argument(
        "--max-iterations",
        type=int,
        dest="max_iterations",
        default=50,
        help="Max iterations per subagent (default 50)",
    )

    p_delegate_status = delegate_sub.add_parser(
        "status", help="Get status of a delegate session"
    )
    p_delegate_status.add_argument(
        "--session-id",
        dest="session_id",
        required=True,
        help="Session ID to check",
    )

    p_delegate_list = delegate_sub.add_parser(
        "list-active", help="List all active delegate sessions"
    )

    args = parser.parse_args()

    if not args.cmd:
        parser.print_help()
        return

    ensure_dirs()

    if args.cmd == "init":
        cmd_init(args)
    elif args.cmd == "task":
        if args.task_cmd == "create":
            task_id = cmd_task_create(args)
            if getattr(args, "assign_to", None):
                args2 = argparse.Namespace(task_id=task_id, agent=args.assign_to)
                cmd_task_assign(args2)
        elif args.task_cmd == "list":
            cmd_task_list(args)
        elif args.task_cmd == "status":
            cmd_task_status(args)
        elif args.task_cmd == "assign":
            cmd_task_assign(args)
        elif args.task_cmd == "complete":
            cmd_task_complete(args)
        elif args.task_cmd == "fail":
            cmd_task_fail(args)
        elif args.task_cmd == "tree":
            cmd_task_tree(args)
        else:
            task_sub.print_help()
    elif args.cmd == "artifact":
        if args.art_cmd == "list":
            cmd_artifact_list(args)
        elif args.art_cmd == "get":
            cmd_artifact_get(args)
        elif args.art_cmd == "pin":
            cmd_artifact_pin(args)
        else:
            art_sub.print_help()
    elif args.cmd == "agents":
        if args.agent_cmd == "list":
            cmd_agents_list(args)
        elif args.agent_cmd == "register":
            cmd_agents_register(args)
        else:
            agents_sub.print_help()
    elif args.cmd == "status":
        cmd_status(args)
    elif args.cmd == "review":
        cmd_review(args)
    elif args.cmd == "skills":
        cmd_skills(args)
    elif args.cmd == "checkpoint":
        cmd_checkpoint(args)
    elif args.cmd == "delegate":
        cmd_delegate(args)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
