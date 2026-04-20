#!/usr/bin/env python3
"""
Inbox Watchdog — runs every 30 min via cron.

Fetches unread emails via gws, saves batch JSON to data/inbox-triage/raw/,
filters noise (LinkedIn, newsletters), logs to inbox_watchdog.log.

Usage:
    python3 inbox_watchdog.py [--dry-run]
"""

import os
import sys
import json
import subprocess
import shutil
import re
import argparse
from datetime import datetime, timedelta

# --- Dynamic HARVEY_HOME resolution ---
# Script is at harvey-os/skills/inbox-triage/inbox_watchdog.py (4 levels below HARVEY_HOME)
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
_HH_FALLBACK = os.path.dirname(os.path.dirname(os.path.dirname(SCRIPT_DIR)))  # up 3 from SCRIPT_DIR = HARVEY_HOME
HARVEY_HOME = os.environ.get("HARVEY_HOME", _HH_FALLBACK)
HARVEY_HOME = os.path.realpath(HARVEY_HOME)

DATA_DIR = os.path.join(HARVEY_HOME, "data", "inbox-triage")
RAW_DIR = os.path.join(DATA_DIR, "raw")
LOG_DIR = os.path.join(HARVEY_HOME, "data", "logs")
LOG_FILE = os.path.join(LOG_DIR, "inbox_watchdog.log")

# Ensure dirs exist
os.makedirs(RAW_DIR, exist_ok=True)
os.makedirs(LOG_DIR, exist_ok=True)

# Resolve gws
GWS_PATH = os.environ.get("GWS_PATH") or shutil.which("gws") or "gws"

# --- Noise patterns ---
NOISE_PATTERNS = [
    r"linkedin|linked\.com",          # LinkedIn job alerts/newsletters
    r"noreply|no-reply",              # Automated no-reply
    r"notify@|notification@",         # System notifications
    r"newsletter|campaign|mailchimp", # Email marketing
    r"unsubscribe",                    # Marketing footer
    r"^Job Alerts?$",                  # Subject is just "Job Alert"
    r"via linkedin",                   # LinkedIn mailer header
]

# Recruiter signal patterns (direct, human-written emails)
RECRUITER_SIGNALS = [
    r"\bI've\b.{0,60}\byour\b.{0,60}\bprofile\b",
    r"\bread your (profile|portfolio|github|cv|resume)\b",
    r"\bfound your (profile|portfolio|github|cv|resume)\b",
    r"\breaching out (about|for|to discuss)\b",
    r"\binterested in\b.{0,40}\bcandidates?\b",
    r"\brole\b.{0,30}\b( Berlin|Germany|Remote|Europe|UK|USA)\b",
    r"\b(interview|call|chat|meet)\b.{0,40}\b(purpose|discuss|learn)\b",
    r"\b(startup|company|team)\b.{0,40}\b(building|growing|hiring)\b",
    r"\bCTO|CEO|COO|VP|Head of\b",
    r"\bsalary|budget|rate|compensation\b",
    r"\b@\b.{0,20}\.com$",  # Personal email domain (not corporate like ibm.com)
]

SKIP_FROM = [
    "notifications@linkedin.com",
    "jobs-noreply@linkedin.com",
    "invitations@linkedin.com",
    "messages-noreply@linkedin.com",
    "auto-confirm@coinbase.com",
]

SKIP_SUBJECT_PREFIXES = [
    "Job Alert",
    "Newsletter",
    "Update:",
    "Digest:",
]


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{ts}] {msg}"
    print(line)
    with open(LOG_FILE, "a") as f:
        f.write(line + "\n")


def run_gws(args: list) -> str:
    """Run a gws subcommand, return stdout or empty string on failure."""
    cmd = [GWS_PATH] + args
    result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
    if result.returncode != 0:
        log(f"gws error: {result.stderr.strip()}")
        return ""
    return result.stdout


def fetch_unread_emails() -> list:
    """Fetch recent unread emails from Gmail via gws."""
    # Use --params JSON with userId embedded (gws CLI style)
    list_params = json.dumps({"userId": "me", "query": "newer_than:1d is:unread in:inbox", "maxResults": 50})
    output = run_gws([
        "gmail", "users", "messages", "list",
        "--params", list_params,
        "--format", "json",
    ])
    if not output:
        return []

    try:
        data = json.loads(output)
        msg_ids = data.get("messages", []) or []
    except json.JSONDecodeError:
        log(f"Failed to parse gws list response: {output[:200]}")
        return []

    emails = []
    for m in msg_ids[:20]:  # cap at 20 for speed
        msg_id = m["id"]
        get_params = json.dumps({"userId": "me", "id": msg_id})
        detail = run_gws([
            "gmail", "users", "messages", "get",
            "--params", get_params,
        ])
        if detail:
            try:
                emails.append(json.loads(detail))
            except json.JSONDecodeError:
                log(f"Failed to parse message {msg_id}")
    return emails


