# Harvey Auto-Memory System

## Philosophy

Humans don't consciously "save to memory." Sleep → brain consolidates. Repeated events → pattern recognition. Important moments → emotional weight triggers persistence.

Harvey's memory should work the same way:
- **Continuous capture** (everything flows through the system)
- **Automatic significance detection** (via embeddings, context, frequency)
- **Intelligent consolidation** (SANCHO dreams do the heavy lifting)
- **Unified knowledge graph** (Brain + Superbrain + vector embeddings)

## Architecture: Memory Acquisition Pipeline

```
User Action (git commit, email, calendar, bash, etc.)
    ↓
EventBus/HookManager capture (raw facts)
    ↓
Auto-Memory Router
    ├─ Route to Brain journal (raw log)
    ├─ Embed in Superbrain (semantic index)
    └─ Signal SANCHO consolidation (dream task)
    ↓
SANCHO Dreams (periodic consolidation)
    ├─ Extract patterns from journal
    ├─ Synthesize learnings
    ├─ Update memory files
    └─ Rebuild knowledge graph
    ↓
Superbrain (semantic search + retrieval)
    └─ Query understands intent, not just keywords
```

## Core Components (All Existing)

### 1. **Continuous Capture** (EventBus + HookManager)
Every significant action emits an event:
- `git.commit.created` → commit facts
- `task.completed` → task results
- `agent.spawned` → agent startup
- `gmail.message.received` → email context
- `skill.invoked` → skill execution
- `brain.journal.written` → knowledge created

**Router function:** Auto-Memory enriches each event with:
- Semantic embedding (via Gemini/Claude)
- Significance score (time cost? decision made? new learning?)
- Entity extraction (people, projects, decisions)
- Bidirectional links (wikilinks to related entities)

### 2. **Brain Journal** (source of truth)
Every captured fact appended to `data/Brain/journals/YYYY_MM_DD.md`:
```markdown
- [git.commit] Refactored auth middleware to use JWT
  - **Triggered:** compliance audit flagged session tokens
  - **Decision:** httpOnly cookies + 15min rotation
  - **Impact:** blocks login flow, needs frontend update
  - **Links:** [[CSO]] [[Traylinx]] [[auth-middleware]]

- [task.completed] HTE implementation finished
  - **What:** 4-phase terminal engine + 4 system wizards
  - **Why:** cross-CLI compatible UI reduces friction
  - **Key decision:** use existing box-draw patterns from sprites.py
  - **Next:** system hooks integration
  - **Links:** [[Harvey Terminal Engine]] [[system-wizards]]
```

Raw data, no filtering. Journal captures everything.

### 3. **Superbrain Vector Index** (Qdrant embeddings)
Every fact gets embedded and indexed:
- **Embedding strategy**: Use Gemini (3072d) not pgvector (max 2000d)
- **Metadata**: timestamp, source (git/email/task/etc), entities, significance
- **Query**: "What did I decide about auth?" → semantic search understands intent
- **Retrieval**: Top-K similar facts from entire history

Config in `data/Brain/settings.json`:
```json
{
  "embedding": {
    "provider": "gemini",
    "model": "embedding-002",
    "dimensions": 3072,
    "batch_size": 50,
    "sync_interval": 3600
  }
}
```

### 4. **SANCHO Consolidation** (Dream Task)
Existing `SANCHO` engine runs periodic consolidation:

**Dream task schedule:** Every 4 hours (or after 5+ journal entries)

**Process:**
```python
# core/sancho/tasks.py - existing task
class MemoryConsolidationTask(ProactiveTask):
    """Automatic memory consolidation (existing SANCHO pattern)"""
    
    async def execute(self):
        # 1. Read today's journal
        journal_entries = logseq.read_journal(today)
        
        # 2. Extract semantic clusters
        clusters = semantic_cluster(journal_entries)
        
        # 3. For each cluster:
        #    - Generate summary
        #    - Identify decision/learning/blocker
        #    - Update relevant memory file
        #    - Create/update wiki page
        
        # 4. Rebuild knowledge graph
        wiki_compile()
        superbrain.rebuild_index()
        
        # 5. Log consolidation event
        emit("sancho.memory_consolidated", 
             facts_processed=len(journal_entries),
             clusters=len(clusters))
```

This already exists in SANCHO. We just need to wire it to journal facts.

### 5. **Memory Files** (Auto-updated)
SANCHO dream consolidation updates memory files:

**Feedback:** `data/Brain/memory/feedback_*.md`
```markdown
---
name: JWT Auth Decision
type: feedback
---

**Rule:** Always use httpOnly cookies for session tokens, never localStorage.

**Why:** Compliance audit flagged localStorage as security risk. httpOnly prevents XSS token theft.

**How to apply:** Any auth flow needing session persistence.

**Source:** [[git commit 3a7f2b1]] (2026-04-10)
```

