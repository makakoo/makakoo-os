import os
import re
import sys
import yaml
from datetime import datetime

# Setup dynamic paths
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

# Flatten the keywords from config to use for scoring
critical_keywords = []
for cat_data in config.get("skills", {}).values():
    critical_keywords.extend(cat_data.get("keywords", []))

def calculate_priority(history_content, job_content):
    score = 0
    reasons = []

    # 1. Friction / Access Scoring (30 points max)
    if "Target Profile: https://www.linkedin.com/in/" in history_content or "Target Profile: http" not in history_content and "@" in history_content:
        score += 30
        reasons.append("Direct access to decision-maker (Low Friction)")
    elif "Workable" in history_content or "Greenhouse" in history_content:
        score += 10
        reasons.append("ATS Portal Application (High Friction)")

    # 2. Skill Overlap (40 points max)
    match_count = sum(1 for kw in critical_keywords if kw.lower() in job_content.lower())
    overlap_score = min(40, match_count * 5) # Adjusted weighting based on full config list
    score += overlap_score
    if overlap_score >= 20:
        reasons.append(f"Exceptional Tech Match ({match_count} core keywords found)")

    # 3. Recency (30 points max)
    date_match = re.search(r'Date Initiated:\s*(\d{4}-\d{2}-\d{2})', history_content)
    if date_match:
        init_date = datetime.strptime(date_match.group(1), '%Y-%m-%d')
        days_old = (datetime.now() - init_date).days
        if days_old <= 2:
            score += 30
            reasons.append("Highly Recent (<48 hours old)")
        elif days_old <= 7:
            score += 20
        else:
            score += 5
            reasons.append("Aging Lead (>7 days old)")
    else:
        # Fallback if no specific init date tag is found, look for general dates
        date_match = re.search(r'## (\d{4}-\d{2}-\d{2}) - \[OUTREACH DRAFT\]', history_content)
        if date_match:
            init_date = datetime.strptime(date_match.group(1), '%Y-%m-%d')
            days_old = (datetime.now() - init_date).days
            if days_old <= 2:
                score += 30
                reasons.append("Highly Recent Draft (<48 hours old)")
            elif days_old <= 7:
                score += 20
            else:
                score += 5
                reasons.append("Aging Draft (>7 days old)")

    return score, reasons

def advise_execution():
    pipeline = []
    
    if not os.path.exists(COMM_DIR):
        print("No communications directory found.")
        return

    for folder in os.listdir(COMM_DIR):
        folder_path = os.path.join(COMM_DIR, folder)
        if not os.path.isdir(folder_path) or folder == "_template":
            continue

        history_path = os.path.join(folder_path, "HISTORY.md")
        job_path = os.path.join(folder_path, "JOB_INSERT.md")

        if os.path.exists(history_path):
            with open(history_path, 'r') as f:
                history = f.read()
            
            if "**Status:** `Drafting`" in history:
                job_content = ""
                if os.path.exists(job_path):
                    with open(job_path, 'r') as jf:
                        job_content = jf.read()
                
                score, reasons = calculate_priority(history, job_content)
                pipeline.append({
                    "name": folder.replace("_", " ").title(),
                    "folder": folder_path,
                    "score": score,
                    "reasons": reasons,
                    "history": history
                })

    if not pipeline:
        print("✅ Pipeline Clear! No pending drafts require your attention.")
        return

    # Sort by score descending
    pipeline.sort(key=lambda x: x['score'], reverse=True)
    top_pick = pipeline[0]

    print("\n" + "="*50)
    print("🏆 HARVEY'S TOP EXECUTION RECOMMENDATION V2")
    print("="*50)
    print(f"\nTarget: {top_pick['name']}")
    print(f"Probability Score: {top_pick['score']}/100")
    print("Why this one?")
    for r in top_pick['reasons']:
        print(f" - {r}")
    
    print("\n" + "-"*50)
    print("📋 EXACT EXECUTION STEPS:")
    print("-"*50)
    
    # Extract Target Profile
    target_match = re.search(r'\*\*Target Profile:\*\*\s*(.+)', top_pick['history'])
    target = target_match.group(1).strip() if target_match else "See HISTORY.md"
    
    # Identify CV to use
    cv_match = re.search(r'\*\s*\*\*CV Sent:\*\*\s*`([^`]+)`', top_pick['history'])
    cv = cv_match.group(1) if cv_match else "Your standard Agentic CV"

    print(f"1. OPEN TARGET: {target}")
    print(f"2. ATTACH CV:   career/communications/{os.path.basename(top_pick['folder'])}/{cv}")
    print("3. COPY PITCH:")
    
    pitch_match = re.search(r'Proposed Message:\s*```(?:text)?\n(.*?)\n```', top_pick['history'], re.DOTALL)
    if pitch_match:
        print(f"\n{pitch_match.group(1)}\n")
    else:
        print("\n(Check HISTORY.md for the pitch text)\n")
        
    print("4. STATUS UPDATE: Tell me 'Harvey, I sent the pitch to [Company]' when done.")

    # Log to Brain
    if LogseqBrain:
        brain = LogseqBrain()
        brain.log_daily_action(
            action_type="Advisor Recommendation",
            entity=top_pick['name'],
            details=[f"Recommended execution. Score: {top_pick['score']}/100.", "Awaiting user to send pitch."],
            tags=["#CRM", "#advisory"]
        )

if __name__ == "__main__":
    advise_execution()
