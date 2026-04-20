#!/usr/bin/env python3
import os
import yaml
import json
import argparse
import sys
from datetime import datetime

# Setup paths
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))

# Import Logseq Brain (Sibling Skill)
try:
    sys.path.append(os.path.join(os.path.dirname(SCRIPT_DIR), "logseq-brain"))
    from logseq_bridge import LogseqBrain
except ImportError:
    LogseqBrain = None

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
BASE_DIR = os.path.join(HARVEY_HOME, "data", "career-manager")
CONFIG_FILE = os.path.join(BASE_DIR, "config.yaml")

with open(CONFIG_FILE, 'r') as f:
    config = yaml.safe_load(f)

COMM_DIR = os.path.join(BASE_DIR, "communications")
JSON_DB_FILE = os.path.join(BASE_DIR, "leads_data.json")

def find_lead_in_db(company_name):
    if not os.path.exists(JSON_DB_FILE):
        return None
    with open(JSON_DB_FILE, 'r') as f:
        leads = json.load(f)
    for lead in leads:
        if isinstance(lead, dict) and company_name.lower() in lead.get('company', '').lower():
            return lead
    return None

def draft_pitch(company_name, job_title=None, skills_matched=None, contact="Hiring Manager", template_name="default"):
    # Try to augment with DB data if running from raw inputs
    lead = find_lead_in_db(company_name)
    if lead:
        if not job_title:
            job_title = lead.get('title', 'Senior Engineer')
        if not skills_matched:
            skills_matched = ", ".join(lead.get('skills', []))
        # Use full company name from DB if available
        company_name = lead.get('company', company_name)
        
    # Fallbacks
    job_title = job_title or "Senior Engineer"
    skills_matched = skills_matched or "AI and Microservices"

    folder_name = company_name.lower().replace(" ", "_").replace(",", "").replace("-", "_")
    target_dir = os.path.join(COMM_DIR, folder_name)
    os.makedirs(target_dir, exist_ok=True)
    
    # Load template from config
    templates = config.get("pitch_templates", {})
    pitch_template = templates.get(template_name, templates.get("default", "Hi {company_name},\n\nI am applying for the {job_title} role.\n\nBest,\nSebastian"))

    # Format the message
    try:
        formatted_message = pitch_template.format(
            company_name=company_name,
            job_title=job_title,
            skills_matched=skills_matched
        )
    except KeyError as e:
        print(f"Warning: Template contains unknown variable {e}. Falling back to standard string replace.")
        formatted_message = pitch_template.replace("{company_name}", company_name).replace("{job_title}", job_title).replace("{skills_matched}", skills_matched)

    history_content = f"""# CRM History: {company_name}
**Contact:** {contact}
**Status:** `Drafting`

## {datetime.now().strftime('%Y-%m-%d')} - [OUTREACH DRAFT]
**Target Profile:** LinkedIn / Open Web

**Proposed Message:**
```text
{formatted_message}
```
"""
    history_path = os.path.join(target_dir, "HISTORY.md")
    with open(history_path, "w") as f:
        f.write(history_content)
        
    print(f"✅ Draft created for {company_name} in {history_path}")
    print(f"   Using template: '{template_name}'")
    
    # Sync company to Brain
    try:
        from sync_to_brain import sync_company_to_brain
        print(f"  🧠 Syncing CRM state to Brain: Company - {company_name}")
        sync_company_to_brain(company_name, history_content, status="Drafting", contact=contact)
    except ImportError as e:
        print(f"  ⚠️ Could not import sync_to_brain: {e}")
    
    # Log to Brain
    if LogseqBrain:
        brain = LogseqBrain()
        brain.log_daily_action(
            action_type="Pitch Drafted",
            entity=company_name,
            details=[f"Drafted a pitch for the **{job_title}** role.", f"Template used: `{template_name}`"],
            tags=["#CRM", "#outreach"]
        )

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate a draft pitch using dynamic config templates.")
    parser.add_argument("company", help="The name of the company to pitch")
    parser.add_argument("--title", help="Job title (optional, will try to fetch from JSON DB)", default=None)
    parser.add_argument("--skills", help="Comma separated skills (optional, will try to fetch from JSON DB)", default=None)
    parser.add_argument("--contact", help="Contact person or email", default="Hiring Manager")
    parser.add_argument("--template", help="Name of the template from config.yaml", default="default")
    
    args = parser.parse_args()
    draft_pitch(args.company, args.title, args.skills, args.contact, args.template)
