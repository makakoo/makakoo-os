---
name: dev-orchestrator
description: Use when Sebastian says "build X", "implement Y", or any multi-component development request that can be parallelized across independent tasks
---

# Dev Orchestrator — Parallel Development Team

## Overview

The Dev Orchestrator is how Harvey runs development like a startup team with parallel agents. When given a multi-component task, it breaks work into independent units, launches agents in parallel, and consolidates results.

**Core principle:** Parallel by default. If tasks don't depend on each other, run them simultaneously.

## When to Use

```
User: "build the auth system"
User: "implement the checkout flow"
User: "add user profiles + dashboard + notifications"
User: "build X feature"
```

**Dev Orchestrator is the right tool when:**
- The request has 2+ components that can be built independently
- No shared state between components during implementation
- Components don't need output from each other mid-build
- You want 4x-10x speedup on implementation

**Do NOT use when:**
- Components are tightly coupled (building A requires knowing B's internals)
- There's a single logical unit of work
- You need sequential iteration (build A, review A, build B, review B)

## The Mental Model

Treat each agent as a **person on your team**:

| Agent Name | Role | Responsibility |
|------------|------|----------------|
| team-auth | Backend engineer | Database models, API endpoints, middleware |
| team-frontend | Frontend engineer | UI components, state management |
| team-tests | QA engineer | Test suite, integration tests |
| team-docs | Technical writer | API docs, README updates |

**Named agents** so you can track who did what. Output files so results are permanent and reviewable.

## The Process

### Step 1: Analyze the Request

Break the feature into components by asking:
- What are the distinct layers? (db, api, ui, tests, docs)
- What files does each component touch? (must be disjoint sets)
- What does each component depend on from the others? (none = fully parallel)

**Example — "build the auth system":**

```
Components:
1. Auth models + migrations      → team-auth-models
2. Auth API endpoints            → team-auth-api
3. Auth middleware/guards         → team-auth-middleware
4. Auth tests                   → team-auth-tests

Why independent:
- Models don't need API endpoints to exist (just the schema)
- Middleware just needs the models + endpoint signatures
- Tests can be written against the same interfaces
- No circular dependencies
```

### Step 2: Write Agent Prompts

Each agent prompt must include:
1. **What to build** — specific, scoped, unambiguous
2. **Full context** — the plan, relevant code, file locations
3. **Output contract** — what file to write, what format
4. **Constraints** — what NOT to touch, branch rules

**Prompt template:**
```markdown
You are team-{component}, a backend engineer on the Harvey OS project.

## Your Task
Build the {component_name} for the {feature_name} feature.

## What to Build
{bullet-point specification}

## Context
- Feature plan: {plan_summary}
- Working directory: {project_root}
- Target branch: {branch_name}

## Output Contract
Write your implementation to: {output_file}
Summarize results (what you built, files changed, tests added) to: {summary_file}

## Constraints
- Do NOT touch files outside {component_scope}
- Do NOT commit — write results only
- Follow existing code patterns in {reference_files}
- If you need to make an assumption, note it in your summary

Start immediately. Report when complete.
```

### Step 3: Launch in Parallel

Launch all agents simultaneously using the `launch_parallel()` function from `orchestrate.py`. Each agent runs as a background subprocess with its own isolated context.

**Named agents** — use `team-{component}` naming so results are traceable.

### Step 4: Collect and Consolidate

Wait for all agents to complete using `wait_for_results()`. Read each output file, identify failures, and surface a unified summary:

```
Team Report — Auth System
========================
team-auth-models:     DONE (auth/models.py, migrations/)
team-auth-api:        DONE (auth/routes.py, 3 endpoints)
team-auth-middleware: DONE (auth/middleware.py, 2 guards)
team-auth-tests:      DONE (auth/test_suite.py, 12 tests)

Integration: Pass all 4 outputs to the finisher for commit + PR.
```

### Step 5: Fail Fast

If any agent fails hard:
1. Surface the failure immediately — don't wait for others
2. Log which task failed and why
3. Options: fix in this session, or retry the failed agent only

## Identifying Independent Tasks

### Rules for Independence

Two tasks are independent if:
- They write to different files (no file overlap)
- They don't need each other's output at build time
- They can be built in any order and produce the same final result

### Dependency Check

```
Task A and Task B are independent if:
  ✓ A's output is not B's input during build
  ✓ B's output is not A's input during build
  ✓ Neither reads the other's intermediate state

Task A must wait for Task B if:
  ✗ A needs B's compiled output to build
  ✗ A needs B's generated code to compile
  ✗ A's tests depend on B's runtime behavior
```

### Common Parallelization Patterns

| Feature Type | Typical Components | Parallelizable? |
|--------------|-------------------|------------------|
| Auth system | models, api, middleware, tests, docs | Yes — all independent |
| API endpoint | schema, route, validation, tests, docs | Yes — all independent |
| UI component | component, styles, tests, story | Yes — all independent |
| Full-stack feature | backend, frontend, integration tests | Partial — backend+frontend parallel, tests sequential |
| Database migration | migration, rollback, tests | Sequential — must run in order |

### Red Flags (Don't Parallelize)

- "Component A generates code that component B consumes" → sequential
- "Component B's tests need component A running" → sequential
- "Both components edit the same file" → sequential
- "Component A is a library used by component B" → B depends on A

## Output File Convention

Each agent writes to a designated output file. Convention:

```
data/dev-orchestrator/
  {feature_name}/
    {task_name}.output.md     # Full implementation details
    {task_name}.summary.md    # Short status + files changed
    plan.md                   # The original plan
    sprint.json               # Task metadata + results
```

## Integration with Other Skills

**Use with:**
- `plan` — Create the implementation plan first, then Orchestrate executes it
- `ship` — After consolidation, use ship to commit and PR
- `guard` — Run in guard mode for destructive commands
- `review` — Run final code review before shipping

**Orchestration flow:**
```
plan → dev-orchestrator (parallel build) → ship (commit + PR)
```

## Key Principles

1. **Parallel by default** — if tasks don't depend, run them in parallel
2. **One agent = one person** — treat each agent as a team member
3. **Named agents** — "team-auth", "team-frontend" — so you can track who did what
4. **Output files** — each agent writes to a file, consolidate after
5. **Fail fast** — if one fails hard, surface immediately
6. **Don't over-parallelize** — 4-8 agents is optimal; more creates coordination overhead

## Anti-Patterns

**❌ Too many agents:** "Launch 20 agents for 20 files" — creates overhead
**❌ Not truly independent:** Agents stepping on each other's files
**❌ No output contract:** Agent builds something, you don't know what
**❌ Sequential with parallel dispatch:** Launching agents but waiting for each one before launching next
**❌ No fail-fast:** Waiting for all agents when one has clearly failed

## Running the Orchestration

The `orchestrate.py` script provides the tooling:

```bash
# Basic usage
python3 harvey-os/skills/dev/dev-orchestrator/orchestrate.py \
  --feature "auth system" \
  --plan-file /path/to/plan.md \
  --output-dir data/dev-orchestrator/auth-system

# With task specification
python3 harvey-os/skills/dev/dev-orchestrator/orchestrate.py \
  --feature "checkout flow" \
  --tasks-config tasks.json \
  --output-dir data/dev-orchestrator/checkout
```

Or import the functions directly:

```python
from orchestrate import plan_tasks, launch_parallel, wait_for_results, run_development_sprint

tasks = plan_tasks("build the auth system with models, API, middleware, and tests")
agent_ids = launch_parallel(tasks)
results = wait_for_results(agent_ids)
```
