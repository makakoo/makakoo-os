#!/usr/bin/env python3
"""sync_to_brain.py - Reusable module for syncing career data to Logseq Brain."""

import os
import sys
from datetime import datetime

# Import Logseq Brain
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
try:
    sys.path.append(os.path.join(os.path.dirname(SCRIPT_DIR), "logseq-brain"))
    from logseq_bridge import LogseqBrain
except ImportError:
    LogseqBrain = None

brain = LogseqBrain() if LogseqBrain else None


def sync_lead_to_brain(lead):
    """Sync a single job lead dictionary to a Logseq Brain page."""
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
    
    properties = {
        "type": "career-lead",
        "company": company,
        "job-title": title,
        "score": str(score),
        "contract-type": contract_type,
        "status": "New",
        "link": link,
        "skills": skills,
        "date-added": date_added,
        "source": "career-manager"
    }

    content = f"""
## Details
- Application Link: {link}
- Matched Skills: {skills}
- Date Added: {date_added}
"""
    # Create or update the page
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
