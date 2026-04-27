# ABI: SANCHO-Task — v0.1

**Status:** v0.1 LOCKED — 2026-04-15
**Kind:** `sancho-task` (or any plugin kind declaring `[sancho.tasks]`)
**Owner:** Makakoo kernel, `crates/core/src/sancho/`
**Promotes to v1.0:** after Phase E dogfooding

---

## 0. What a SANCHO task is

A **SANCHO task** is a scheduled, gate-guarded unit of work. The SANCHO
scheduler ticks on an interval, checks each task's gates, and runs the
task if all gates pass. Examples: `gym_classify` (hourly), `dream`
(every 4h), `pg_watchdog` (every 15m), `daily_briefing` (8h + active
hours 7-22), `arbitrage_tick` (5m + 6-23 active hours).

Tasks can live in a dedicated `kind = "sancho-task"` plugin OR be
declared by any agent/skill plugin via `[sancho.tasks]`. Either way,
the contract is the same.

## 1. Contract

A SANCHO task is declared in a plugin manifest:

```toml
[sancho]
tasks = [
  { name = "gym_classify",       interval = "1h",    active_hours = [0, 24] },
  { name = "gym_hypothesize",    interval = "23.5h", active_hours = [1, 4], gates = ["session", "lock"] },
  { name = "arbitrage_tick",     interval = "5m",    active_hours = [6, 23] },
]
```

The plugin's `[entrypoint].run` command is invoked with `--task <name>`
when SANCHO decides the task should fire.

## 2. Task declaration fields

| Field | Required | Type | Meaning |
|---|---|---|---|
| `name` | yes | string | Unique task name across all plugins + native kernel tasks |
| `interval` | yes | string | Minimum duration between runs (`"5m"`, `"300s"`, `"23.5h"`, `"7d"`) |
| `active_hours` | no | `[start, end]` | Local-time hour range. Default `[0, 24]` = always |
| `weekdays` | no | array | `["mon", "tue", ...]` to restrict to specific days |
| `gates` | no | array | Additional gates: `"session"`, `"lock"`, `"idle"` |
| `min_battery_pct` | no | number | Don't run if battery below this % (laptops) |
| `network_required` | no | bool | Don't run if offline |

**Naming rules:** lowercase, digits, underscores. Must be unique across
all installed plugins. Collisions are refused at plugin install time.

## 3. Interval format

- `Ns` — N seconds
- `Nm` — N minutes
- `Nh` — N hours (supports decimals: `23.5h`)
- `Nd` — N days

Interval is a **minimum** between runs, not a fixed schedule. A task
with `interval = "1h"` runs no more than once per hour, but may run
less often if other gates prevent it.

## 4. Gates

### 4.1 TimeGate (implicit)

Every task has an implicit TimeGate from `interval`. The task can fire
only if the time since its last successful run exceeds the interval.

### 4.2 ActiveHoursGate (from `active_hours`)

Task fires only if the local time is within `[start, end]`. `[7, 22]`
means 7am to 10pm local. Wrap-around (`[22, 6]` = 10pm to 6am next day)
is supported.

### 4.3 WeekdayGate (from `weekdays`)

Task fires only on listed weekdays.

### 4.4 SessionGate

Fires only if there's no active user session (user is away from
keyboard). Used for tasks that shouldn't disturb work: `dream`,
`gym_hypothesize`, heavy computations.

Detection:
- macOS: idle time from `ioreg -c IOHIDSystem | grep HIDIdleTime`
- Linux: X/Wayland idle time from `xprintidle` or `swayidle`
- Windows: `GetLastInputInfo`
- Fallback: always pass (no session detection available)

### 4.5 LockGate

Fires only if the screen is locked. Complements SessionGate for tasks
that should only run when the user is definitely away.

### 4.6 IdleGate

Fires only if CPU load is below a threshold (default 20%). Used for
low-priority maintenance work.

### 4.7 BatteryGate (from `min_battery_pct`)

On laptops, skips the task if battery is below the threshold. Desktop
systems always pass.

### 4.8 NetworkGate (from `network_required`)

Skips if no network. Set `network_required = true` for tasks like
`hackernews_monitor` that need external API access.

