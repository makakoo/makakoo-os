#!/usr/bin/env python3
"""
PostgreSQL Cluster Watchdog — runs every 15 min via cron.

Checks all PostgreSQL clusters via pg_lsclusters.
If any are down:
  1. Try pg_ctlcluster <ver> <name> start
  2. If that fails, read the log, ask local AI (switchAILocal) to diagnose
  3. Apply AI's suggested fix command (single safe pg_ctlcluster command only)
  4. Log everything to data/logs/pg_watchdog.log + Logseq Brain journal
"""

import json
import os
import re
import subprocess
import sys
from datetime import datetime

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HARVEY_HOME = os.environ.get(
    "HARVEY_HOME", os.path.realpath(os.path.join(SCRIPT_DIR, "..", ".."))
)

LOG_DIR = os.path.join(HARVEY_HOME, "data", "logs")
LOG_FILE = os.path.join(LOG_DIR, "pg_watchdog.log")
BRAIN_JOURNALS = os.path.join(HARVEY_HOME, "data", "Brain", "journals")

AI_URL = (
    os.environ.get("SWITCHAI_URL", "http://localhost:18080") + "/v1/chat/completions"
)
AI_KEY = os.environ.get("SWITCHAI_KEY", "")
AI_MODEL = os.environ.get("LLM_MODEL", "minimax:MiniMax-M2.7")

# Only these commands are allowed to be executed from AI suggestions
SAFE_CMD_PREFIX = "pg_ctlcluster"
MAX_RESTART_ATTEMPTS = 2
MAX_AI_ATTEMPTS = 1
LOG_TAIL_LINES = 50

os.makedirs(LOG_DIR, exist_ok=True)


def log(msg):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_FILE, "a") as f:
        f.write(line + "\n")


def run_cmd(cmd, timeout=30):
    """Run a shell command and return (returncode, stdout, stderr)."""
    try:
        r = subprocess.run(
            cmd, shell=True, capture_output=True, text=True, timeout=timeout
        )
        return r.returncode, r.stdout.strip(), r.stderr.strip()
    except subprocess.TimeoutExpired:
        return -1, "", "timeout"
    except Exception as e:
        return -1, "", str(e)


def parse_clusters():
    """Parse pg_lsclusters output into list of dicts."""
    rc, stdout, stderr = run_cmd("pg_lsclusters")
    if rc != 0:
        log(f"ERROR: pg_lsclusters failed: {stderr}")
        return []

    clusters = []
    for line in stdout.splitlines()[1:]:  # skip header
        parts = line.split()
        if len(parts) >= 6:
            clusters.append(
                {
                    "ver": parts[0],
                    "name": parts[1],
                    "port": parts[2],
                    "status": parts[3],
                    "owner": parts[4],
                    "datadir": parts[5],
                    "logfile": parts[6] if len(parts) > 6 else "",
                }
            )
    return clusters


def try_restart(ver, name):
    """Attempt pg_ctlcluster start. Returns (success, stdout, stderr)."""
    cmd = f"pg_ctlcluster {ver} {name} start"
    log(f"  Attempting: {cmd}")
    rc, stdout, stderr = run_cmd(cmd, timeout=30)
    if rc == 0:
        log(f"  SUCCESS: {name} started")
        return True, stdout, stderr
    else:
        log(f"  FAILED (rc={rc}): {stderr}")
        return False, stdout, stderr


def get_log_tail(logfile, n=LOG_TAIL_LINES):
    """Read last N lines of a PG log file."""
    if not logfile or not os.path.exists(logfile):
        return "(log file not found)"
    try:
        rc, stdout, _ = run_cmd(f"tail -n {n} '{logfile}'")
        return stdout if rc == 0 else "(failed to read log)"
    except Exception:
        return "(failed to read log)"


def ai_diagnose(ver, name, port, stderr_output, log_tail):
    """Ask local AI to diagnose the failure and suggest a fix command."""
    prompt = f"""You are a PostgreSQL DBA. A cluster failed to start. Diagnose and suggest ONE fix command.

Cluster: PostgreSQL {ver}, name="{name}", port={port}
Start command failed: pg_ctlcluster {ver} {name} start
Error output:
{stderr_output}

Last {LOG_TAIL_LINES} lines of PostgreSQL log:
{log_tail}

Rules:
- Suggest exactly ONE shell command to fix this
- The command MUST start with "pg_ctlcluster" (e.g. pg_ctlcluster {ver} {name} start -- -o "...")
- If the fix requires something else (disk space, permissions, config edit), explain what but still give the best pg_ctlcluster command to try
- Do NOT suggest destructive commands (DROP, rm -rf, etc.)

Reply in this exact JSON format:
{{"diagnosis": "one sentence explanation", "fix_command": "pg_ctlcluster ...", "confidence": "high/medium/low"}}"""

    payload = {
        "model": AI_MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 500,
        "thinking": {"type": "enable"},
        "tools": [],
        "tool_choice": "auto",
    }

    try:
        rc, stdout, stderr = run_cmd(
            f"curl -s --max-time 60 -X POST '{AI_URL}' "
            f"-H 'Content-Type: application/json' "
            f"-H 'Authorization: Bearer {AI_KEY}' "
            f"-d '{json.dumps(payload)}'",
            timeout=90,
        )
        if rc != 0:
            log(f"  AI call failed: {stderr}")
            return None

        resp = json.loads(stdout)
        content = resp.get("choices", [{}])[0].get("message", {}).get("content", "")
        if not content:
            log("  AI returned empty response")
            return None

        # Extract JSON from response
        start = content.find("{")
        end = content.rfind("}") + 1
        if start >= 0 and end > start:
            result = json.loads(content[start:end])
            log(f"  AI diagnosis: {result.get('diagnosis', '?')}")
            log(f"  AI fix: {result.get('fix_command', '?')}")
            log(f"  AI confidence: {result.get('confidence', '?')}")
            return result

        log(f"  AI response not parseable: {content[:200]}")
        return None

    except json.JSONDecodeError as e:
        log(f"  AI JSON parse error: {e}")
        return None
    except Exception as e:
        log(f"  AI error: {e}")
        return None