**Project:** `data/Brain/memory/project_*.md`
```markdown
---
name: HTE Implementation
type: project
---

**Status:** Complete (2026-04-10)

**What:** 4-phase terminal engine for cross-CLI compatibility

**Why:** Existing UI code was fragmented. HTE consolidates into reusable widgets.

**Delivered:** 
- core/terminal/ (7 modules, 2000 LOC)
- skills/system/ (4 wizards, 1500 LOC)
- Onboarding (300 LOC)

**Key decisions:**
- ASCII fallback for all Unicode
- Graceful animation disable when piped
- 6 input types in Wizard framework

**Source:** [[task completion 4a2f1c8]] (2026-04-10)
```

**Reference:** `data/Brain/memory/reference_*.md`
```markdown
---
name: Brain API (optional Logseq)
type: reference
---

**Location:** `http://127.0.0.1:12315`

**Key endpoints:**
- GET `/api/graph` - list all pages
- POST `/api/journal` - append to journal
- POST `/api/page` - create/update page

**Used by:** logseq_bridge.py, auto_memory_router.py

**Note:** Optional — requires Logseq running locally
```

## Implementation: Four Layers

### Layer 1: Capture Router (`core/memory/auto_memory_router.py`)
```python
class AutoMemoryRouter:
    """Route all captured facts to memory subsystems"""
    
    async def on_event(self, event):
        """Called by EventBus on ANY event"""
        
        # 1. Extract facts
        facts = extract_facts(event)
        if not facts:
            return
        
        # 2. Enrich with metadata
        facts["_timestamp"] = now()
        facts["_source_event"] = event.topic
        facts["_embedding"] = await embed(facts)
        facts["_entities"] = extract_entities(facts)
        
        # 3. Route to systems
        await self.log_to_journal(facts)
        await self.index_in_superbrain(facts)
        await self.signal_consolidation()  # Wake SANCHO if needed
    
    async def log_to_journal(self, facts):
        """Append to Brain journal (raw capture)"""
        entry = format_journal_entry(facts)
        logseq.append_to_journal(entry)
    
    async def index_in_superbrain(self, facts):
        """Index fact in Qdrant vector database"""
        vector = facts["_embedding"]
        superbrain.index(
            vector=vector,
            metadata={
                "source": facts["_source_event"],
                "entities": facts["_entities"],
                "timestamp": facts["_timestamp"],
                "text": facts_to_text(facts)
            }
        )
    
    async def signal_consolidation(self):
        """Tell SANCHO to consolidate if threshold met"""
        entry_count = logseq.count_today_entries()
        if entry_count % 5 == 0:  # Every 5 entries
            sancho.trigger_task("memory_consolidation")

router = AutoMemoryRouter()
EventBus.instance().subscribe("*", router.on_event)
```

### Layer 2: Journal Enrichment (`core/memory/logseq_formatter.py`)
```python
def format_journal_entry(facts):
    """Format facts as Brain outliner entry with wikilinks"""
    
    source = facts["_source_event"]
    
    if source == "git.commit":
        return f"""
- [git] {facts['message'][:60]}
  - **Files changed:** {facts['files_changed']}
  - **Significance:** {classify_commit(facts['message'])}
  - **Entities:** {' '.join(f'[[{e}]]' for e in facts['_entities'])}
"""
    
    elif source == "task.completed":
        return f"""
- [task] {facts['subject']}
  - **Duration:** {facts['duration']}m
  - **Key decisions:** {facts.get('decisions', 'N/A')}
  - **Entities:** {' '.join(f'[[{e}]]' for e in facts['_entities'])}
  - **Blocked:** {facts.get('blocked_by', [])}
"""
    
    # ... handlers for other events
    
    # Default: raw JSON for unknown events
    return f"- [{source}] {json.dumps(facts)}\n"
```

### Layer 3: Consolidation (SANCHO Existing Task)
```python
# core/sancho/tasks.py - ADD THIS TASK TO EXISTING SANCHO

class MemoryConsolidationTask(ProactiveTask):
    """Consolidate daily journal into permanent memory files"""
    
    id = "memory_consolidation"
    schedule = "every 4 hours"
    gate = "has 5+ journal entries today"
    
    async def execute(self):
        # 1. Read today's journal
        journal = logseq.read_journal(today)
        
        # 2. Extract clusters of related facts
        clusters = await semantic_cluster(journal)
        
        # 3. For each cluster, update memory
        for cluster in clusters:
            fact_type = classify_facts(cluster)
            
            if fact_type == "decision":
                await update_memory_file("feedback_*", cluster)
            elif fact_type == "project":
                await update_memory_file("project_*", cluster)
            elif fact_type == "learning":
                await update_memory_file("research_*", cluster)
            
            # Create/update wiki page if needed
            await create_wiki_page(cluster)
        
        # 4. Rebuild knowledge graph
        await superbrain.rebuild_index()
        
        # 5. Log success
        emit("memory.consolidated",
             entries_processed=len(journal),
             clusters=len(clusters),
             memory_files_updated=count_updated())
