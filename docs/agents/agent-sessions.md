# Agent Sessions v1

Agent Sessions are Makakoo OS' durable sub-agent primitive. They give parent agents a small context surface while preserving complete child-agent output, gate logs, and status history in `$MAKAKOO_HOME/data/agent_sessions.db`.

This release is deliberately **sync-first**. It does not pretend to launch detached daemon workers yet. The CLI creates durable sessions, completes deterministic sync tasks, stores full results behind handles, and records verification gates. Detached swarm workers can build on the same schema later.

## Commands

```bash
makakoo agent-session open \
  --name repo-audit \
  --role explore \
  --task "Inspect plugin install flow" \
  --workspace /path/to/repo \
  --json

makakoo agent-session eval repo-audit --wait --json
makakoo agent-session read repo-audit --section EVIDENCE
makakoo agent-session gate repo-audit --name unit-tests --cwd /path/to/repo --cmd 'cargo test -p makakoo-core agent_session'
makakoo agent-session gates repo-audit --json
makakoo agent-session close repo-audit --json
```

Handle reads are global:

```bash
makakoo handle read agent-artifact://asa_... --summary
makakoo handle read agent-artifact://asa_... --section EVIDENCE
makakoo handle read agent-artifact://asa_... --head 40
makakoo handle read agent-artifact://asa_... --tail 80
makakoo handle read agent-artifact://asa_... --jsonpath $.summary
```

## Storage model

Database path:

```text
$MAKAKOO_HOME/data/agent_sessions.db
```

Tables:

- `agent_sessions` — identity, role, status, assignment, workspace, handles, metadata.
- `agent_session_events` — append-only lifecycle log.
- `agent_session_artifacts` — full payload storage addressed by `agent-artifact://<id>`.
- `agent_session_gates` — command, cwd, exit code, duration, classification, full log handle.
- `agent_session_items` — future task/result subrecords for detached execution.

## Design invariants

- Parent context gets summaries and handles, not raw megabyte transcripts.
- Every result that matters is retrievable after CLI restart.
- Duplicate active session names are rejected by a SQLite partial unique index while sessions are `queued` or `running`.
- Session names are labels, not paths; slashes, control characters, `.` and `..` are rejected.
- Gates keep full stdout/stderr behind a handle and return non-zero when the checked command fails.
- Event bus publishing is best-effort. SQLite is the source of truth.
- `makakoo agent` remains plugin lifecycle management; `makakoo agent-session` is the orchestration primitive.

## Current limits

- `eval` is sync deterministic scaffolding, not a detached LLM worker.
- `--timeout` is reserved in CLI shape but not meaningful until daemon execution lands.
- `agent-session eval --message` overrides the original assignment for the sync evaluation result.
- JSONPath support is intentionally small: root objects plus dotted keys and integer array indexes.

## Next slice

The next implementation should attach this store to real worker execution:

1. Spawn worker process/task with session id and owner token.
2. Stream stdout/stderr into transcript artifacts.
3. Update `running` heartbeats and detect stale sessions.
4. Add `makakoo agent-session join` for blocking on detached workers.
5. Add MCP tools for open/eval/read/gate so Claude/OpenCode/Lope can use the same primitive.
