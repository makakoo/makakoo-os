---
name: autoimprover
description: Harvey OS self-improvement system — background review, memory/skill nudges, budget enforcement, session compaction, activity logging, goal hierarchy
version: 7.0.0
---

# Auto-Improver

Harvey's self-improvement system. Learns from every session by:

1. **Nudging memory saves** after N turns — user facts, preferences, work style
2. **Nudging skill creation** after N iterations — non-trivial approaches worth codifying
3. **Running background review agents** to create/update skills and memory
4. **Enforcing token budgets** with warnings at 80% and hard stops at configured limits
5. **Compacting sessions** when context gets large (50+ turns, 150K+ input tokens, 500K+ total tokens, 2+ hours)
6. **Logging all significant actions** for audit and analysis (append-only JSONL)
7. **Linking tasks to goals** so sub-agents see WHY

---

## Quick Start

```python
from harvey-os.skills.meta.autoimprover import AutoImprover

improver = AutoImprover(
    session_id="abc123",
    memory_nudge_interval=10,
    skill_nudge_interval=10,
)

# In agent loop — per turn:
improver.on_turn()

# After each tool call:
improver.on_tool_call("read_file", success=True)

# After each API call:
improver.on_api_call(input_tokens=500, output_tokens=1000)

# Check if review should fire:
if improver.should_review():
    improver.spawn_review(messages, callback=print)

# Budget check before next API call:
if improver.should_stop():
    raise BudgetExceededError()

# Compaction check:
if improver.should_compact():
    handoff = improver.run_compaction(messages)
```

---

## Architecture

```
Agent Loop
  │
  ├─ on_turn() ──────────────────────────────► NudgeTriggers
  │                                              ├─ turn_count++ → should_review_memory()
  │                                              └─ iteration_count++ → should_review_skills()
  │
  ├─ on_tool_call(tool, success) ─────────────► NudgeTriggers
  │                                              └─ iteration_count++ → should_review_skills()
  │
  ├─ on_api_call(input, output) ───────────────► BudgetTracker ──► BudgetEnforcer
  │                                              └─ check_budget() → can_continue() / should_stop()
  │
  ├─ should_review() ─────────────────────────► ReviewSpawner (daemon thread)
  │                                              ├─ MemoryNudge → BrainWriter → Brain
  │                                              └─ SkillNudge → SkillManager → harvey-os/skills/
  │
  ├─ should_compact() ────────────────────────► CompactionPolicy
  │                                              └─ HandoffGenerator + SessionArchiver
  │
  └─ ActivityLogger (every event) ────────────► data/logs/activity/YYYY/MM/YYYY_MM_DD.jsonl
```

**Nested nudges are always disabled** in the review agent — no infinite loops.

---

## Core Loop

| Step | Method | What it does |
|------|--------|--------------|
| 1 | `on_turn()` | Increment turn counter; check memory nudge threshold |
| 2 | `on_tool_call(tool, success)` | Increment iteration counter; check skill nudge threshold; update budget |
| 3 | `on_api_call(input, output, ...)` | Record token usage; check budget enforcement |
| 4 | `should_review()` | Returns True when memory or skill nudge threshold is hit |
| 5 | `spawn_review(messages, callback)` | Fork background daemon — runs memory/skill review |
| 6 | `should_compact()` | Check compaction thresholds (turns, tokens, age) |
| 7 | `run_compaction()` | Generate handoff, archive session to Brain, reset counters |

---

## API Reference

### Nudges & Review

**`on_turn()`**
Increment turn counter. Call after each user turn.

**`on_tool_call(tool_name, success)`**
Increment iteration counter. Call after each tool invocation.
```python
improver.on_tool_call("Read", success=True)
improver.on_tool_call("Bash", success=False)  # failures still count as iterations
```

**`should_review_memory() -> bool`**
True when `turn_count >= memory_nudge_interval`. Resets on review or on memory tool use.

**`should_review_skills() -> bool`**
True when `iteration_count >= skill_nudge_interval`. Resets on review or on skill manager use.

**`should_review() -> bool`**
True when either memory or skill review should fire.

**`spawn_review(messages, callback)`**
Fork a background daemon that runs the review. Non-blocking.
```python
if improver.should_review():
    improver.spawn_review(
        messages_snapshot=messages,
        callback=lambda result: print(f"Review done: {result}")
    )
```

**Review prompts:**

*Memory nudge:*
> Review the conversation above and consider saving to memory if appropriate. Focus on user facts, preferences, work style. Write to Brain. If nothing worth saving, say 'Nothing to save.'

*Skill nudge:*
> Review the conversation above and consider saving or updating a skill if appropriate. Focus on non-trivial approaches that required trial and error. Create or patch skills under `harvey-os/skills/<category>/`. If nothing worth saving, say 'Nothing to save.'

---

### Iteration Budget

**`budget.consume() -> bool`**
Try to use one iteration. Returns False when exhausted. Free tools (execute_code, bash) are refunded — they do not consume budget.

