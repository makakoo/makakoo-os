# Harvey Workflow System — Multi-Step Task Orchestration

## Problem: Long-Running Multi-Agent Tasks

You asked: "How do we not lose context and flow when tasks take hours/days and multiple processes and agents are involved?"

**Scenario:**
```
Day 1, 10am:  User: "Research AI safety for a week, give me a comprehensive report"
Day 1, 4pm:   Research task progresses, finds 15 papers, starts extracting
Day 2, 2am:   System restarts (crash, deploy, maintenance)
Day 2, 9am:   User asks: "What's the status?"

PROBLEM: Where were we? Did we lose the 15 papers? Which were extracted? Start over?
```

## Solution: Workflow Engine with Checkpointing

### Three-Layer Architecture

```
┌─────────────────────────────────────────────────────┐
│  HarveyChat (User Interface)                        │
│  - Receives "start research campaign"               │
│  - Shows status, pause prompts                      │
│  - Receives user feedback at pause points           │
└────────────────────┬────────────────────────────────┘
                     │
                     ↓ (creates & resumes)
┌─────────────────────────────────────────────────────┐
│  TaskQueue (Short: Q&A, single-step)                │
│  - Image generation (1-5 minutes)                   │
│  - Quick research lookups                           │
│  - User feedback collection                         │
└────────────────────┬────────────────────────────────┘
                     │
                     ↓ (for complex work)
┌─────────────────────────────────────────────────────┐
│  WorkflowEngine (Multi-step, multi-day)             │
│  - Full DAG orchestration                           │
│  - Dependency management                            │
│  - Checkpointing after each step                    │
│  - Failure recovery                                 │
│  - Human-in-the-loop pause points                   │
└────────────────────┬────────────────────────────────┘
                     │
          ┌──────────┼──────────┐
          ↓          ↓          ↓
    ┌─────────┐ ┌─────────┐ ┌──────────┐
    │ Agent 1 │ │ Agent 2 │ │ Agent N  │
    │(Harvey) │ │(ImageGen│ │(Research)│
    └─────────┘ └─────────┘ └──────────┘
```

**Key insight:** Workflows don't lose context because:
1. **Persistent state** — SQLite stores complete workflow state after each step
2. **Checkpointing** — Context saved after every step completes
3. **Restart-safe** — On restart, just resume from last checkpoint
4. **Dependency tracking** — Know which steps are done, which are waiting

## Workflow Structure

### Workflow = DAG of Steps

```python
wf = Workflow("research_campaign")

# Step 1: Requirements gathering (no dependencies)
wf.steps.append(WorkflowStep(
    id="step_1",
    name="Define Research Scope",
    agent="harvey",           # Which agent executes this
    action="define_scope",    # What to do
    input_context={},         # What step receives
    output_context={},        # What step produces
))

# Step 2: Search (depends on step 1)
wf.steps.append(WorkflowStep(
    id="step_2",
    name="Search Literature",
    agent="researcher",
    action="search_arxiv",
    depends_on=["step_1"],    # Only runs after step_1 is done
))

# Step 3: Extract (depends on step 2)
wf.steps.append(WorkflowStep(
    id="step_3",
    name="Extract Findings",
    agent="researcher",
    action="extract_findings",
    depends_on=["step_2"],
))

# Step 4: Get feedback (depends on step 3, PAUSE POINT)
wf.steps.append(WorkflowStep(
    id="step_4",
    name="Get User Preference",
    agent="user",
    action="input",
    depends_on=["step_3"],
    pause_prompt="Did we find what you need? Any directions to pursue?"
))

# Step 5: Synthesize (depends on step 4 resuming)
wf.steps.append(WorkflowStep(
    id="step_5",
    name="Synthesize Insights",
    agent="synthesizer",
    action="synthesize",
    depends_on=["step_4"],
))
```

### State Transitions

**Workflow states:**
```
DRAFT → QUEUED → RUNNING → PAUSED → RUNNING → COMPLETED
                    ↓                   ↑
                    └─ FAILED (if strategy="pause")
```

**Step states:**
```
PENDING → RUNNING → CHECKPOINTED ✓
          ↓
         FAILED (if max_retries hit)
         
PAUSED (awaiting input) → CHECKPOINTED (when resumed)

SKIPPED (if failed and strategy="skip")
```

### Context Flow

**Global context:** Shared across ALL steps
```python
wf.context = {
    "user_id": "123",
    "topic": "AI safety",
    "requirement_type": "comprehensive_report",
    
    # Step outputs accumulate here:
    "scope": {...},              # from step 1
    "papers": [...],             # from step 2
    "extracted_findings": {...}, # from step 3
    "user_choice": "focus_on_ethics",  # from step 4
    "insights": {...},           # from step 5
}
```

**Step-specific input/output:**
```python
step.input_context = wf.context.copy()  # Step receives current context
step.output_context = {...}             # Step produces new data
wf.context.update(step.output_context)  # Merge back to global
```

## Checkpointing: How Context Persists

After EVERY step completes, we save:

