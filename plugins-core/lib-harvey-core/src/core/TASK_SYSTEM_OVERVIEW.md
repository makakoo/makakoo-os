# Harvey Complete Task System — Three Tiers

## Overview: Everything Works Together

```
User: "Research AI safety for a week, give me a report"
  ↓
HarveyChat Gateway (gateway.py)
  classify_complexity() → "This is a WORKFLOW (weeks, 7+ steps, needs feedback)"
  ↓
Create Workflow with 10 steps:
  Step 1: Define scope (Harvey agent) → CHECKPOINT
  Step 2: Search literature (Research agent) → CHECKPOINT
  Step 3: Extract findings (Research agent) → CHECKPOINT
  Step 4: Get user feedback (User input pause) ← PAUSE POINT
  Step 5: Synthesize (Synthesizer agent) → CHECKPOINT
  ...
  ↓
WorkflowExecutor (executor.py, running as daemon)
  Polls every 30 seconds for QUEUED/RUNNING workflows
  ↓
  Executes Step 1 → Step 2 → Step 3 → [PAUSED]
  Checkpoints after each step
  ↓
[Day 2: User provides feedback]
  HarveyChat receives response
  Detects: workflow paused, resume with input
  ↓
  Executor continues: Step 5 → Step 6 → ... → COMPLETED
  ↓
[Day 7: Workflow completes]
  Final report sent to user
  Full context preserved from Day 1 to Day 7
```

## Three-Tier System Comparison

| Layer | Module | Duration | Use Case | Example |
|-------|--------|----------|----------|---------|
| **TIER 1: TaskQueue** | core.chat.task_queue | Seconds to minutes | Q&A style, single roundtrip | "Generate image" → image generated |
| **TIER 2: Workflow** | core.workflow | Minutes to weeks | Multi-step DAG, checkpointing | "Research for a week" → comprehensive report |
| **TIER 3: Campaign** | (future) | Weeks to months | Multi-workflow coordination | Product launch (multiple parallel workflows) |

## Tier 1: TaskQueue — Quick Interactive Work

**When to use:**
- User asks a question, expects answer in same conversation
- Multi-turn refinement but within 10-15 minutes
- Single agent handling the work

**How it works:**
```python
task = task_queue.create_task(channel, user_id, goal)
# → Task state: QUEUED → RUNNING → AWAITING_INPUT → COMPLETED

task.awaiting_input_prompt = "What style?"
# → Telegram shows question, waits for response

user_input → resume_task(task, response)
# → Continue with full history preserved
```

**Files:**
- core.chat.task_queue.py (TaskQueue, Task, TaskState)
- core.chat.conversation.py (ConversationState, ConversationManager)
- core.chat.gateway.py (integrated)

**Example flow:**
```
User: "Generate image"
  → Task created, state: QUEUED
  → LLM asks: "What style?"
  → Task state: AWAITING_INPUT
  → User: "Watercolor"
  → resume_task() continues with history
  → Image sent, Task state: COMPLETED
```

## Tier 2: Workflow — Complex Multi-Agent Work

**When to use:**
- Task takes >10 minutes
- 3+ steps, multiple agents
- Might crash/restart mid-way
- Needs human feedback mid-way
- Long-running campaigns

**How it works:**
```python
wf = Workflow("research_campaign")
wf.steps = [
    WorkflowStep(id="1", agent="harvey", action="scope", depends_on=[]),
    WorkflowStep(id="2", agent="researcher", action="search", depends_on=["1"]),
    WorkflowStep(id="3", agent="user", action="input", depends_on=["2"],
                 pause_prompt="Any directions?"),
    ...
]

engine.start_workflow(wf)
# → Workflow state: QUEUED

# Executor polls and executes
executor.execute_cycle()
# → Step 1 RUNNING → CHECKPOINTED
# → Step 2 RUNNING → CHECKPOINTED
# → Step 3 PAUSED (awaiting input)

# User responds
resume_workflow(wf, user_input)
# → Step 4 RUNNING → CHECKPOINTED
# → ... until COMPLETED
```

