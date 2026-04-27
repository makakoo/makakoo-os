# Agent Orchestration System — Operating Procedures

**Skill:** `orchestrator` (Harvey OS Infrastructure)

## Overview

The orchestrator is a task orchestration layer that:
1. Splits complex tasks into executable sub-tasks
2. Routes sub-tasks to specialized agents (varying LLM capabilities)
3. Provides async message-passing between agents
4. Aggregates results with dependency resolution
5. Handles DAG-based task graphs with error propagation

## Architecture

```
data/orchestrator/
├── queues/
│   ├── incoming/          # New tasks land here
│   ├── running/           # Currently executing
│   ├── completed/         # Finished successfully
│   ├── failed/            # Failed tasks
│   └── blocked/           # Waiting on dependencies
├── messages/              # Inter-agent message boxes
│   └── {agent_id}/
│       ├── inbox/         # Messages received
│       └── outbox/        # Messages to send
├── state/
│   └── task_graph.json    # DAG state
└── config/
    └── routing_rules.json # Model routing config
```

## Usage

### Starting the Orchestrator

**Daemon management (preferred):**
```bash
# Start
/usr/local/opt/python@3.11/bin/python3.11 $HARVEY_HOME/harvey-os/skills/infrastructure/orchestrator/daemon.py start

# Stop
/usr/local/opt/python@3.11/bin/python3.11 $HARVEY_HOME/harvey-os/skills/infrastructure/orchestrator/daemon.py stop

# Status
/usr/local/opt/python@3.11/bin/python3.11 $HARVEY_HOME/harvey-os/skills/infrastructure/orchestrator/daemon.py status

# Restart
/usr/local/opt/python@3.11/bin/python3.11 $HARVEY_HOME/harvey-os/skills/infrastructure/orchestrator/daemon.py restart
```

**Direct run (development):**
```bash
python3 $HARVEY_HOME/harvey-os/skills/infrastructure/orchestrator/controller.py
```

Runs as a long-lived daemon process. Use Ctrl+C or send SIGTERM for graceful shutdown.
Logs: `$HARVEY_HOME/data/logs/orchestrator.log`

### Submitting Tasks

```python
import sys
sys.path.insert(0, "$HARVEY_HOME/harvey-os/skills/infrastructure/orchestrator")
from controller import Orchestrator

orch = Orchestrator()
task_id = orch.submit({
    "description": "Analyze codebase for security issues",
    "agent_type": "code-review",
    "priority": 8,
    "dependencies": [],
    "payload": {
        "instructions": "Review the harvey-os codebase for security vulnerabilities...",
        "context": {"repo": "harvey-os"}
    }
})
```

### Task Schema

```json
{
  "task_id": "uuid-v4",
  "parent_id": "uuid-v4 | null",
  "description": "Human-readable task description",
  "agent_type": "gsd-executor | gsd-planner | codex | general-purpose",
  "model": "minimax:M2 | claude-sonnet-4-20250514",
  "priority": 1-10,
  "dependencies": ["task_id_1", "task_id_2"],
  "payload": {
    "instructions": "...",
    "context": {}
  },
  "status": "pending | running | completed | failed | blocked",
  "result": null,
  "created_at": "ISO8601"
}
```

### Routing Rules

The router selects models based on task characteristics. Edit `config/routing_rules.json` to customize:

```json
{
  "rules": [
    {"match": {"complexity": "low"}, "model": "minimax:M2", "endpoint": "http://localhost:18080"},
    {"match": {"complexity": "high"}, "model": "claude-sonnet-4-20250514", "endpoint": "anthropic"}
  ],
  "default": {"model": "minimax:M2", "endpoint": "http://localhost:18080"}
}
```

## Components

### TaskQueue (`task_queue.py`)
File-based priority queue with atomic operations.

```python
queue = TaskQueue()
queue.enqueue(task)        # Add to incoming
queue.dequeue()            # Pop highest priority
queue.complete(task_id)    # Move to completed
queue.fail(task_id, error) # Move to failed
queue.block(task_id)      # Move to blocked
```

### TaskGraph (`task_graph.py`)
DAG for dependency management.

```python
graph = TaskGraph()
graph.add_task(task)              # Add node + edges
graph.get_runnable()             # Tasks with deps satisfied
graph.topological_sort()         # Kahn's algorithm
graph.notify_completed(task_id)  # Unblock dependents
graph.notify_failed(task_id)     # Propagate to dependents
```

### MessageBus (`message_bus.py`)
Inter-agent messaging.

```python
bus = MessageBus()
bus.send(to_agent, "result", {"task_id": "...", "result": {...}})
msg = bus.receive(agent_id, timeout=30)
bus.broadcast("shutdown", {})
```

### Router (`router.py`)
Model routing based on task characteristics.

```python
router = Router()
model, endpoint = router.route(task)
```

### LLM Gateway (`llm_gateway.py`)
LLM client with fallback support.

```python
response = call_llm(prompt, model="minimax:M2", endpoint="http://localhost:18080")
response = call_llm_anthropic(prompt, model="claude-sonnet-4-20250514")
```

## Dependencies

- Python 3.8+
- `requests` library

## Error Handling

| Error Type | Response |
|------------|----------|
| Agent crash | Mark failed, propagate to dependents |
| Timeout | Mark failed, propagate to dependents |
| Model API error | Retry with fallback model |
| Dependency failed | Mark blocked tasks as failed |

## Monitoring

Check queue states:

```bash
ls -la $HARVEY_HOME/data/orchestrator/queues/incoming/
ls -la $HARVEY_HOME/data/orchestrator/queues/running/
ls -la $HARVEY_HOME/data/orchestrator/queues/completed/
```

Check task graph:

```bash
cat $HARVEY_HOME/data/orchestrator/state/task_graph.json
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `HARVEY_TASK_ID` | Current task being executed |
| `HARVEY_AGENT_ID` | Current agent identifier |
| `HARVEY_AGENT_TYPE` | Agent type (general-purpose, codex, etc.) |
| `HARVEY_MODEL` | Model to use |
| `HARVEY_ENDPOINT` | LLM gateway endpoint |
| `LLM_API_KEY` | API key for LLM gateway |
| `ANTHROPIC_API_KEY` | API key for Anthropic |