```sql
INSERT INTO workflow_checkpoints (workflow_id, step_id, checkpoint_at, context)
VALUES ('wf_123', 'step_3', 1712756400, '{"scope":..., "papers":..., "findings":...}')
```

**Recovery after crash:**
```
1. System restarts
2. Find workflow in RUNNING state
3. Find last checkpoint (step 3 completed)
4. Resume at step 4
5. Load context from checkpoint
6. Step 4 continues with full knowledge of steps 1-3
```

## Example: Image Generation Workflow

**2-day turnaround:**

```
Day 1, 2pm: User → HarveyChat
  "Generate a mascot for Harvey OS"
  
Day 1, 2:05pm: Workflow Step 1 (Gather Requirements)
  Agent: Harvey
  Action: requirements(initial_request)
  → Asks: "Style? (cartoon/realistic/abstract)"
  → Asks: "Size? (icon/banner/full-body)"
  → Saves answers to context
  → Step state: CHECKPOINTED
  
Day 1, 2:15pm: Workflow Step 2 (Generate Variations)
  Agent: Image Generation
  Action: generate_variations(context)
  Input context includes requirements from step 1
  → Generates 3 variations
  → Saves to ~/pics/
  → Step output: { "variation_1": "path", "variation_2": "path", ... }
  → Step state: CHECKPOINTED
  → Global context now includes all variations
  
Day 1, 2:20pm: Workflow Step 3 (Get User Feedback) — PAUSE POINT
  Agent: User Input
  Action: input(pause_prompt)
  Prompt: "Which do you prefer? (1-3) or request changes"
  → Workflow state: PAUSED
  → Sends variations to user via Telegram
  
Day 1, 3pm: User responds
  "I like #2, but make the owl bigger"
  → HarveyChat receives message
  → Detects: workflow paused, waiting for input
  → resume_workflow(wf, {"user_choice": 2, "feedback": "make owl bigger"})
  → Context updated with user input
  
Day 1, 3:05pm: Workflow Step 4 (Generate Refined)
  Agent: Image Generation
  Action: refine(context)
  Input: variation #2 + "make owl bigger"
  → Generates refined version
  → Step state: CHECKPOINTED
  
Day 1, 3:10pm: Workflow Step 5 (Upscale)
  Agent: Processor
  Action: upscale(context)
  → Upscales to 4K
  → Step state: CHECKPOINTED
  
Day 1, 3:15pm: Workflow Step 6 (Archive)
  Agent: Storage
  Action: save_to_brain(context)
  → Saves to ~/MAKAKOO/data/resources/
  → Logs to Brain journal
  → Step state: CHECKPOINTED
  
Day 1, 3:16pm: Workflow state: COMPLETED
  → Result available to user
  → Full conversation preserved in context
  
SCENARIO: Crash at step 4
  → System restarts
  → Finds workflow in RUNNING state
  → Loads last checkpoint (step 3 completed)
  → Context fully restored (user_choice, feedback, etc.)
  → Resumes at step 4 (upscale)
  → No work lost!
```

## Integration with HarveyChat + TaskQueue

### Decision Tree: TaskQueue vs Workflow

```
User request arrives at HarveyChat
  ↓
Estimate complexity:
  
  < 10 minutes, 1-2 steps?
    → Use TaskQueue (current agentic flow)
    → Example: "generate image", "summarize PDF"
    
  > 10 minutes, 3+ steps, or multi-day?
    → Use Workflow
    → Example: "research X for a week", "create marketing campaign"
    
  Needs human feedback mid-way?
    → TaskQueue for simple (1 clarification)
    → Workflow for complex (3+ pause points)
```

### Flow Diagram

```
HarveyChat receives: "Generate mascot for Harvey"
  ↓
classify_request():
  - Estimated time: 30 mins
  - Multi-step: requires + generate + feedback + upscale + archive
  - Decision: WORKFLOW
  ↓
WorkflowTemplates.image_generation_workflow()
  ↓
engine.create_workflow(...)
engine.start_workflow(wf)
  ↓
executor polls (every 30 seconds)
  ↓
  Step 1: Requirements → checkpoint
  Step 2: Generate → checkpoint
  Step 3: Pause (awaiting feedback)
  ↓
  [User responds to pause prompt]
  ↓
  HarveyChat → resume_workflow(wf, user_input)
  ↓
  Step 4: Refine → checkpoint
  Step 5: Archive → checkpoint
  ↓
Workflow.state = COMPLETED
Send result to user
```

## Preventing Context Loss: Key Mechanisms

### 1. Persistent State (SQLite)

```sql
-- Workflows table: Full workflow state serialized as JSON
workflows(id, name, state, steps[], context{}, current_step_idx)

-- Checkpoints table: State after each step
workflow_checkpoints(workflow_id, step_id, checkpoint_at, context)
```

### 2. Atomic Checkpointing

After EVERY step:
```python
step.completed_at = time.time()
step.output_context = handler_result
wf.context.update(step.output_context)  # Merge

# ONLY after success:
engine.save_workflow(wf)
executor._checkpoint(wf, step)
```

### 3. Graceful Restart