```

### Layer 4: Query Interface (Superbrain)
```python
# core/superbrain/query.py - EXISTING, NO CHANGES NEEDED

async def query(question):
    """Semantic search over all captured facts"""
    
    # 1. Embed the question
    embedding = await embed(question)
    
    # 2. Search Qdrant
    results = qdrant.search(embedding, top_k=10)
    
    # 3. Synthesize answer
    answer = await llm.synthesize(
        question=question,
        context=results
    )
    
    return answer

# Example usage:
# "What did I decide about authentication?"
# → searches all journal entries + memory files
# → finds JWT decision from git commit + RFC + feedback memory
# → synthesizes comprehensive answer
```

## Boot Sequence (No Changes, Just Wire-Up)

```bash
# 1. Start Harvey as usual
python3 -m core.orchestration.dispatcher

# 2. Inside dispatcher boot, add:
from core.memory.auto_memory_router import AutoMemoryRouter
router = AutoMemoryRouter()
EventBus.instance().subscribe("*", router.on_event)

# 3. SANCHO already has memory_consolidation task:
# It runs every 4 hours automatically
# Consolidates journal → memory files → wiki → knowledge graph

# 4. That's it. Everything flows automatically.
```

## Example Flow: Single git commit triggers whole pipeline

```
$ git commit -m "Fix auth middleware: use httpOnly cookies instead of localStorage"

→ post-commit hook emits "git.commit.created" event
↓
→ EventBus fires
↓
→ AutoMemoryRouter.on_event()
  - Extract: message, files, diff
  - Classify: "This is a decision about security/auth"
  - Embed: "auth session tokens cookies compliance"
  - Find entities: [[CSO]], [[auth-middleware]], [[Traylinx]]
  - Significance score: 0.87 (high - security + compliance)
↓
→ Log to Brain journal
  - [git] Fix auth middleware: use httpOnly cookies
    - **Changed:** core/auth/middleware.py (150 lines)
    - **Significance:** SECURITY DECISION
    - **Context:** CSO audit flagged localStorage
    - **Related:** [[CSO]] [[compliance]] [[auth-middleware]]
↓
→ Index in Superbrain
  - Vector: [0.23, 0.45, ..., 0.87] (3072 dims)
  - Metadata: {source: "git", entities: ["CSO", "auth"], time: 2026-04-10, ...}
↓
→ Signal SANCHO (if 5+ entries captured today)
  - Wake memory_consolidation task
↓
→ SANCHO Dream (4-hour window or on demand)
  - Read today's journal (100+ entries)
  - Find cluster: "authentication decisions"
  - Synthesize: "Today made 3 auth decisions around token handling"
  - Update: memory/feedback_jwt_tokens.md
  - Update: data/Brain/pages/authentication.md
  - Rebuild: knowledge graph edges
  - Rebuild: Superbrain Qdrant index
↓
→ Result: System "remembers" automatically
  - Journal: raw capture (machine-readable)
  - Memory files: synthesized insights (human-readable)
  - Wiki: knowledge graph (browseable)
  - Superbrain: semantic index (queryable)
```

Later, when you ask:
```bash
superbrain query "What did we decide about session tokens?"
```

→ Searches all captured facts
→ Finds git commit, memory file, wiki page, related discussions
→ Synthesizes: "On 2026-04-10, decided to use httpOnly cookies with 15min rotation instead of localStorage. Driven by CSO compliance audit. Related tickets: TRX-2341."

**No manual prompting. No "remember this." It just works.**

## Files to Create/Modify

**New files:**
- `harvey-os/core/memory/auto_memory_router.py` (~150 LOC)
- `harvey-os/core/memory/logseq_formatter.py` (~200 LOC)

**Modified files:**
- `harvey-os/core/sancho/tasks.py` — Add MemoryConsolidationTask (if not exists)
- `harvey-os/core/orchestration/dispatcher.py` — Wire router at boot

**No changes to:**
- Brain (already being used)
- Superbrain (already works)
- SANCHO (just uses existing task system)
- EventBus (already fires events)

## Why This Works

1. **Uses existing systems** — No new framework. Just connects EventBus → Journal → Superbrain → SANCHO
2. **Automatic** — No user prompting. Facts flow continuously.
3. **Intelligent** — Embedding + semantic search understands meaning, not just keywords
4. **Scalable** — Handle 1000s of facts/day without overhead
5. **CLI-agnostic** — Works with Claude Code, Gemini CLI, Codex, OpenCode
6. **Human-like memory** — Sleep/consolidation metaphor matches actual neuroscience

## This Is Not a New System

This is **wiring together what already exists:**
- ✅ EventBus (capture)
- ✅ Brain (journal)
- ✅ Superbrain (semantic index)
- ✅ SANCHO (consolidation)

The missing piece was the **router** connecting them. That's it.
