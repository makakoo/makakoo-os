#!/usr/bin/env python3
import sys
import json
import re
import os
import subprocess
import base64
import yaml
import shutil
from datetime import datetime
from html import unescape

# Ensure local modules can be imported
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.append(SCRIPT_DIR)
from verify_job import verify_job_link

# Import Logseq Brain (Sibling Skill)
try:
    sys.path.append(os.path.join(os.path.dirname(SCRIPT_DIR), "logseq-brain"))
    from logseq_bridge import LogseqBrain
except ImportError:
    LogseqBrain = None

# --- CONFIGURATION (Dynamic) ---
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
BASE_DIR = os.path.join(HARVEY_HOME, "data", "career-manager")
CONFIG_FILE = os.path.join(BASE_DIR, "config.yaml")
with open(CONFIG_FILE, 'r') as f:
    config = yaml.safe_load(f)

STATE_FILE = os.path.join(BASE_DIR, "career_state.json")
OUTPUT_FILE = os.path.join(BASE_DIR, "CAREER_LEADS.md")
JSON_DB_FILE = os.path.join(BASE_DIR, "leads_data.json")

# Resolve GWS Path
GWS_PATH = os.environ.get("GWS_PATH") or shutil.which("gws") or "gws"

CORE_SKILLS = config.get("skills", {})
FILTERS = config.get("filters", {})

def run_gws(subcommand, params):
    cmd = [GWS_PATH] + subcommand + ["--params", json.dumps(params), "--format", "json"]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"GWS Error: {result.stderr}", file=sys.stderr)
        return None
    return result.stdout

def get_last_id():
    if os.path.exists(STATE_FILE):
        with open(STATE_FILE, 'r') as f:
            try:
                return json.load(f).get("last_processed_id")
            except:
                return None
    return None

def save_state(msg_id):
    with open(STATE_FILE, 'w') as f:
        json.dump({"last_processed_id": msg_id, "updated_at": str(datetime.now())}, f, indent=2)

def clean_html(html):
    if not html: return ""
    text = re.sub(r'<style.*?>.*?</style>', '', html, flags=re.DOTALL)
    text = re.sub(r'<[^>]+>', ' ', text)
    return unescape(re.sub(r'\s+', ' ', text).strip())

def extract_links(html):
    return re.findall(r'href="([^"]+)"', html)

def detect_contract(text):
    text = text.lower()
    freelance_keywords = FILTERS.get("freelance_keywords", [])
    if any(k in text for k in freelance_keywords):
        return "🟢 Freelance / Contract"
    return "🔵 Permanent / Full-time"

def calculate_match_score(text):
    text = text.lower()
    score = 0
    matches = []
    for cat_name, cat_data in CORE_SKILLS.items():
        weight = cat_data.get("weight", 1)
        keywords = cat_data.get("keywords", [])
        for skill in keywords:
            if re.search(r'\b' + skill + r'\b', text):
                score += weight
                matches.append(skill.title())
    return score, sorted(list(set(matches)))