On executor startup:
```python
# Find all workflows in RUNNING/PAUSED state
cursor = db.execute("SELECT * FROM workflows WHERE state IN (?, ?)",
                    (WorkflowState.RUNNING, WorkflowState.PAUSED))

# For each, load latest checkpoint
for wf_row in cursor:
    wf = Workflow.from_dict(wf_row.data)
    latest_checkpoint = db.execute(
        "SELECT context FROM workflow_checkpoints WHERE workflow_id = ? "
        "ORDER BY checkpoint_at DESC LIMIT 1"
    ).fetchone()
    
    if latest_checkpoint:
        wf.context = json.loads(latest_checkpoint.context)
        # Resume from current_step_idx
```

### 4. Dependency Tracking

```python
step.depends_on = ["step_1", "step_3"]  # This step waits for those

before_running_step():
    if not engine.can_start_step(step):
        # Skip, wait for dependencies
        return
```

## Failure Handling

### Strategy 1: Pause on Failure
```python
wf.failure_strategy = "pause"

# If step 3 fails:
step.state = FAILED
step.error = "PDF parsing failed: corrupt file"
step.retry_count += 1

if step.retry_count >= step.max_retries:
    wf.state = PAUSED
    wf.pause_reason = "Step failed: PDF parsing. Manual intervention needed?"
    # User can:
    # - Retry (set max_retries higher, resume)
    # - Skip (set step.state = SKIPPED, resume)
    # - Edit context and retry
```

### Strategy 2: Skip on Failure
```python
wf.failure_strategy = "skip"

# If step 3 fails:
step.state = SKIPPED
# Continue to step 4 (if no dependencies on step 3)
```

## Monitoring: WorkflowExecutor Status

```bash
# Check running workflows
curl http://localhost:18080/workflow/status

# Output:
{
  "running": true,
  "poll_interval": 30.0,
  "workflows": {
    "running": 3,
    "paused": 1,
    "completed": 42,
    "failed": 2
  }
}
```

Dashboard view:
```
WORKFLOWS STATUS
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Running:    3 workflows (in progress)
  - wf_123: Image Generation    [████░░░░░] 40% (step 3/6)
  - wf_124: Research Campaign    [███░░░░░░] 30% (step 2/7, paused for input)
  - wf_125: Doc Processing       [██░░░░░░░] 20% (step 1/5)

Paused:     1 workflow (awaiting input)
  - wf_124: Research → "Did we find what you need?"

Completed:  42 workflows (this week)
Failed:     2 workflows (requires manual fix)
```

## Built-in Step Handlers

The executor comes with handlers for:
- `user/input` — Pause and collect user feedback
- `brain/log_event` — Log to journal
- `telegram/notify` — Send message to user
- `system/delay` — Rate limiting, scheduled delays
- Custom handlers can be registered for any agent/action pair

## Next: Integration with Existing Systems

1. **Hook into HarveyChat gateway.py**
   - Detect long-running requests
   - Auto-create workflow instead of single task
   - Send pause notifications back to Telegram

2. **Executor runs as daemon**
   - `python3 -m core.workflow executor --daemon`
   - Or triggered by cron: `0 * * * * python3 -m core.workflow executor`

3. **Resume from HarveyChat**
   - User responds to pause prompt
   - HarveyChat detects active workflow
   - Calls `engine.resume_workflow(wf, user_input)`

4. **Observability**
   - Log all checkpoints to Brain journal
   - Dashboard showing workflow progress
   - Alerts on failures

## Testing

```bash
# Start executor daemon
python3 -m core.workflow executor --daemon

# Create and run test workflow
python3 << 'EOF'
from core.workflow.engine import WorkflowEngine
from core.workflow.executor import WorkflowExecutor, WorkflowTemplates

engine = WorkflowEngine()
executor = WorkflowExecutor(engine)

# Register test handlers
@engine.register_handler("test", "step1")
def handler_step1(step, context):
    return {"result": "step1_done"}

# Create workflow
wf = engine.create_workflow("test", steps=[...])
engine.start_workflow(wf)

# Execute steps
executor.execute_cycle()
executor.execute_cycle()

# Check status
print(engine.get_workflow(wf.id))
EOF

# Monitor
sqlite3 ~/MAKAKOO/data/workflow/workflows.db \
  "SELECT id, name, state FROM workflows ORDER BY started_at DESC LIMIT 5;"
```

## Summary: Why This Prevents Context Loss

| Challenge | Solution |
|-----------|----------|
| Process crash during step 5 | Last checkpoint (step 4) saved, resume from there |
| Lost intermediate data | Each step output stored in global context |
| Restart at wrong place | current_step_idx tracked, dependencies respected |
| Multiple agent handoffs | Context passed via wf.context, fully serializable |
| Long-running jobs | Checkpoints after every step, no progress lost |
| Human feedback needed | Pause points with prompts, resume with input |
| Rate limits / delays | Explicit delay steps in DAG |
| Circular dependencies | Dependency tracking prevents infinite loops |
| Cascading failures | failure_strategy (pause or skip) prevents cascade |
