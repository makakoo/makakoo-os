# Memory Retrieval - Pre-Session Memory Loading

## Overview

The Memory Retrieval system automatically loads relevant context before each Harvey session using a 6-layer priority retrieval strategy.

## Components

### 1. MemoryLoader (`memory_loader.py`)
Core pre-session loader with 6-layer priority retrieval:

| Layer | Source | Max Tokens | Purpose |
|-------|--------|------------|---------|
| 1. Identity | Profile, Preferences, Insights pages | 2k | Core grounding |
| 2. Today | Today's journal | 4k | Session continuity |
| 3. Active Projects | Active project pages | 4k | Current work context |
| 4. Recent Leads | Top 5 CRM leads | 4k | Career pipeline |
| 5. Pending Actions | Tagged action/pending/waiting | 2k | Unblocked tasks |
| 6. Recent Context | Last 3 journals | 6k | Temporal context |

**Usage:**
```python
from memory_retrieval import MemoryLoader

loader = MemoryLoader()
context = loader.load_session_context(available_tokens=80000)
print(context)
```

### 2. MemoryScorer (`memory_scorer.py`)
Scores and ranks memories based on:
- topic_match (40%): keyword overlap
- recency_decay (30%): exponential decay over 90 days
- usage_frequency (15%): access_count normalization
- tag_overlap (15%): explicit tag matches

**Usage:**
```python
from memory_retrieval import filter_and_rank

ranked = filter_and_rank(memories, session_query="career pipeline", top_k=20)
```

### 3. MemorySummarizer (`memory_summarizer.py`)
LLM-powered compression via localhost:18080:

| Original Size | Level | Target |
|---------------|-------|--------|
| < 2k chars | None | Original |
| 2-8k chars | Light | 150 words |
| 8-32k chars | Medium | 400 words |
| > 32k chars | Heavy | 600 words |

**Usage:**
```python
from memory_retrieval import MemorySummarizer

summarizer = MemorySummarizer()
compressed = summarizer.summarize(long_text, max_words=200)
```

### 4. SessionStateManager (`session_state.py`)
Persists session state between sessions:

```python
from memory_retrieval import SessionStateManager

mgr = SessionStateManager()
session = mgr.get_or_create_session()  # Resume or create
mgr.update_state(session["session_id"], {"context_summary": "Working on X"})
mgr.end_session(session["session_id"])
```

### 5. FreshnessValidator (`freshness_validator.py`)
Detects stale memories:

| Level | Threshold |
|-------|-----------|
| FRESH | < 7 days |
| OK | 7-30 days |
| STALE | 30-90 days |
| ARCHIVE | > 90 days |

**Usage:**
```python
from memory_retrieval import FreshnessValidator

validator = FreshnessValidator()
level = validator.check_freshness(memory)
stale_leads = validator.flag_stale_leads(threshold_days=30)
```

### 6. ProactiveContextInjector (`proactive_injector.py`)
Anticipatory context injection based on:
- Morning (8am weekdays): career pipeline status
- Monday 9am: week ahead summary
- Harvey OS: recent commits, active tasks
- Career: pending outreach, interview status

**Usage:**
```python
from memory_retrieval import ProactiveContextInjector

injector = ProactiveContextInjector()
injections = injector.get_proactive_injections()
```

## Data Paths

- Brain: `$HARVEY_HOME/data/Brain/`
- Sessions: `$HARVEY_HOME/data/Brain/sessions/`
- Journals: `$HARVEY_HOME/data/Brain/journals/`
- Pages: `$HARVEY_HOME/data/Brain/pages/`

## API Fallback

When the optional Logseq API (http://127.0.0.1:12315) is unavailable, the system falls back to reading files directly from the Brain directory.

## Token Estimation

1 token ≈ 4 characters

## Integration

This skill is invoked automatically at session start via the Boot Procedure. It loads relevant context before Harvey's first response.
