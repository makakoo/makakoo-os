#!/usr/bin/env python3
"""
Stage 1: Knowledge Miner — fetches from Gmail and Google Calendar via gws.

Reads recent emails and calendar events, saves raw JSON to data/knowledge-extraction/raw/.

Usage:
    python3 miner.py [--days 7]
"""

import os
import sys
import json
import shutil
import subprocess
import argparse
from datetime import datetime, timedelta

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
_HH_FALLBACK = os.path.dirname(os.path.dirname(os.path.dirname(SCRIPT_DIR)))
HARVEY_HOME = os.environ.get("HARVEY_HOME", _HH_FALLBACK)
HARVEY_HOME = os.path.realpath(HARVEY_HOME)

RAW_DIR = os.path.join(HARVEY_HOME, "data", "knowledge-extraction", "raw")
os.makedirs(RAW_DIR, exist_ok=True)

GWS_PATH = os.environ.get("GWS_PATH") or shutil.which("gws") or "gws"


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] miner: {msg}")


def run_gws(args: list) -> str:
    cmd = [GWS_PATH] + args
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if result.returncode != 0:
        log(f"gws error: {'; '.join(result.stderr.strip().splitlines())}")
        return ""
    return result.stdout


def mine_emails(days: int = 7) -> dict:
    """Fetch emails from the last N days."""
    query = f"newer_than:{days}d -from:noreply -from:notifications"
    params = json.dumps({"userId": "me", "query": query, "maxResults": 100})
    output = run_gws([
        "gmail", "users", "messages", "list",
        "--params", params,
        "--format", "json",
    ])
    if not output:
        return {"source": "gmail", "query": query, "messages": [], "fetched_at": datetime.now().isoformat()}

    try:
        data = json.loads(output)
        msg_ids = data.get("messages", []) or []
    except json.JSONDecodeError:
        log("Failed to parse Gmail list response")
        return {"source": "gmail", "query": query, "messages": [], "fetched_at": datetime.now().isoformat()}

    messages = []
    for m in msg_ids[:50]:  # cap at 50 for speed
        msg_id = m["id"]
        get_params = json.dumps({"userId": "me", "id": msg_id})
        detail = run_gws([
            "gmail", "users", "messages", "get",
            "--params", get_params,
        ])
        if detail:
            try:
                messages.append(json.loads(detail))
            except json.JSONDecodeError:
                pass

    log(f"Mined {len(messages)} emails from last {days} days")
    return {
        "source": "gmail",
        "query": query,
        "days": days,
        "messages": messages,
        "fetched_at": datetime.now().isoformat(),
    }


def mine_calendar(days: int = 7) -> dict:
    """Fetch calendar events for the next N days."""
    time_min = datetime.now().strftime("%Y-%m-%dT%H:%M:%SZ")
    time_max = (datetime.now() + timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ")

    params = json.dumps({"calendarId": "primary", "timeMin": time_min, "timeMax": time_max, "maxResults": 50})
    output = run_gws([
        "calendar", "events", "list",
        "--params", params,
        "--format", "json",
    ])
    if not output:
        return {"source": "calendar", "events": [], "fetched_at": datetime.now().isoformat()}

    try:
        data = json.loads(output)
        events = data if isinstance(data, list) else data.get("items", [])
    except json.JSONDecodeError:
        log("Failed to parse Calendar response")
        return {"source": "calendar", "events": [], "fetched_at": datetime.now().isoformat()}

    log(f"Mined {len(events)} calendar events")
    return {
        "source": "calendar",
        "events": events,
        "fetched_at": datetime.now().isoformat(),
    }


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--days", type=int, default=7, help="Days to look back (default: 7)")
    args = parser.parse_args()

    log(f"Starting knowledge miner for last {args.days} days")
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")

    email_data = mine_emails(args.days)
    cal_data = mine_calendar(args.days)

    # Save raw
    email_file = os.path.join(RAW_DIR, f"emails_{ts}.json")
    cal_file = os.path.join(RAW_DIR, f"calendar_{ts}.json")

    with open(email_file, "w") as f:
        json.dump(email_data, f, indent=2, default=str)
    log(f"Saved emails to {email_file}")

    with open(cal_file, "w") as f:
        json.dump(cal_data, f, indent=2, default=str)
    log(f"Saved calendar to {cal_file}")

    # Write latest pointer
    latest = {"email_file": email_file, "calendar_file": cal_file, "mined_at": datetime.now().isoformat()}
    with open(os.path.join(RAW_DIR, "latest.json"), "w") as f:
        json.dump(latest, f, indent=2)

    log("Miner complete")


if __name__ == "__main__":
    run()
