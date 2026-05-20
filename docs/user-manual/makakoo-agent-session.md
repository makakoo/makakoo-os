# `makakoo agent-session` — durable child-agent sessions

`makakoo agent-session` is the sync-first orchestration primitive for child-agent work. It lets a parent agent keep context small while full child-agent output, result artifacts, and verification gate logs stay durable on disk.

Use it when you want to hand off a bounded task, preserve the evidence, and read back only the slice you need instead of pasting a whole transcript into the parent context.

## Storage

Agent sessions write to:

```text
$MAKAKOO_HOME/data/agent_sessions.db
```

Result and gate logs are stored as artifacts and addressed as:

```text
agent-artifact://<artifact-id>
```

The companion [`makakoo handle`](makakoo-handle.md) command reads those handles with bounded output modes.

## Commands

| Command | Purpose |
|---|---|
| `makakoo agent-session open` | Create a durable queued session. |
| `makakoo agent-session list` | List recent sessions. |
| `makakoo agent-session status <name-or-id>` | Show one session. |
| `makakoo agent-session eval <name-or-id>` | Complete the sync v1 session and write a result handle. |
| `makakoo agent-session read <name-or-id>` | Read the session result/transcript handle. |
| `makakoo agent-session gate <name-or-id>` | Run a command, persist stdout/stderr, attach pass/fail gate. |
| `makakoo agent-session gates <name-or-id>` | List gates for a session. |
| `makakoo agent-session close <name-or-id>` | Archive/close a session. |

## Basic flow

```bash
makakoo agent-session open \
  --name repo-audit \
  --role explore \
  --task "Inspect plugin install flow" \
  --workspace /path/to/repo

makakoo agent-session eval repo-audit --wait
makakoo agent-session read repo-audit --section EVIDENCE
makakoo agent-session gate repo-audit \
  --name unit-tests \
  --cwd /path/to/repo \
  --cmd 'cargo test -p makakoo-core agent_session'
makakoo agent-session gates repo-audit
makakoo agent-session close repo-audit
```

Use `--json` on every subcommand when another program or AI agent consumes the output.

## `open`

```bash
makakoo agent-session open \
  --name <label> \
  --role general|explore|plan|review|implementer|verifier|custom \
  --task <text> \
  --workspace <existing-dir> \
  [--model <model>] \
  [--json]
```

Rules:

- `--workspace` must already exist and is canonicalized before storing.
- Session names are labels, not paths: slashes, control characters, `.`, and `..` are rejected.
- Only one active (`queued` or `running`) session may use a given name at a time.

## `eval`

```bash
makakoo agent-session eval <name-or-id> --wait [--message <override>] [--json]
```

v1 is intentionally sync-first. `eval` writes a deterministic result artifact and marks the session complete. It does **not** claim detached daemon execution yet. `--message` overrides the original assignment for this sync result.

Detached workers, heartbeats, and `join` are the next slice.

## `read`

```bash
makakoo agent-session read <name-or-id> [--section NAME|--head N|--tail N] [--json]
```

Reads the session result handle if present, otherwise the transcript handle. Use this when you know the session but do not want to manually copy the `agent-artifact://...` handle.

## `gate`

```bash
makakoo agent-session gate <name-or-id> \
  --name <gate-name> \
  --cwd <existing-dir> \
  --cmd '<shell-command>' \
  [--json]
```

The command runs under `/bin/sh -c` in `--cwd`. Makakoo stores the full command log as an artifact, records exit code and duration, and returns exit code `0` for pass / `1` for fail.

Human output:

```text
pass unit-tests exit=0 log=agent-artifact://asa_...
```

JSON output contains `classification`, `exit_code`, `duration_ms`, `summary`, and `log_artifact_id`.

## Design boundaries

- Parent context gets summaries and handles, not megabyte transcripts.
- SQLite is the source of truth; event bus publishing is best-effort.
- v1 is local and sync-first. It prepares the durable substrate for real detached swarm workers but does not launch them yet.