def main():
    print(f"🚀 Harvey Career Curator V2 | {datetime.now().strftime('%Y-%m-%d %H:%M')}")
    last_id = get_last_id()
    
    lookback_days = config.get("settings", {}).get("email_lookback_days", 7)
    raw_query = config.get("email_queries", {}).get("job_search", "")
    query = raw_query.format(days=lookback_days)
    
    # --- DYNAMIC CRM INCLUSION ---
    # Append all known CRM contacts to the query to ensure we NEVER miss a client reply
    import glob
    crm_emails = set()
    for history_file in glob.glob(os.path.join(BASE_DIR, "communications", "*", "HISTORY.md")):
        try:
            with open(history_file, 'r') as f:
                content = f.read()
                # Find **Contact:** email or raw email formats
                match = re.search(r'\*\*Contact:\*\*.*?([a-zA-Z0-9_.+-]+@[a-zA-Z0-9-]+\.[a-zA-Z0-9-.]+)', content)
                if match:
                    crm_emails.add(match.group(1))
        except Exception:
            pass
            
    if crm_emails:
        crm_query_part = " OR ".join([f"from:{email}" for email in crm_emails])
        query = f"({query}) OR ({crm_query_part})"
    # -----------------------------
    
    print(f"📡 Querying Gmail...")
    
    params = {"userId": "me", "q": query, "maxResults": 50}
    raw_list = run_gws(["gmail", "users", "messages", "list"], params)
    
    if not raw_list:
        print("❌ GWS Error: Check authentication or path.")
        return

    messages_data = json.loads(raw_list)
    messages = messages_data.get("messages", [])
    if not messages:
        print("✅ No matching emails found.")
        return

    new_messages = []
    for m in messages:
        if m['id'] == last_id: break
        new_messages.append(m)

    if not new_messages:
        print("✅ No new leads since last run.")
        return

    print(f"🔍 Analyzing {len(new_messages)} new emails...")
    leads = []
    newest_processed_id = new_messages[0]['id']

    for msg_meta in new_messages:
        msg_id = msg_meta['id']
        raw_msg = run_gws(["gmail", "users", "messages", "get"], {"userId": "me", "id": msg_id})
        if not raw_msg: continue
        
        msg = json.loads(raw_msg)
        snippet = msg.get('snippet', '')
        headers = {h['name']: h['value'] for h in msg['payload']['headers']}
        subject = headers.get('Subject', '')
        sender = headers.get('From', '')
        
        # Extract Body
        body_data = ""
        payload = msg['payload']
        if 'parts' in payload:
            for p in payload['parts']:
                if p['mimeType'] == 'text/html' and 'data' in p['body']:
                    body_data = p['body']['data']
                    break
        elif 'body' in payload and 'data' in payload['body']:
            body_data = payload['body']['data']
            
        html_body = ""
        if body_data:
            html_body = base64.urlsafe_b64decode(body_data + '=' * (4 - len(body_data) % 4)).decode('utf-8', errors='ignore')
        
        text_content = clean_html(html_body)
        full_text = (subject + " " + snippet + " " + text_content).lower()

        # 0. Detect Bounced Emails
        if "mailer-daemon" in sender.lower() or "delivery status notification" in subject.lower():
            inbound_file = os.path.join(BASE_DIR, "INBOUND_MESSAGES.md")
            with open(inbound_file, 'a') as f:
                f.write(f"\n## ⚠️ BOUNCED EMAIL: {datetime.now().strftime('%Y-%m-%d %H:%M')}\n")
                f.write(f"- **Subject:** {subject}\n")
                f.write(f"- **Snippet:** {snippet}...\n")
                f.write(f"- **Link:** https://mail.google.com/mail/u/0/#inbox/{msg_id}\n\n")
            print(f"  ⚠️ BOUNCE DETECTED: Logged delivery failure for a recently sent message.")
            continue # Skip further processing for bounces

        # 0.5. Sender-Domain Noise Blocklist (skip newsletters & automated senders early)
        noise_sender_domains = FILTERS.get("noise_sender_domains", [])
        sender_lower = sender.lower()
        is_noise_sender = any(nd in sender_lower for nd in noise_sender_domains)
        
        if is_noise_sender:
            # Still process as a potential job lead (e.g., XING job alerts), but NEVER flag as URGENT INBOUND
            pass
        else:
            # 1. Detect Direct Client Inquiries & Replies (ONLY from non-noise senders)
            is_direct_reply = "re:" in subject.lower() or "aw:" in subject.lower() or "antwort:" in subject.lower()
            recruiter_keywords = ["sind sie verfügbar", "verfügbarkeit", "ihre bewerbung", "interview", "telefonieren", "kennenzulernen", "austausch"]
            is_recruiter_direct = any(k in full_text for k in recruiter_keywords)
            
            auto_digests = FILTERS.get("automated_digests", [])
            is_automated_digest = any(k in full_text for k in auto_digests)
            
            if (is_direct_reply or is_recruiter_direct) and os.environ.get("PERSONAL_EMAIL_PREFIX", "your.email") not in sender_lower and not is_automated_digest:
                inbound_file = os.path.join(BASE_DIR, "INBOUND_MESSAGES.md")
                # Dedup: check if this msg_id was already logged
                existing_inbound = ""
                if os.path.exists(inbound_file):
                    with open(inbound_file, 'r') as f:
                        existing_inbound = f.read()
                if msg_id not in existing_inbound:
                    with open(inbound_file, 'a') as f:
                        f.write(f"\n## 🚨 URGENT INBOUND: {datetime.now().strftime('%Y-%m-%d %H:%M')}\n")
                        f.write(f"- **From:** {sender}\n")
                        f.write(f"- **Subject:** {subject}\n")
                        f.write(f"- **Snippet:** {snippet}...\n")
                        f.write(f"- **Link:** https://mail.google.com/mail/u/0/#inbox/{msg_id}\n\n")
                    print(f"  🚨 ALARM: Logged urgent direct client message from {sender}")
                else:
                    print(f"  ⏭️ Skipped duplicate inbound from {sender}")
                continue # Skip job lead processing for direct replies

        # 2. Quality Filters for Job Leads
        noise_keywords = FILTERS.get("noise_keywords", [])
        auto_digests = FILTERS.get("automated_digests", [])
        is_automated_digest = any(k in full_text for k in auto_digests)
        if any(noise in full_text for noise in noise_keywords) or is_automated_digest:
            continue
            
        score, matches = calculate_match_score(full_text)
        seniority_keywords = FILTERS.get("seniority", [])
        has_seniority = any(r in full_text for r in seniority_keywords)

        # STRIKE: High Match Criteria
        if score >= 3 and has_seniority:
            links = extract_links(html_body)
            apply_link = f"https://mail.google.com/mail/u/0/#inbox/{msg_id}"
            
            # Find the actual job link
            for l in links:
                if any(k in l for k in ["comm/jobs/view", "xing.com/jobs", "xing.com/m", "jobot.com/jobs", "workable.com", "greenhouse.io"]):
                    apply_link = l
                    break
            
            # Proof of Life Verification
            is_alive = True
            if "mail.google.com" not in apply_link:
                print(f"  -> Verifying link: {apply_link}")
                is_alive = verify_job_link(apply_link)
            
            if is_alive:
                leads.append({
                    "title": subject.replace("“", "").replace("”", ""),
                    "company": sender.split('<')[0].strip(),
                    "type": detect_contract(full_text),
                    "link": apply_link,
                    "skills": matches,
                    "score": score
                })
            else:
                print(f"  -> Discarded DEAD lead: {subject}")

    if not leads:
        print("✅ New emails analyzed. No high-quality matches found.")
        save_state(newest_processed_id)
        return

    # Update Lead File (Prepend new results)
    current_content = ""
    if os.path.exists(OUTPUT_FILE):
        with open(OUTPUT_FILE, 'r') as f:
            current_content = f.read()

    new_report = f"## 📅 Run: {datetime.now().strftime('%Y-%m-%d %H:%M')}\n\n"
    for l in sorted(leads, key=lambda x: x['score'], reverse=True):
        new_report += f"### {l['title']}\n"
        new_report += f"- **Company:** {l['company']}\n"
        new_report += f"- **Match Score:** {l['score']}/10\n"
        new_report += f"- **Type:** {l['type']}\n"
        new_report += f"- **Apply:** [Direct Link]({l['link']})\n"
        new_report += f"- **Matched Skills:** {', '.join(l['skills'])}\n\n"

    # If first run or no header, add header
    header = "# 💎 Harvey Curated Career Leads\n\n"
    if "# 💎" not in current_content:
        full_output = header + new_report + current_content
    else:
        full_output = current_content.replace(header, header + new_report)

    with open(OUTPUT_FILE, 'w') as f:
        f.write(full_output)
    
    # Save to JSON Database
    if leads:
        current_leads = []
        if os.path.exists(JSON_DB_FILE):
            try:
                with open(JSON_DB_FILE, 'r') as f:
                    raw = json.load(f)
                    # Normalize: process_leads.py saves as {"jobs": [...]}, curate saves as flat list
                    if isinstance(raw, dict):
                        current_leads = raw.get("jobs", [])
                    elif isinstance(raw, list):
                        current_leads = raw
            except Exception:
                pass
        
        # Prepend new leads
        # adding a date_added field
        now_str = datetime.now().strftime('%Y-%m-%d %H:%M')
        for l in leads:
            l['date_added'] = now_str
            
        updated_leads = leads + current_leads
        with open(JSON_DB_FILE, 'w') as f:
            json.dump(updated_leads, f, indent=2)

        # Build Brain integration: live sync new leads
        try:
            from sync_to_brain import sync_lead_to_brain
            for l in leads:
                print(f"  🧠 Syncing lead to Brain: {l['title']}")
                sync_lead_to_brain(l)
        except ImportError as e:
            print(f"  ⚠️ Could not import sync_to_brain: {e}")

    print(f"🎉 Success! Found {len(leads)} high-quality leads.")
    save_state(newest_processed_id)
    
    # Log to Brain
    if LogseqBrain and leads:
        brain = LogseqBrain()
        details = [f"Analyzed {len(new_messages)} emails."]
        details.append(f"Discovered **{len(leads)}** high-quality jobs.")
        for l in leads[:5]: # Top 5
            details.append(f"[{l['title']}]({l['link']}) at {l['company']} ({l['score']}/10)")
        
        brain.log_daily_action(
            action_type="Job Scan Complete",
            entity="Career Manager",
            details=details,
            tags=["#CRM", "#jobs"]
        )

if __name__ == "__main__":
    main()