**Files:**
- core.workflow.engine.py (Workflow, WorkflowStep, WorkflowEngine)
- core.workflow.executor.py (WorkflowExecutor, WorkflowTemplates)
- core.workflow.__init__.py (exports)

**Example flow:**
```
Day 1: User: "Research AI safety for a week"
  → Workflow created (10 steps)
  → Step 1-3: executed, checkpointed
  → Step 4: PAUSED (ask for direction)

Day 2: User: "Focus on ethics"
  → resume_workflow(wf, {direction: "ethics"})
  → Step 5-10: continue executing
  → All context from Day 1 preserved

Day 7: Workflow COMPLETED
  → Report sent
  → Full conversation preserved
```

## Tier 3: Campaign (Future)

**When to use:**
- Multiple workflows in sequence
- Cross-workflow dependencies
- Month-long projects

**Concept:**
```
Campaign: Product Launch
  ├─ Workflow 1: Research competition (Week 1)
  ├─ Workflow 2: Define strategy (Week 2, depends on 1)
  ├─ Workflow 3: Create marketing plan (Week 2, parallel)
  ├─ Workflow 4: Build assets (Week 3, depends on 2+3)
  └─ Workflow 5: Launch (Week 4, depends on 4)
```

## Decision Tree: Which Layer to Use?

```
User request arrives
  ↓
How long will it take?
  ├─ < 10 min?
  │   └─ TaskQueue (TIER 1)
  │
  ├─ 10 min to 1 week?
  │   └─ Workflow (TIER 2)
  │
  └─ > 1 week?
      └─ Campaign (TIER 3, future)

How many steps?
  ├─ 1-2 steps?
  │   └─ TaskQueue
  │
  ├─ 3-10 steps?
  │   └─ Workflow
  │
  └─ 10+ steps / multiple workflows?
      └─ Campaign

Does it need human feedback?
  ├─ Once (mid-way)?
  │   └─ Can use either
  │
  ├─ Multiple times (3+)?
  │   └─ Workflow
  │
  └─ No feedback?
      └─ Either

Could it crash/restart?
  ├─ Yes, and would lose important work?
  │   └─ Workflow (has checkpointing)
  │
  └─ No, quick enough to redo?
      └─ Either
```

## Context Preservation: Three Mechanisms

### Mechanism 1: TaskQueue — In-Conversation Memory

```python
# Conversation history maintained
store.add_message(channel, user_id, "user", text)
history = store.get_history(channel, user_id, limit=20)
# → Full history passed to each LLM call
```

**Survives:** Message boundaries, task state changes
**Lost on:** Manual `/clear` command, new conversation started

### Mechanism 2: Workflow — Checkpoint-Based Persistence

```python
# After each step completes:
wf.context.update(step.output_context)
engine.save_workflow(wf)
_checkpoint(wf, step)  # Save to SQLite

# On restart:
latest_checkpoint = db.query("... ORDER BY checkpoint_at DESC LIMIT 1")
wf.context = json.loads(latest_checkpoint.context)
# → Resume with full context
```

**Survives:** Process crashes, restarts, deployments, hours/days/weeks of time
**Lost on:** Manual deletion, database corruption

### Mechanism 3: Brain Journal — Long-Term Memory

```python
# Every significant event logged
brain.append_journal(f"- [[Workflow]] {wf.name}: Step 5 completed")

# Can query later
superbrain.query("What happened in the research campaign?")
# → Finds all journal entries, consolidates insights
```

**Survives:** Everything, permanent record
**Purpose:** Learning, auditing, future context

## Architecture Diagram