```python
if not budget.consume():
    raise IterationExhaustedError()
```

**`budget.refund()`**
Refund the last consumed iteration. Used for bash/execute_code.

**`budget.remaining -> int`**
Current remaining iterations.

---

### Budget

**`on_api_call(input_tokens, output_tokens, cached_tokens=0, reasoning_tokens=0)`**
Record token usage from an API call. Updates running totals for both token and cost limits.

```python
improver.on_api_call(
    session_id="abc123",
    input_tokens=1200,
    output_tokens=3404,
    cached_tokens=500,
    reasoning_tokens=200,
)
```

**`check_budget(session_id) -> BudgetStatus`**
Returns full budget status:
```python
@dataclass
class BudgetStatus:
    state: BudgetState          # OK, WARNING, EXCEEDED, PAUSED
    spent_tokens: int
    spent_cost: float
    limit_tokens: int
    limit_cost: float
    pct_tokens: float           # 0.0–1.0+
    pct_cost: float             # 0.0–1.0+
    message: str                 # Human-readable
    trigger: str | None         # Which limit triggered
```

**`can_continue(session_id) -> bool`**
True if session is under budget and can make another API call.

**`should_stop() -> bool`**
True if session has exceeded budget — hard stop. Do not make further API calls.

---

### Compaction

**`should_compact() -> bool`**
True when ANY threshold is met:
| Condition | Default threshold |
|-----------|-------------------|
| Turn count | 50+ turns |
| Input tokens | 150K+ |
| Total tokens | 500K+ |
| Session age | 2+ hours |

A grace period of 10 turns prevents compaction on trivial sessions.

**`compaction_trigger_reason() -> str | None`**
Returns e.g. `"turn limit: 51 >= 50"` or None if no trigger.

**`run_compaction(messages) -> dict`**
```python
result = improver.run_compaction(messages_snapshot)
# result["user_message"]  → "📦 Session compacted — 3 previous sessions archived"
# result["handoff"]      → handoff summary string (prepend to new session)
# result["session_id"]   → archived session ID
```

---

### Activity Logging

**`activity_logger.log(action, **kwargs)`**
Log a raw event to the append-only JSONL audit trail.

**Convenience methods:**
```python
logger = improver.activity_logger

logger.on_turn(turn_number=5, input_tokens=1200, output_tokens=3400)
logger.on_tool_call(tool_name="Read", tool_input={"file_path": "..."})
logger.on_tool_result(tool_name="Read", success=True, duration_ms=45)
logger.on_api_call(input_tokens=1200, output_tokens=3400, cost=0.042)
logger.on_memory_nudge()
logger.on_skill_nudge()
logger.on_review(memory=True, skill=False)
logger.on_review_completed(memory_entries=3, skills_updated=1)
logger.on_compaction_start()
logger.on_compaction_completed(session_id="sess_abc", runs=52)
logger.on_budget_warning(pct=0.82, spent_tokens=410000)
logger.on_budget_exceeded(spent_tokens=500000)
logger.on_session_start()
logger.on_session_end()
logger.on_goal_created(goal_id="goal_abc", title="Launch v1.0")
logger.on_goal_completed(goal_id="goal_abc")
logger.on_task_created(task_id="task_xyz", goal_id="goal_abc")
logger.on_task_completed(task_id="task_xyz")
logger.on_skill_created(name="code-review", category="dev")
logger.on_skill_evaluated(name="code-review", quality_score=0.85)

logger.flush()   # Ensure events are on disk
logger.close()   # Flush and release
```

**Log file location:** `data/logs/activity/YYYY/MM/YYYY_MM_DD.jsonl`

---

### Goals & Tasks

**`create_goal(title, description, priority, parent_id) -> Goal`**
```python
goal = improver.create_goal(
    title="Launch v1.0",
    description="Ship the initial version",
    priority=GoalPriority.HIGH,
    parent_id=None,         # None = root goal
)
# Returns Goal(id="goal_abc123", ...)
```

**`get_goal(goal_id) -> Goal`**

**`update_goal(goal_id, **fields)`** — title, description, priority, state

**`complete_goal(goal_id)` / `abandon_goal(goal_id)` / `block_goal(goal_id)`**

**`get_goal_context(goal_id) -> str`**
Returns indented tree context:
```
Launch v1.0
 └── User accounts
     └── Implement authentication (THIS)
```

**`create_task(goal_id, title, description) -> Task`**
```python
task = improver.create_task(
    goal_id=goal.id,
    title="Implement OAuth2 flow",
    description="Authorization code flow with PKCE",
)
```

**`complete_task(task_id)` / `update_task(task_id, **fields)`**

**`inject_task_context(task_id, prompt) -> str`**
Prepends the goal ancestry to a sub-agent prompt so the agent understands WHY:
```
You are implementing OAuth2 because:
 └── Launch v1.0
     └── User accounts
         └── Implement authentication (THIS)

Implement the OAuth2 authorization code flow with PKCE.
```

