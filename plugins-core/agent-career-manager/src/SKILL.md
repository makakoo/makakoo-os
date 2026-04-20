---
name: career-manager
description: Consolidated autonomous career agent — CRM pipeline, LinkedIn outreach, inbox watchdog, and email communications.
---

# Career Manager Agent

Consolidated autonomous career agent — CRM pipeline, LinkedIn outreach, inbox watchdog, and email communications.

## Components

### Core CRM (`*.py` in root)
Lead generation, curation, pitch drafting, job verification, and Brain sync.

| Script | Purpose |
|--------|---------|
| `curate_career.py` | Parse Gmail job alerts via `gws`, score and filter leads |
| `active_search.py` | Proactive job search across platforms |
| `process_leads.py` | Process and deduplicate incoming leads |
| `digest_parser.py` | Parse digest-format email alerts into structured leads |
| `verify_job.py` | Verify job postings are still live before adding to pipeline |
| `advisor.py` | Recommend highest-probability outreach targets |
| `draft_pitch.py` | Generate personalized pitches from templates + lead data |
| `linkedin_scraper.py` | Stealth headless browser for LinkedIn job/profile extraction |
| `sync_to_brain.py` | Sync CRM state to Logseq Brain |
| `migrate_career_to_brain.py` | One-time migration of career data into Brain pages |

### LinkedIn Outreach (`send_pitches.py`, `session_saver.py`)
Headless browser automation for LinkedIn connection requests. Reads validated leads, drafts personalized 300-char connection notes, and executes via Playwright.

- `send_pitches.py` — Main outreach executor
- `session_saver.py` — LinkedIn session cookie persistence

### Inbox Watchdog (`inbox_watchdog.py`)
Monitors Gmail for client replies, bounce detection, and new inbound leads. Cross-references sender against CRM communications folders.

### Email Communications (`communications/`)
Per-lead email scripts organized by company. Each subfolder may contain `send_email.py`, `send_reply.py`, or specialized scripts for that lead's communication flow.

## Targeting

- **Profile:** Senior Software Architect, Agentic AI Engineer, Foundation Model/MCP Specialist
- **Stack:** Ruby (Rails), Python (FastAPI/LangChain/Agno), Kubernetes, Event-Driven Systems
- **Priority:** Freelance/B2B contracts first, strong permanent roles always evaluated
- **Filters:** No junior roles, no roles older than 14 days, remote/hybrid EU or global preferred

## Workflow Phases

1. **Lead Generation** — `curate_career.py` ingests Gmail alerts, scores against CV, verifies links are live
2. **Deep Analysis** — `linkedin_scraper.py` extracts full job descriptions from LinkedIn
3. **Pitch Engineering** — `draft_pitch.py` generates outreach from templates + lead context
4. **Outreach Execution** — `advisor.py` recommends targets; `send_pitches.py` handles LinkedIn; email scripts handle cold email (always requires user approval before send)
5. **Inbox Monitoring** — `inbox_watchdog.py` scans for replies, bounces, and new inbound
6. **Client Onboarding** — Directory setup for active engagements with billing/contract tracking

## Data Locations

- **Leads DB:** `$HARVEY_HOME/data/career-manager/leads_data.json`
- **Leads Markdown:** `$HARVEY_HOME/data/career-manager/CAREER_LEADS.md`
- **CRM Audit Trail:** `$HARVEY_HOME/data/career-manager/communications/[lead]/HISTORY.md`
- **Client Tracking:** `$HARVEY_HOME/data/career-manager/clients/[lead]/PROJECT_TRACKING.md`
- **Brain:** `$HARVEY_HOME/data/Brain/pages/` (synced entities)

## Requirements

```
pip install -r requirements.txt
playwright install chromium
```

## Safety Rules

- **Never auto-send emails.** Always show draft and wait for explicit user approval.
- **Verify before adding.** Every lead must have a live job posting confirmed.
- **Log everything.** All outreach gets timestamped in the lead's `HISTORY.md`.