## 5. Invocation contract

When SANCHO decides a task should fire:

1. Kernel spawns the plugin's `[entrypoint].run` command with args
   `--task <task-name>`
2. Environment: same as skills (SOCKET_PATH, PLUGIN_NAME, etc.)
3. Timeout: default 5 minutes (configurable per task via
   `[sancho.tasks] ... timeout = "10m"`)
4. Stdout: captured, parsed as JSON (see §6)
5. Stderr: captured to
   `$MAKAKOO_HOME/logs/sancho/<task-name>/<timestamp>.stderr`
6. Exit 0: success
7. Non-zero exit: failure; task marked failed, next run scheduled after
   backoff

## 6. Output format

The task's stdout must be a single JSON object on success:

```json
{
  "status": "ok",
  "duration_ms": 1234,
  "items_processed": 42,
  "next_run": "2026-04-15T18:42:00Z"
}
```

**Required fields:**
- `status`: `"ok"` | `"partial"` | `"skipped"`

**Optional fields:**
- `duration_ms`: self-reported duration
- `message`: human-readable one-liner (shown in `makakoo sancho status`)
- `next_run`: override the default next-run time (ignored if in the past)
- Arbitrary task-specific metrics

The kernel parses this, logs to
`$MAKAKOO_HOME/logs/sancho/runs.jsonl`, and displays the message in
`makakoo sancho status`.

**Non-JSON stdout:** kernel accepts it as the `message` field with
`status = "ok"` if exit code is 0. This lets legacy Python tasks that
just `print("done")` work without refactoring.

## 7. Scheduling behavior

The SANCHO scheduler ticks every 60 seconds. On each tick:

1. Walk the registered task list
2. For each task, check all gates in order
3. If all gates pass, add to the run queue
4. Run queued tasks sequentially (not parallel — avoids capability
   socket contention)
5. After each task, update its last-run timestamp

**Parallel execution:** not supported in v0.1. All tasks run sequentially
on a single worker. v0.2 may add a per-plugin parallel worker if a task
spawns long-running subshells.

## 8. Failure handling

- First failure: log, schedule retry at interval
- Second consecutive failure: log, double the next interval
- Third consecutive failure: mark task as degraded (visible in `makakoo
  sancho status`)
- After 5 consecutive failures: disable task, user must `makakoo sancho
  enable <name>` to resume
- After 10 consecutive failures across different intervals: mark plugin
  as quarantined (same as agent crash quarantine)

## 9. Task unregistration

When a plugin is uninstalled, its tasks are unregistered from the
scheduler atomically. A task in the middle of running is allowed to
complete; new runs are not scheduled.

## 10. Forbidden for tasks at v0.1

- **Interactive input.** Tasks run headless; stdin is closed. Tasks
  that need user input should be skills, not SANCHO tasks.
- **Long-running (> 10 min).** Tasks that take longer should be
  refactored into an agent with its own internal loop.
- **Mutual dependencies.** Task A depending on task B running first is
  not supported in v0.1. Order is implicit from registration order.
  v0.2 may add explicit DAG deps.

## 11. Versioning

Same semver rules.

## 12. Example: `gym_classify`

**Plugin:** `plugins-core/gym/plugin.toml` declares:

```toml
[sancho]
tasks = [
  { name = "gym_classify", interval = "1h", active_hours = [0, 24] },
]
```

**Entrypoint (partial):**
```python
# gym/run.py
import sys, json
from gym.classifier import run_classifier

def main():
    if sys.argv[1:] == ["--task", "gym_classify"]:
        result = run_classifier()
        print(json.dumps({
            "status": "ok",
            "items_processed": result.count,
            "message": f"classified {result.count} errors",
        }))
        return 0
    return 2  # unknown task

if __name__ == "__main__":
    sys.exit(main())
```

**Invocation:** SANCHO scheduler ticks, gym_classify's TimeGate shows
last run was 1h ago, no other gates configured, fires the task.

**Output seen in `makakoo sancho status`:**
```
gym_classify:   last=2026-04-15T17:32:11Z  next=~18:32  status=ok
                "classified 3 errors" (11ms)
```

---

**Status:** v0.1 LOCKED.
