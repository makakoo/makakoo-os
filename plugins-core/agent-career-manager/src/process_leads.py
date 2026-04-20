import json
import os
from datetime import datetime

# Files
HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
LEADS_JSON = os.path.join(HARVEY_HOME, "data", "career-manager", "leads_data.json")
BROAD_JSON = os.path.join(HARVEY_HOME, "data", "career-manager", "broad_scraped_results.json")
LEADS_MD = os.path.join(HARVEY_HOME, "data", "career-manager", "CAREER_LEADS.md")
APPLY_MD = os.path.join(HARVEY_HOME, "data", "career-manager", "APPLY_NOW.md")
CV_LINK = "https://jevvellabsblog.com/cv.html"
SENDER_NAME = os.environ.get("SENDER_NAME", "Your Name")

def score_job(title):
    keywords = {
        "ruby": 3, "rails": 3, 
        "ai": 2, "agent": 3, "llm": 2, "rag": 3, "generative": 2,
        "architect": 3, "devops": 2, "kubernetes": 3, "k8s": 3,
        "python": 2, "fastapi": 2, "langchain": 3,
        "gcp": 2, "aws": 2, "cloud": 1,
        "founding": 2, "lead": 2, "senior": 1,
        "rust": 3, "mcp": 4
    }
    score = 0
    t = title.lower()
    for kw, val in keywords.items():
        if kw in t:
            score += val
    return min(12, score) # Cap at 12

def get_pitch(job):
    title = job.get('title', 'Senior Engineer')
    return f"""Hi Team,

I saw your opening for {title} and wanted to reach out. I am a Senior Software Architect and Agentic AI Specialist with over 10 years of experience in high-availability systems, Kubernetes, and modern AI orchestration.

Recently, I've been focusing on building autonomous agent frameworks and optimizing RAG systems for production environments. Given my background in Ruby on Rails, Python, and cloud infrastructure, I believe I can bring immediate value to your roadmap.

You can find my full CV and project portfolio here: {CV_LINK}

Are you open to a brief chat to see if my expertise aligns with your current needs?

Best,
{SENDER_NAME}"""

def main():
    if not os.path.exists(BROAD_JSON):
        print("No broad results found.")
        return
        
    with open(BROAD_JSON, 'r') as f:
        new_jobs = json.load(f)
        
    if not os.path.exists(LEADS_JSON):
        current_data = {"jobs": []}
    else:
        with open(LEADS_JSON, 'r') as f:
            current_data = json.load(f)
            if isinstance(current_data, list):
                current_data = {"jobs": current_data}
                
    added_count = 0
    for nj in new_jobs:
        # Check if already exists (by normalized URL)
        clean_url = nj['url'].split('?')[0].rstrip('/')
        if any(clean_url == (j.get('url') or '').split('?')[0].rstrip('/') for j in current_data['jobs']):
            continue
            
        score = score_job(nj['title'])
        if score < 4: continue # Filter noise
        
        job_entry = {
            "title": nj['title'],
            "company": nj.get('company', 'Unknown'),
            "url": nj['url'],
            "source": nj['source'],
            "match_score": score,
            "status": "Pending",
            "date_found": datetime.now().strftime('%Y-%m-%d')
        }
        current_data['jobs'].append(job_entry)
        added_count += 1
        
    with open(LEADS_JSON, 'w') as f:
        json.dump(current_data, f, indent=2)
        
    print(f"Added {added_count} new high-quality leads to database.")
    
    # Generate APPLY_NOW.md with TOP 50
    best_leads = [j for j in current_data['jobs'] if j.get('status') == 'Pending']
    best_leads.sort(key=lambda x: x.get('match_score', 0), reverse=True)
    
    top_50 = best_leads[:50]
    
    with open(APPLY_MD, 'w') as f:
        f.write("# 🚀 Harvey's Top 50 Career Execution List\n")
        f.write(f"Generated: {datetime.now().strftime('%Y-%m-%d')} | Pool size: {len(best_leads)} pending jobs\n\n")
        f.write("I have expanded the search keywords and platforms. Here are the top 50 matches for your profile.\n\n")
        f.write("---\n\n")
        
        for i, job in enumerate(top_50, 1):
            f.write(f"## {i}. {job['title']}\n")
            f.write(f"- **Company:** {job.get('company', 'Unknown')}\n")
            f.write(f"- **Match Score:** {job.get('match_score', 0)}/12\n")
            f.write(f"- **Source:** {job.get('source', 'Manual')}\n")
            f.write(f"- **🔗 [APPLY HERE]({job['url']})**\n\n")
            
            f.write("### 📝 Application Pitch\n")
            f.write("```text\n")
            f.write(get_pitch(job))
            f.write("\n```\n\n")
            f.write("---\n\n")
            
    print(f"Successfully generated execution file: {APPLY_MD}")

if __name__ == "__main__":
    main()
