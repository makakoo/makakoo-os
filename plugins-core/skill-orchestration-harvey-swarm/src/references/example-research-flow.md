# Example: research → store → celebrate

User says:

> "Investigate the latest diffusion transformer papers and tell me
> what's worth reading."

## What you do

### 1. Plan first (optional but cheap)

```
mcp__harvey__harvey_swarm_run({
  request: "Investigate the latest diffusion transformer papers and tell me what's worth reading",
  plan_only: true
})
```

Returns something like:

```json
{
  "mode": "plan_only",
  "classification": {
    "intent": "research",
    "confidence": 0.857,
    "keywords_hit": ["research", "investigate"]
  },
  "team": {
    "name": "research_team",
    "parallelism": 3,
    "members": [
      {"agent": "researcher", "action": "search_all", "role": "parallel_researcher", "count": 3},
      {"agent": "synthesizer", "action": "combine", "role": "synthesizer", "count": 1},
      {"agent": "storage", "action": "save", "role": "storage", "count": 1}
    ]
  },
  "workflow": {
    "id": "wf_abc123",
    "step_count": 5
  }
}
```

Tell the user in one line: "Routing to research_team — 3 parallel
researchers + synthesizer + storage. Running now."

### 2. Execute

```
mcp__harvey__harvey_swarm_run({
  request: "Investigate the latest diffusion transformer papers and tell me what's worth reading",
  timeout_s: 180
})
```

Returns:

```json
{
  "mode": "executed",
  "workflow_id": "wf_abc123",
  "workflow_state": "WorkflowState.COMPLETED",
  "artifacts": {
    "parallel_researcher_1": {...},
    "parallel_researcher_2": {...},
    "parallel_researcher_3": {...},
    "synthesizer": {"summary": "...", "input_count": 3},
    "storage": {"saved_to": "..."}
  }
}
```

### 3. Show the synthesizer artifact to the user

The synthesizer step's payload is what the user actually wants. Pull
its `summary` field and present it.

### 4. Log + celebrate

```
mcp__harvey__harvey_journal_entry({
  summary: "Ran swarm on diffusion transformer papers — 3 standout reads in synthesizer artifact wf_abc123:synthesizer",
  tags: ["DiffusionTransformers", "Research"]
})

mcp__harvey__harvey_olibia_speak({
  message: "diffusion transformer research complete",
  tone: "celebrate"
})
```

Done.