```
┌────────────────────────────────────────────────────────────┐
│                    HarveyChat (User Interface)              │
│  - Telegram, WhatsApp, Discord                             │
│  - Sends/receives messages                                  │
└──────────────────────┬─────────────────────────────────────┘
                       │
                       ↓ handle_message(text)
┌──────────────────────────────────────────────────────────────┐
│             Gateway (gateway.py)                             │
│  - classify_request: TaskQueue vs Workflow?                 │
│  - Detect: new vs resume task/workflow                      │
│  - Route to appropriate executor                            │
└──┬──────────────────────────────────┬──────────────────────┘
   │                                  │
   ↓ (Quick work)                     ↓ (Complex work)
┌────────────────────────┐      ┌──────────────────────┐
│ TaskQueue              │      │ WorkflowEngine       │
│ (core/chat/task_queue) │      │ (core/workflow)      │
│                        │      │                      │
│ States:                │      │ States:              │
│ Q→R→A→C               │      │ D→Q→R→P→C           │
│                        │      │                      │
│ Context:               │      │ Context:             │
│ - Per-task messages    │      │ - Global (shared)    │
│ - Awaiting_input_prompt│      │ - Per-step output    │
│ - Files to send        │      │ - Checkpoints (SQLite)
└────────────────────────┘      └──────────────────────┘
         ↓                              ↓
         │                    WorkflowExecutor
         │                    (polling daemon)
         │                            │
         ↓                            ↓
    [HarveyAgent]          [Step Handlers Registry]
    (bridge, tools)         (user/input, brain/log, etc)
         ↓                            ↓
    [External APIs]          [Custom Agent Handlers]
    (Telegram, LLM)          (Harvey, ImageGen, Researcher)
```

## State Persistence Across All Layers

```
Message arrives
  ↓
HarveyChat stores in: message_store (SQLite)
  ↓
TaskQueue uses: task_queue (SQLite)
  ↓
Workflow uses: workflows + checkpoints (SQLite)
  ↓
Brain logs to: journals/YYYY_MM_DD.md
  ↓
Superbrain indexes: superbrain_store (SQLite/Qdrant)
```

**Result:** Multiple complementary persistence layers
- Immediate: In-memory (wf.context, task.messages)
- Short-term: SQLite (messages, tasks, checkpoints)
- Long-term: Brain (journals, memory files, knowledge graph)

## Testing Each Layer

### Tier 1: TaskQueue
```bash
# Start HarveyChat daemon
python3 -m core.chat start --daemon

# Test task flow
# In Telegram:
#  1. "generate image" → LLM asks "what style?"
#  2. "watercolor" → image sent
#  3. Verify task state in DB
sqlite3 ~/MAKAKOO/data/chat/tasks.db "SELECT state FROM tasks WHERE id='task_xxx';"
```

### Tier 2: Workflow
```bash
# Start executor daemon
python3 -m core.workflow executor --daemon

# Create test workflow
python3 << 'EOF'
from core.workflow.engine import WorkflowEngine
from core.workflow.executor import WorkflowTemplates

engine = WorkflowEngine()
wf = WorkflowTemplates.research_workflow("AI safety", depth="medium")
engine.start_workflow(wf)
EOF

# Watch execution
sqlite3 ~/MAKAKOO/data/workflow/workflows.db \
  "SELECT name, state, current_step_idx FROM workflows ORDER BY started_at DESC LIMIT 1;"
```

## Performance Characteristics

| Operation | Tier 1 | Tier 2 |
|-----------|--------|--------|
| Create task/workflow | <1ms | <1ms |
| Store message | <5ms | <5ms |
| Checkpoint after step | - | <20ms (SQLite write) |
| Resume from pause | <10ms | <10ms + LLM latency |
| Poll cycle (100 workflows) | - | ~100-500ms (depends on handler execution) |
| Query recent messages | <50ms | <50ms |

## Summary: What You Now Have

✅ **Tier 1 (TaskQueue):** Multi-turn Q&A with persistent state
- User can refine, clarify, iterate
- Within single conversation
- Survives message boundaries

✅ **Tier 2 (Workflow):** Multi-step campaigns with checkpointing
- Hours, days, weeks
- Multiple agents
- Survives crashes, restarts, deployments
- Human-in-the-loop feedback
- Full context preserved

🔜 **Tier 3 (Campaign):** Multi-workflow orchestration (future)
- Months-long projects
- Cross-workflow dependencies
- Parallel execution

**Result:** Harvey can now:
- Answer questions with multi-turn refinement (seconds to minutes)
- Execute complex multi-agent campaigns (minutes to weeks)
- Survive system failures without losing context
- Handle human feedback mid-way
- Scale from simple to extremely complex tasks
