#!/usr/bin/env python3
"""
Stage 2: Knowledge Extractor — LLM processing of mined raw data.

Reads raw JSON from miner.py output, calls Gemini to extract structured facts
into 4 categories: entities, preferences, insights, events.

Writes to data/knowledge-extraction/learned_facts.json.

Usage:
    GEMINI_API_KEY=... python3 extractor.py [--model gemini-2.0-flash]
"""

import os
import sys
import json
import argparse
from datetime import datetime

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
HARVEY_HOME = os.environ.get(
    "HARVEY_HOME",
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(SCRIPT_DIR))))
)
HARVEY_HOME = os.path.realpath(HARVEY_HOME)

RAW_DIR = os.path.join(HARVEY_HOME, "data", "knowledge-extraction", "raw")
OUTPUT_FILE = os.path.join(HARVEY_HOME, "data", "knowledge-extraction", "learned_facts.json")

GEMINI_API_KEY = os.environ.get("GEMINI_API_KEY", "")
GEMINI_MODEL = os.environ.get("GEMINI_MODEL", "gemini-2.0-flash")


PROMPT = """You are an AI assistant analyzing a person's email and calendar data to extract useful knowledge.
Extract ONLY information that is NEW or DIFFERENT from what's already known.

Return a JSON object with exactly these 4 keys:

{
  "entities": [
    // People, companies, projects mentioned. Format: "Name: Description"
    // Only include if specific and actionable (not generic)
  ],
  "preferences": [
    // Stated or inferred preferences, working styles, communication preferences
    // Format: "Name: Preference detail"
  ],
  "insights": [
    // Strategic observations about goals, priorities, focus areas
    // Format: "Insight: detail"
  ],
  "events": [
    // Scheduled events, meetings, deadlines with dates
    // Format: "YYYY-MM-DD: Event title"
  ]
}

Rules:
- Only extract things you are CONFIDENT about (not guesses)
- Skip generic corporate newsletters (IBM, SAP, etc.)
- Focus on RECRUITER conversations, JOB opportunities, MEETINGS with people
- People names: extract full name + context
- Max 5 items per category — only the most important
- If nothing worth extracting: return all arrays empty
- Return ONLY the JSON, no preamble, no explanation
"""


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] extractor: {msg}")


def build_email_context(email_data: dict) -> str:
    """Flatten emails into a readable context string."""
    lines = []
    for msg in email_data.get("messages", []):
        headers = {h["name"]: h["value"] for h in msg.get("payload", {}).get("headers", [])}
        sender = headers.get("From", "Unknown")
        subject = headers.get("Subject", "(no subject)")
        snippet = msg.get("snippet", "")
        lines.append(f"From: {sender}\nSubject: {subject}\nPreview: {snippet}\n")
    return "\n".join(lines[:50])  # cap at 50 emails


def build_calendar_context(cal_data: dict) -> str:
    """Flatten calendar events into a readable context string."""
    lines = []
    for evt in cal_data.get("events", []):
        title = evt.get("summary", "(no title)")
        start = evt.get("start", {}).get("dateTime", evt.get("start", {}).get("date", ""))
        end = evt.get("end", {}).get("dateTime", "")
        attendees = evt.get("attendees", [])
        attendee_names = [a.get("displayName", a.get("email", "")) for a in attendees[:5]]
        lines.append(f"Event: {title} | Start: {start} | End: {end} | Attendees: {', '.join(attendee_names)}")
    return "\n".join(lines)


def call_gemini(prompt: str, context: str) -> dict:
    """Call Gemini API with the extraction prompt + context."""
    if not GEMINI_API_KEY:
        log("GEMINI_API_KEY not set — using empty extraction")
        return {"entities": [], "preferences": [], "insights": [], "events": []}

    import urllib.request
    import urllib.parse

    url = f"https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL}:generateContent?key={GEMINI_API_KEY}"
    payload = json.dumps({
        "contents": [{"parts": [{"text": f"{prompt}\n\n--- DATA TO ANALYZE ---\n{context}"}]}],
        "generationConfig": {"responseMimeType": "application/json", "maxOutputTokens": 2048}
    }).encode()

    req = urllib.request.Request(
        url, data=payload,
        headers={"Content-Type": "application/json"},
        method="POST"
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            result = json.loads(resp.read())
            text = result["candidates"][0]["content"]["parts"][0]["text"]
            return json.loads(text)
    except Exception as e:
        log(f"Gemini API error: {e}")
        return {"entities": [], "preferences": [], "insights": [], "events": []}


def load_latest_raw() -> tuple[dict, dict]:
    """Load the latest email and calendar JSON from raw/."""
    latest_path = os.path.join(RAW_DIR, "latest.json")
    if not os.path.exists(latest_path):
        log("No latest.json — run miner.py first")
        return {}, {}

    with open(latest_path) as f:
        latest = json.load(f)

    email_data = {}
    cal_data = {}

    if os.path.exists(latest.get("email_file", "")):
        with open(latest["email_file"]) as f:
            email_data = json.load(f)

    if os.path.exists(latest.get("calendar_file", "")):
        with open(latest["calendar_file"]) as f:
            cal_data = json.load(f)

    return email_data, cal_data


def load_existing_facts() -> dict:
    """Load existing learned_facts.json so we don't re-extract known facts."""
    if os.path.exists(OUTPUT_FILE):
        with open(OUTPUT_FILE) as f:
            return json.load(f)
    return {"entities": [], "preferences": [], "insights": [], "events": [], "last_updated": ""}


def save_facts(facts: dict):
    facts["last_updated"] = datetime.now().isoformat()
    with open(OUTPUT_FILE, "w") as f:
        json.dump(facts, f, indent=2)
    log(f"Saved {len(facts['entities'])} entities, {len(facts['preferences'])} prefs, "
        f"{len(facts['insights'])} insights, {len(facts['events'])} events to learned_facts.json")


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=GEMINI_MODEL)
    args = parser.parse_args()

    log("Starting knowledge extractor")
    email_data, cal_data = load_latest_raw()

    if not email_data and not cal_data:
        log("No raw data found — run miner.py first")
        return

    # Build context
    email_context = build_email_context(email_data)
    cal_context = build_calendar_context(cal_data)
    context = f"=== EMAILS ===\n{email_context}\n\n=== CALENDAR ===\n{cal_context}"

    # Call LLM
    facts = call_gemini(PROMPT, context)
    existing = load_existing_facts()

    # Deduplicate new facts against existing
    for key in ["entities", "preferences", "insights", "events"]:
        new_items = facts.get(key, [])
        existing_items = set(existing.get(key, []))
        unique_new = [item for item in new_items if item not in existing_items]
        existing[key].extend(unique_new)

    save_facts(existing)
    log("Extraction complete")


if __name__ == "__main__":
    run()
