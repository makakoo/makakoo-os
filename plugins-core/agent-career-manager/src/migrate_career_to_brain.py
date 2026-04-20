#!/usr/bin/env python3
"""migrate_career_to_brain.py - One-time migration of career leads and CRM history to Brain."""

import os
import sys
import json
import glob
import re
from datetime import datetime

# Ensure local modules can be imported
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.append(SCRIPT_DIR)

try:
    from sync_to_brain import sync_lead_to_brain, sync_company_to_brain, sync_inbound_to_brain, brain
except ImportError as e:
    print(f"Error importing sync_to_brain: {e}")
    sys.exit(1)

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
BASE_DIR = os.path.join(HARVEY_HOME, "data", "career-manager")
JSON_DB_FILE = os.path.join(BASE_DIR, "leads_data.json")

def migrate_leads():
    print("🚀 Starting Migration: Leads -> Brain")
    if not brain:
        print("❌ LogseqBrain API/Bridge is unavailable.")
        return 0, 0

    if not os.path.exists(JSON_DB_FILE):
        print(f"❌ JSON DB file not found: {JSON_DB_FILE}")
        return 0, 0

    with open(JSON_DB_FILE, 'r') as f:
        try:
            leads = json.load(f)
        except json.JSONDecodeError:
            print("❌ JSON decoding failed.")
            return 0, 0

    migrated_count = 0
    skipped_count = 0

    for lead in leads:
        company = lead.get("company", "Unknown")
        title = lead.get("title", "Unknown Role")
        page_title = f"Lead - {company} - {title}"
        
        if brain.page_exists(page_title):
            skipped_count += 1
            print(f"  ⏭️ Skipped (already exists): {page_title}")
        else:
            sync_lead_to_brain(lead)
            migrated_count += 1
            print(f"  ✅ Migrated: {page_title}")

    return migrated_count, skipped_count

def migrate_companies():
    print("\n🚀 Starting Migration: Companies (CRM) -> Brain")
    if not brain:
        return 0, 0
        
    comm_dir = os.path.join(BASE_DIR, "communications")
    history_files = glob.glob(os.path.join(comm_dir, "*", "HISTORY.md"))
    
    migrated_count = 0
    skipped_count = 0
    
    for hfile in history_files:
        if "_template" in hfile:
            continue
            
        company_folder = os.path.basename(os.path.dirname(hfile))
        
        # Parse history file
        with open(hfile, 'r') as f:
            content = f.read()
            
        # Extract Contact and Status
        contact_match = re.search(r'\*\*Contact:\*\*\s*(.+)', content)
        status_match = re.search(r'\*\*Status:\*\*\s*(.+)', content)
        name_match = re.search(r'# CRM History:\s*(.+)', content)
        
        contact = contact_match.group(1).replace('`', '').strip() if contact_match else "Unknown"
        status = status_match.group(1).replace('`', '').strip() if status_match else "Unknown"
        company_name = name_match.group(1).replace('`', '').strip() if name_match else company_folder
        
        # Extract last interaction date
        date_matches = re.findall(r'## (\d{4}-\d{2}-\d{2})', content)
        last_interaction = date_matches[-1] if date_matches else None
        
        page_title = f"Company - {company_name}"
        if brain.page_exists(page_title):
            skipped_count += 1
            print(f"  ⏭️ Skipped (already exists): {page_title}")
        else:
            sync_company_to_brain(company_name, content, status=status, contact=contact, last_interaction=last_interaction)
            migrated_count += 1
            print(f"  ✅ Migrated: {page_title}")
            
    return migrated_count, skipped_count

def migrate_inbound():
    print("\n🚀 Starting Migration: Inbound Messages -> Brain")
    if not brain:
        return 0, 0
        
    inbound_file = os.path.join(BASE_DIR, "INBOUND_MESSAGES.md")
    if not os.path.exists(inbound_file):
        print(f"  ⏭️ skipped, no INBOUND_MESSAGES.md file.")
        return 0, 0
        
    with open(inbound_file, 'r') as f:
        content = f.read()
        
    # Split by ## 🚨 URGENT INBOUND
    sections = re.split(r'## 🚨 URGENT INBOUND:\s*', content)
    
    migrated_count = 0
    skipped_count = 0
    
    for section in sections[1:]: # Skip the first part before the first match
        lines = section.split('\n')
        date_str = lines[0].strip() # usually date time
        
        sender = ""
        subject = ""
        snippet = ""
        msg_id = ""
        
        for line in lines:
            if line.startswith("- **From:**"):
                sender = line.split("From:**")[1].strip()
            elif line.startswith("- **Subject:**"):
                subject = line.split("Subject:**")[1].strip()
            elif line.startswith("- **Snippet:**"):
                snippet = line.split("Snippet:**")[1].strip()
            elif line.startswith("- **Link:**"):
                link = line.split("Link:**")[1].strip()
                if "inbox/" in link:
                    msg_id = link.split("inbox/")[1].strip()
                    
        page_title = f"Inbox - {sender} - {date_str[:10]}"
        if brain.page_exists(page_title):
            skipped_count += 1
            print(f"  ⏭️ Skipped (already exists): {page_title}")
        else:
            sync_inbound_to_brain(sender, subject, snippet, msg_id, urgency="High")
            migrated_count += 1
            print(f"  ✅ Migrated: {page_title}")
            
    return migrated_count, skipped_count


if __name__ == "__main__":
    print(f"Current Time: {datetime.now().strftime('%Y-%m-%d %H:%M')}")
    m_leads, s_leads = migrate_leads()
    m_comps, s_comps = migrate_companies()
    m_in, s_in = migrate_inbound()
    
    print("\n" + "="*40)
    print("📊 MIGRATION SUMMARY")
    print("="*40)
    print(f"Leads:          {m_leads} migrated, {s_leads} skipped")
    print(f"Companies:      {m_comps} migrated, {s_comps} skipped")
    print(f"Inbound Msgs:   {m_in} migrated, {s_in} skipped")
    print("="*40)
