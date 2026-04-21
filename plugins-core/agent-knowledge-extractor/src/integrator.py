#!/usr/bin/env python3
"""
Stage 3: Knowledge Integrator — writes extracted facts to Logseq Brain.

Reads learned_facts.json, creates/updates Logseq pages:
  - Person pages: [[Person - {Name}]]
  - Event pages: [[Event - {Title} - {Date}]]
  - Project pages: [[Project - {Name}]]
  - Sebastian - Preferences (upserts)
  - Sebastian - Insights (upserts)
  - Today's journal (summary entry)

Usage:
    python3 integrator.py [--dry-run]
"""

import os
import sys
import json
import re
import argparse
from datetime import datetime

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
_HH_FALLBACK = os.path.dirname(os.path.dirname(os.path.dirname(SCRIPT_DIR)))
HARVEY_HOME = os.environ.get("HARVEY_HOME", _HH_FALLBACK)
HARVEY_HOME = os.path.realpath(HARVEY_HOME)

FACTS_FILE = os.path.join(HARVEY_HOME, "data", "knowledge-extraction", "learned_facts.json")


def log(msg: str):
    ts = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] integrator: {msg}")


def load_facts() -> dict:
    if not os.path.exists(FACTS_FILE):
        log("No learned_facts.json found — run extractor.py first")
        return {}
    with open(FACTS_FILE) as f:
        return json.load(f)


def extract_name(text: str) -> str:
    """Try to extract a person name from 'Name: Description' format."""
    return text.split(":")[0].strip()


def create_person_page(name: str, description: str, source: str = "") -> dict:
    """Create a Person page."""
    page_name = f"Person - {name}"
    props = {"type": "person", "source": source or "knowledge-extraction"}
    content = f"- # {name}\n- **Description:** {description}\n"
    return {"page": page_name, "props": props, "content": content}


def create_event_page(event_str: str) -> dict:
    """Create an Event page from 'YYYY-MM-DD: Event title' format."""
    match = re.match(r"^(\d{4}-\d{2}-\d{2}):\s*(.+)$", event_str)
    if not match:
        return None
    date_part, title = match.groups()
    page_name = f"Event - {title[:50]} - {date_part}"
    props = {"type": "event", "date": date_part, "source": "knowledge-extraction"}
    content = f"- # {title}\n- **Date:** {date_part}\n"
    return {"page": page_name, "props": props, "content": content}


def create_project_page(project_str: str) -> dict:
    """Create a Project page from 'Project: Name' or 'Project - Name' format."""
    name = project_str.replace("Project:", "").replace("Project -", "").strip()
    if not name:
        return None
    page_name = f"Project - {name}"
    props = {"type": "project", "source": "knowledge-extraction"}
    content = f"- # {name}\n- **Source:** knowledge-extraction pipeline\n"
    return {"page": page_name, "props": props, "content": content}


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    log("Starting knowledge integrator")
    facts = load_facts()
    if not facts:
        return

    # Direct import — core.memory.brain_bridge lives in lib-harvey-core
    # and is already on PYTHONPATH via the kernel's skill_runner env.
    try:
        from core.memory.brain_bridge import (
            create_page, append_block, upsert_property,
            upsert_page_properties, page_exists,
            log_to_today_journal, sync_inbound_to_brain,
        )
    except ImportError as e:
        log(f"Failed to import core.memory.brain_bridge: {e}")
        log("Integrator requires lib-harvey-core — install with `makakoo plugin install lib-harvey-core`")
        return

    def create_or_upsert(page, props, content):
        """Idempotent page create: upsert props if page exists, else create fresh."""
        if page_exists(page):
            return upsert_page_properties(page, props)
        return create_page(page, props, content)

    created = []
    errors = []

    # --- Create Person pages ---
    for entity in facts.get("entities", []):
        try:
            if ":" in entity:
                name, desc = entity.split(":", 1)
                name = name.strip()
                desc = desc.strip()
            else:
                name = entity.strip()
                desc = entity.strip()

            if not name:
                continue

            if re.match(r"^\d{4}-\d{2}-\d{2}", entity):  # skip date-prefixed
                continue

            page_info = None
            entity_lower = entity.lower()
            if any(kw in entity_lower for kw in ["project", "tool", "framework", "library"]):
                page_info = create_project_page(entity)
            elif any(c.isupper() for c in name[:3]):  # likely a person name
                page_info = create_person_page(name, desc, source="knowledge-extraction")

            if page_info and not args.dry_run:
                create_or_upsert(page_info["page"], page_info["props"], page_info.get("content", ""))
            if page_info:
                created.append(f"page: {page_info['page']}")
        except Exception as e:
            errors.append(f"entity error: {e}")

    # --- Create Event pages ---
    for event_str in facts.get("events", []):
        try:
            page_info = create_event_page(event_str)
            if page_info and not args.dry_run:
                create_or_upsert(page_info["page"], page_info["props"], page_info.get("content", ""))
            if page_info:
                created.append(f"event: {page_info['page']}")
        except Exception as e:
            errors.append(f"event error: {e}")

    # --- Update Sebastian - Preferences ---
    prefs = facts.get("preferences", [])
    if prefs and not args.dry_run:
        try:
            existing_prefs = []
            upsert_property("Sebastian - Preferences", "last_updated", datetime.now().date().isoformat())
            for pref in prefs:
                append_block("Sebastian - Preferences", f"- {pref}")
            created.append(f"preferences: {len(prefs)} items upserted")
        except Exception as e:
            errors.append(f"preferences error: {e}")

    # --- Update Sebastian - Insights ---
    insights = facts.get("insights", [])
    if insights and not args.dry_run:
        try:
            upsert_property("Sebastian - Insights", "last_updated", datetime.now().date().isoformat())
            for insight in insights:
                append_block("Sebastian - Insights", f"- {insight}")
            created.append(f"insights: {len(insights)} items upserted")
        except Exception as e:
            errors.append(f"insights error: {e}")

    # --- Log to today's journal ---
    if created and not args.dry_run:
        summary = f"Knowledge extraction: {len(created)} items integrated"
        try:
            log_to_today_journal(summary, tags=["knowledge-extraction", "pipeline"])
            created.append("journal: logged to today")
        except Exception as e:
            errors.append(f"journal error: {e}")

    sys.path.pop(0)

    log(f"Created: {', '.join(created) if created else 'none'}")
    if errors:
        log(f"Errors: {', '.join(errors)}")
    log("Integrator complete")


if __name__ == "__main__":
    run()