def extract_header(headers: list, name: str) -> str:
    for h in headers:
        if h.get("name", "").lower() == name.lower():
            return h.get("value", "")
    return ""


def is_noise(msg: dict) -> tuple[bool, str]:
    """Return (is_noise, reason)."""
    payload = msg.get("payload", {})
    headers = payload.get("headers", [])

    sender = extract_header(headers, "From")
    subject = extract_header(headers, "Subject")

    # Skip explicit no-reply senders
    for skip in SKIP_FROM:
        if skip.lower() in sender.lower():
            return True, f"skip sender: {skip}"

    # Skip specific subject prefixes
    for prefix in SKIP_SUBJECT_PREFIXES:
        if subject.lower().startswith(prefix.lower()):
            return True, f"skip subject prefix: {prefix}"

    # Pattern-based noise
    for pattern in NOISE_PATTERNS:
        if re.search(pattern, sender + " " + subject, re.IGNORECASE):
            return True, f"noise pattern: {pattern}"

    return False, ""


def is_recruiter_signal(msg: dict) -> tuple[bool, str]:
    """Return (is_recruiter, matched_pattern)."""
    payload = msg.get("payload", {})
    headers = payload.get("headers", [])
    sender = extract_header(headers, "From")
    subject = extract_header(headers, "Subject")
    snippet = msg.get("snippet", "")

    text = f"{sender} {subject} {snippet}"
    for pattern in RECRUITER_SIGNALS:
        if re.search(pattern, text, re.IGNORECASE):
            return True, pattern
    return False, ""


def triage_email(msg: dict) -> dict:
    """Classify a single email message."""
    payload = msg.get("payload", {})
    headers = payload.get("headers", [])
    sender = extract_header(headers, "From")
    subject = extract_header(headers, "Subject")
    snippet = msg.get("snippet", "")

    noise, noise_reason = is_noise(msg)
    if noise:
        return {"tier": "noise", "reason": noise_reason, "sender": sender, "subject": subject}

    recruiter, recruiter_pattern = is_recruiter_signal(msg)
    if recruiter:
        return {"tier": "recruiter", "sender": sender, "subject": subject,
                "snippet": snippet, "pattern": recruiter_pattern, "msg_id": msg["id"]}

    # Check if it has a reasonable sender (not generic corporate)
    if "@" in sender:
        domain = sender.split("@")[-1].split(">")[0].lower()
        common_corp = ["google.com", "microsoft.com", "amazon.com", "meta.com",
                       "apple.com", "ibm.com", "sap.com", "siemens.com", "deutsche-bank.com"]
        if domain not in common_corp:
            return {"tier": "direct", "sender": sender, "subject": subject,
                    "snippet": snippet, "msg_id": msg["id"]}

    return {"tier": "other", "sender": sender, "subject": subject, "snippet": snippet, "msg_id": msg["id"]}


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    log("=== Inbox Watchdog starting ===")
    if args.dry_run:
        log("DRY RUN — no emails will be synced")

    emails = fetch_unread_emails()
    log(f"Fetched {len(emails)} unread emails")

    if not emails:
        log("No emails fetched — gws may be unavailable or no new mail")
        return

    # Save raw batch
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    batch_file = os.path.join(RAW_DIR, f"inbox_batch_{ts}.json")
    with open(batch_file, "w") as f:
        json.dump(emails, f, indent=2, default=str)
    log(f"Saved raw batch to {batch_file}")

    # Triage
    results = [triage_email(e) for e in emails]
    noise = [r for r in results if r["tier"] == "noise"]
    recruiters = [r for r in results if r["tier"] == "recruiter"]
    direct = [r for r in results if r["tier"] == "direct"]
    other = [r for r in results if r["tier"] == "other"]

    log(f"Triage: {len(noise)} noise | {len(recruiters)} recruiter | {len(direct)} direct | {len(other)} other")

    if recruiters:
        log("== RECRUITER SIGNALS ==")
        for r in recruiters:
            log(f"  [{r['sender']}] {r['subject']}")
            # Sync to brain (import lazily to avoid hard dep on logseq_bridge)
            if not args.dry_run:
                try:
                    brain_path = os.path.join(HARVEY_HOME, "harvey-os", "skills", "logseq-brain")
                    sys.path.insert(0, brain_path)
                    from logseq_bridge import sync_inbound_to_brain
                    sync_inbound_to_brain(
                        sender=r["sender"],
                        subject=r["subject"],
                        snippet=r.get("snippet", ""),
                        msg_id=r["msg_id"],
                        urgency="High"
                    )
                    log(f"  → Synced to Logseq Brain")
                except Exception as e:
                    log(f"  → Brain sync failed: {e}")
                finally:
                    sys.path.pop(0)

    if direct:
        log("== DIRECT EMAILS ==")
        for d in direct:
            log(f"  [{d['sender']}] {d['subject']}")

    log(f"=== Inbox Watchdog done — {len(emails)} processed ===\n")


if __name__ == "__main__":
    run()