**`get_status_summary() -> str`**
Returns a human-readable summary of all active goals, tasks, and progress.

---

## Goal Hierarchy Data Model

```python
@dataclass
class Goal:
    id: str
    title: str
    state: GoalState          # ACTIVE, COMPLETED, ABANDONED, BLOCKED
    priority: GoalPriority    # CRITICAL, HIGH, MEDIUM, LOW
    parent_id: str | None
    child_ids: list[str]
    task_ids: list[str]
    progress: float           # 0.0–1.0, auto-calculated
    created_at: str
    updated_at: str

@dataclass
class Task:
    id: str
    title: str
    state: TaskState         # PENDING, IN_PROGRESS, COMPLETED, CANCELLED
    goal_id: str | None       # None = standalone
    created_at: str
    updated_at: str
    completed_at: str | None
```

**Goal priority:** CRITICAL > HIGH > MEDIUM > LOW

**Progress calculation:** If goal has child goals, progress = average of child progresses. If it has tasks but no children, progress = fraction of completed tasks.

**Storage:** Goals and tasks are persisted as Brain pages at `data/Brain/pages/goals/` and `data/Brain/pages/tasks/`. Thread-safe via `fcntl` locks + atomic writes.

---

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HARVEY_MEMORY_NUDGE_INTERVAL` | 10 | Turns between memory reviews |
| `HARVEY_SKILL_NUDGE_INTERVAL` | 10 | Iterations between skill reviews |
| `HARVEY_MAX_TOKENS_SESSION` | 500,000 | Max total tokens per session |
| `HARVEY_MAX_COST_SESSION` | 10.0 | Max total cost per session (USD) |
| `HARVEY_WARN_AT_PCT` | 0.80 | Warning threshold (80%) |
| `HARVEY_COST_PER_MILLION` | 15.0 | Token cost per 1M tokens (USD) |

### CompactionPolicy Defaults

| Parameter | Default |
|-----------|---------|
| `max_session_runs` | 50 |
| `max_raw_input_tokens` | 150,000 |
| `max_total_tokens` | 500,000 |
| `max_session_age_hours` | 2.0 |
| `min_session_runs_before_compaction` | 10 |

### Config File
`data/autoimprover/config.json` — overrides defaults.

---

## File Structure

```
autoimprover/
├── __init__.py              # AutoImprover class (integrates all sprints)
├── review_spawner.py        # Background daemon thread + review prompts
├── nudge_triggers.py        # Turn/iteration counters + interval config
├── brain_writer.py          # Brain writes from review agent
├── skill_manager.py         # Agent skill creation/editing
├── iteration_budget.py      # Thread-safe budget with refunds for free tools
├── compaction_policy.py    # Compaction thresholds + CompactionState
├── handoff_generator.py     # LLM (Gemini) or template summarization
├── session_archiver.py      # Session archive to Brain
├── budget_tracker.py        # Token accounting per session
├── budget_enforcer.py       # BudgetState enum + warning/stop state machine
├── budget_config.py          # BudgetPolicy, BudgetLimit, DEFAULT_POLICY
├── activity_logger.py       # JSONL audit trail + ActivityAction enum
├── log_reader.py            # Log query interface
├── log_analyzer.py          # Log analysis and reporting
├── goal_tracker.py          # Goal CRUD + ancestry
├── goal_hierarchy.py        # Goal context injection, breadcrumb, cycle validation
├── task_linker.py           # Task→goal linkage + prompt injection
└── test_autoimprover.py    # Integration tests
```

---

## Trigger Phrases

| Phrase | Effect |
|--------|--------|
| `self improve` | Run self-improvement check |
| `learn from this` | Trigger memory/skill review |
| `budget` | Show current budget status |
| `token limit` | Show budget limits and usage |
| `cost tracking` | Show cost breakdown |
| `session compact` / `compact session` | Trigger immediate compaction |
| `goal` | Create or retrieve a goal |
| `task hierarchy` | Show goal hierarchy for a task |
| `why does this matter` | Show breadcrumb for current task |
| `activity log` | Summary of recent activity |
| `audit` | Detailed audit trail for a session |
| `what happened` | Formatted timeline of recent events |

---

## Safety

- **Nested nudges disabled:** Review agents run with `nested_nudges=False` — cannot spawn further reviews.
- **Iteration refunds:** `bash` and `execute_code` iterations are "free" and refunded — they do not count against review budget.
- **Daemon-only:** Review runs in a daemon thread; the user-facing response is never blocked.
- **Immutable logs:** Activity log is append-only JSONL — events cannot be modified or deleted after writing.
- **Budget hard stop:** `should_stop()` returns True at 100%+ of any budget limit — no further API calls allowed.
- **Thread-safe storage:** All disk I/O uses `fcntl` file locks + atomic temp-file + `os.replace()`.
