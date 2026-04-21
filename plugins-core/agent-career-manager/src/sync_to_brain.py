#!/usr/bin/env python3
"""sync_to_brain.py - Reusable module for syncing career data to Logseq Brain.

v0.2 C.2: imports the Brain helpers directly from `core.memory.brain_bridge`
(PYTHONPATH is set up by the kernel; the old `logseq-brain/` skill path is
retired). Idempotent — re-runs use `upsert_page_properties` so a manual
status progression isn't clobbered by the next discovery tick.
"""

import os
import sys
from datetime import datetime

try:
    from core.memory.brain_bridge import (
        LogseqBrain,
        create_page,
        upsert_page_properties,
        append_block,
    )
except ImportError as e:
    print(f"[sync_to_brain] core.memory.brain_bridge unavailable: {e}", file=sys.stderr)
    LogseqBrain = None
    create_page = None
    upsert_page_properties = None
    append_block = None

brain = LogseqBrain() if LogseqBrain else None


def sync_lead_to_brain(lead):
    """Sync a single job lead dictionary to a Logseq Brain page.

    Idempotent (v0.2 C.2): on re-discovery, the lead's `score`, `contract-type`,
    `link`, `skills`, and `date-added` get refreshed, but user-progressed
    `status` stays — we only seed `status: "New"` the first time the page is
    created. Previously, every cron tick would reset the status and torch
    manual progress tracking.
    """
    if not brain:
        return None

    company = lead.get("company", "Unknown")
    title = lead.get("title", "Unknown Role")
    score = lead.get("score", 0)
    contract_type = lead.get("type", "").replace("🟢", "").replace("🔵", "").strip()
    link = lead.get("link", "")
    skills = ", ".join(lead.get("skills", []))
    date_added = lead.get("date_added", datetime.now().strftime('%Y-%m-%d %H:%M'))

    page_title = f"Lead - {company} - {title}"

    # Base props that always get updated.
    properties = {
        "type": "career-lead",
        "company": company,
        "job-title": title,
        "score": str(score),
        "contract-type": contract_type,
        "link": link,
        "skills": skills,
        "date-added": date_added,
        "source": "career-manager",
    }

    if brain.page_exists(page_title):
        # Existing page — refresh fields but DON'T overwrite status.
        return brain.upsert_page_properties(page_title, properties)

    # Fresh lead — seed with status: "New" + the full detail block.
    properties["status"] = "New"
    content = (
        "## Details\n"
        f"- Application Link: {link}\n"
        f"- Matched Skills: {skills}\n"
        f"- Date Added: {date_added}\n"
    )
    return brain.create_page(page_title, properties, content)


def sync_company_to_brain(company_name, history_content, status="Drafting", contact="Hiring Manager", last_interaction=None):
    """Create or update a Company CRM page in the Brain."""
    if not brain:
        return None

    page_title = f"Company - {company_name}"
    
    properties = {
        "type": "company",
        "status": status,
        "contact": contact,
        "source": "career-manager"
    }
    
    if last_interaction:
        properties["last-interaction"] = last_interaction

    # Format the history file content into Logseq bullet format using the helper
    bullet_content = brain._ensure_bullet_format(history_content) if hasattr(brain, '_ensure_bullet_format') else history_content

    # To avoid overwriting old CRM notes, we use the write_page_file which naturally merges/appends.
    # But since create_page has upsert behavior with API, let's use upsert_page_properties for the properties, 
    # and we can append block content using insertBlock via API if it exists, but for simplicity, write_page_file handles offline well.
    # Let's use upsert_page_properties, but since we want to ADD content, we might fall back to write_page_file.
    
    # Check if page exists to avoid duplicating the entire history_content on every run, 
    # or just use create_page if new, upsert properties if exists
    exists = brain.page_exists(page_title)
    
    if not exists:
        return brain.create_page(page_title, properties, bullet_content)
    else:
        brain.upsert_page_properties(page_title, properties)
        # If we have specific new content, we might append it. For now, since history_content is the full file,
        # we only sync the full file once or just update properties. We shouldn't continuously append the full file.
        return True


def sync_inbound_to_brain(sender, subject, snippet, msg_id, urgency="High"):
    """Create an Inbox Event page in the Brain."""
    if not brain:
        return None
        
    now = datetime.now()
    date_str = now.strftime('%Y-%m-%d')
    time_str = now.strftime('%H:%M')
    
    page_title = f"Inbox - {sender} - {date_str}"
    
    properties = {
        "type": "inbox-event",
        "sender": sender,
        "subject": subject,
        "urgency": urgency,
        "date": date_str,
        "time": time_str,
        "source": "career-manager"
    }
    
    content = f"""
- Subject: {subject}
- Snippet: {snippet}...
- Link: https://mail.google.com/mail/u/0/#inbox/{msg_id}
"""
    return brain.create_page(page_title, properties, content)

if __name__ == "__main__":
    print("This is a module for syncing career data to the Logseq Brain.")