def validate_and_execute_fix(fix_command, ver, name):
    """Validate AI-suggested command is safe, then execute."""
    if not fix_command:
        return False

    cmd = fix_command.strip()

    # Safety: must start with pg_ctlcluster
    if not cmd.startswith(SAFE_CMD_PREFIX):
        log(f"  REJECTED unsafe command: {cmd}")
        return False

    # Safety: no pipes, semicolons, backticks, $() — prevent injection
    if any(c in cmd for c in ["|", ";", "`", "$(", "&&", "||", ">", "<"]):
        log(f"  REJECTED command with shell operators: {cmd}")
        return False

    # Safety: must reference the correct cluster
    if ver not in cmd or name not in cmd:
        log(f"  REJECTED command for wrong cluster: {cmd}")
        return False

    log(f"  Executing AI fix: {cmd}")
    rc, stdout, stderr = run_cmd(cmd, timeout=30)
    if rc == 0:
        log(f"  AI FIX SUCCESS")
        return True
    else:
        log(f"  AI FIX FAILED (rc={rc}): {stderr}")
        return False


def verify_cluster_up(ver, name):
    """Check if cluster is now online."""
    clusters = parse_clusters()
    for c in clusters:
        if c["ver"] == ver and c["name"] == name:
            return c["status"] == "online"
    return False


def log_to_brain(events):
    """Write significant events to today's Logseq journal."""
    if not events:
        return
    today = datetime.now().strftime("%Y_%m_%d")
    journal_file = os.path.join(BRAIN_JOURNALS, f"{today}.md")
    try:
        lines = []
        for event in events:
            lines.append(f"- [[pg-watchdog]] {event}")
        with open(journal_file, "a") as f:
            f.write("\n".join(lines) + "\n")
        log(f"  Logged {len(events)} events to Brain journal")
    except Exception as e:
        log(f"  Brain journal write failed: {e}")


def run():
    log("=== PG Watchdog starting ===")

    clusters = parse_clusters()
    if not clusters:
        log("No clusters found or pg_lsclusters failed")
        return

    all_online = True
    brain_events = []

    for c in clusters:
        status_str = f"PG {c['ver']}/{c['name']} port={c['port']} → {c['status']}"
        if c["status"] == "online":
            log(f"  OK: {status_str}")
            continue

        all_online = False
        log(f"  DOWN: {status_str}")
        brain_events.append(f"{c['ver']}/{c['name']} (port {c['port']}) found DOWN")

        # Step 1: Try simple restart
        success = False
        last_stderr = ""
        for attempt in range(MAX_RESTART_ATTEMPTS):
            ok, stdout, stderr = try_restart(c["ver"], c["name"])
            last_stderr = stderr
            if ok and verify_cluster_up(c["ver"], c["name"]):
                success = True
                brain_events.append(
                    f"{c['ver']}/{c['name']} restarted successfully (attempt {attempt + 1})"
                )
                break

        if success:
            continue

        # Step 2: AI-assisted diagnosis
        log(f"  Simple restart failed. Calling AI for diagnosis...")
        log_tail = get_log_tail(c["logfile"])

        for ai_attempt in range(MAX_AI_ATTEMPTS):
            diagnosis = ai_diagnose(
                c["ver"], c["name"], c["port"], last_stderr, log_tail
            )
            if not diagnosis:
                log("  AI diagnosis unavailable — giving up")
                brain_events.append(
                    f"{c['ver']}/{c['name']} restart FAILED, AI unavailable"
                )
                break

            fix_cmd = diagnosis.get("fix_command", "")
            diag_text = diagnosis.get("diagnosis", "unknown")

            if validate_and_execute_fix(fix_cmd, c["ver"], c["name"]):
                if verify_cluster_up(c["ver"], c["name"]):
                    brain_events.append(
                        f"{c['ver']}/{c['name']} recovered via AI fix: {diag_text}"
                    )
                    break
                else:
                    log("  AI fix executed but cluster still down")
                    brain_events.append(
                        f"{c['ver']}/{c['name']} AI fix failed: {diag_text} — NEEDS MANUAL INTERVENTION"
                    )
            else:
                brain_events.append(
                    f"{c['ver']}/{c['name']} STILL DOWN — AI diagnosis: {diag_text} — NEEDS MANUAL INTERVENTION"
                )

    if all_online:
        log(f"  All {len(clusters)} clusters online")

    if brain_events:
        log_to_brain(brain_events)

    log("=== PG Watchdog done ===\n")


if __name__ == "__main__":
    run()
