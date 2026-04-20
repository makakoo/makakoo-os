# Knowledge Extraction — Daily Mining Pipeline

**Skill:** `harvey-os/skills/knowledge-extraction/`
**Owner:** Harvey
**Trigger Phrases:** "mine my emails", "extract knowledge", "run the knowledge pipeline", "daily mining"

---

## What It Does

A 3-stage pipeline that mines Gmail + Calendar via `gws`, extracts structured facts using Gemini LLM, and integrates them into the Logseq Brain.

```
miner.py → extractor.py → integrator.py
   ↓            ↓              ↓
 raw JSON   learned_facts   Logseq pages
   (gws)        (Gemini)      (Brain)
```

---

## Stage 1: Miner

Fetches recent emails and calendar events via `gws`, saves raw JSON to `data/knowledge-extraction/raw/`.

```bash
python3 miner.py [--days 7]   # default: last 7 days
```

Output:
- `raw/emails_{timestamp}.json` — up to 100 email objects
- `raw/calendar_{timestamp}.json` — upcoming calendar events
- `raw/latest.json` — pointer to latest files

---

## Stage 2: Extractor

Calls Gemini LLM on the raw data to extract structured facts into 4 categories:
- **entities** — People, companies, projects (format: `Name: Description`)
- **preferences** — Stated/inferred preferences (format: `Name: Preference`)
- **insights** — Strategic observations (format: `Insight: detail`)
- **events** — Scheduled events (format: `YYYY-MM-DD: Event title`)

```bash
GEMINI_API_KEY=... python3 extractor.py [--model gemini-2.0-flash]
```

Output: `data/knowledge-extraction/learned_facts.json`

Deduplicates against previously extracted facts.

---

## Stage 3: Integrator

Writes facts to Logseq Brain:
- **Person pages** — `[[Person - {Name}]]` for named entities
- **Event pages** — `[[Event - {Title} - {Date}]]` for scheduled events
- **Project pages** — `[[Project - {Name}]]` for project mentions
- **Sebastian - Preferences** — appends new preferences
- **Sebastian - Insights** — appends new insights
- **Today's journal** — logs a summary entry

```bash
python3 integrator.py [--dry-run]
```

---

## Running the Full Pipeline

```bash
# All 3 stages sequentially
python3 miner.py && GEMINI_API_KEY=... python3 extractor.py && python3 integrator.py

# Or via cron (recommended: daily at 06:00)
0 6 * * * cd ~/MAKAKOO/harvey-os/skills/knowledge-extraction && python3 miner.py >> ~/MAKAKOO/data/logs/knowledge-miner.log 2>&1 && GEMINI_API_KEY=... python3 extractor.py >> ~/MAKAKOO/data/logs/knowledge-extractor.log 2>&1 && python3 integrator.py >> ~/MAKAKOO/data/logs/knowledge-integrator.log 2>&1
```

---

## Data Locations

```
~/MAKAKOO/data/knowledge-extraction/
├── raw/                    # Raw gws output (emails + calendar JSON)
│   ├── emails_YYYYMMDD_HHMMSS.json
│   ├── calendar_YYYYMMDD_HHMMSS.json
│   └── latest.json          # Pointer to most recent files
├── learned_facts.json      # Deduplicated structured facts
├── history/                # Archived facts snapshots
├── auto_research_content_machine.md   # (manual additions)
└── karpathy_agent_future.md            # (manual additions)

~/MAKAKOO/data/logs/
├── knowledge-miner.log
├── knowledge-extractor.log
└── knowledge-integrator.log
```

---

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `GEMINI_API_KEY` | Yes (for stage 2) | — | Gemini API key |
| `GEMINI_MODEL` | No | `gemini-2.0-flash` | Gemini model to use |
| `GWS_PATH` | No | `gws` | Path to gws CLI |
| `HARVEY_HOME` | No | auto | Base path |

---

## Dependencies

- `gws` CLI tool — for Gmail + Calendar access
- `gemini` API key — for LLM extraction
- `logseq-brain` skill — for Brain integration (stage 3)
- Python 3.11+

---

## Relationship to Other Skills

- **logseq-brain** — integrator uses `logseq_bridge.py` to write pages
- **knowledge-extraction** is NOT the same as **inbox-triage**:
  - inbox-triage: real-time watchdog, urgent recruiter emails → immediate Brain sync
  - knowledge-extraction: daily batch pipeline, all entities/preferences/events
