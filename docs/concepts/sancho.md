# SANCHO Guide

Proactive task engine that runs while you sleep.

## What is SANCHO?

SANCHO is Makakoo's scheduler for automated tasks:

- **Dream:** LLM reflection on recent work
- **Daily Briefing:** Morning summary to you
- **Memory Cleanup:** Brain hygiene
- **Plugin Tasks:** Custom scheduled jobs

```
┌─────────────────────────────────────────────────────────────┐
│                      SANCHO                                  │
│                   (Task Scheduler)                           │
│                                                             │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌──────────┐    │
│  │ dream   │  │ briefing │  │  lint   │  │ plugins  │    │
│  │ every 4h│  │ daily   │  │ daily   │  │ every Xm  │    │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────┬─────┘    │
│       │            │            │            │           │
│       └────────────┴────────────┴────────────┘           │
│                         │                                │
│                         ▼                                │
│              ┌─────────────────────┐                     │
│              │       GATES         │                     │
│              │  Time │ Active │   │                     │
│              │  Hours │ Idle │    │                     │
│              └─────────────────────┘                     │
└─────────────────────────────────────────────────────────────┘
```

## Viewing Tasks

### List All Tasks

```bash
makakoo sancho status
```

Output:
```
Registered tasks: 12 (8 native + 4 manifest)

Native Tasks:
  ✓ dream              (4h, last: 2h ago)
  ✓ wiki_lint          (daily, last: 6h ago)
  ✓ daily_briefing     (8h, last: 1h ago)
  ✓ memory_consolidation (daily, last: 5h ago)
  ✓ superbrain_sync    (1h, last: 30m ago)
  ✓ dynamic_checklist  (6h, last: 3h ago)
  ✓ gym_classify       (1h, last: 45m ago)
  ✓ pg_watchdog        (15m, last: 5m ago)

Manifest Tasks:
  ✓ arbitrage_tick     (5m, active_hours: 6-23)
  ✓ hackernews_monitor (30m)
  ✓ gym_hypothesize    (23.5h, idle required)
  ✓ watchdog_switchailocal (5m)
```

### Task Details

```bash
makakoo sancho status --task dream
```

Output:
```
Task: dream
Type: native
Interval: 4h
Last run: 2026-04-20 08:00:00
Next run: 2026-04-20 12:00:00
Active hours: [1, 4]
Gates: [session]
Status: ✓ healthy
```

## Running Tasks Manually

### Run a Single Task

```bash
makakoo sancho run dream
```

### Run All Due Tasks

```bash
makakoo sancho run --all-due
```

### Force Run (Ignore Gates)

```bash
makakoo sancho run dream --force
```

## Task History

```bash
# View recent runs
makakoo sancho history

# View specific task
makakoo sancho history --task dream

# Filter by status
makakoo sancho history --status failed

# Limit results
makakoo sancho history --limit 50
```

Output:
```
2026-04-20 08:00:00  dream         ✓ ok (1.2s)
2026-04-20 07:00:00  gym_classify  ✓ ok (0.8s)
2026-04-20 06:00:00  wiki_lint     ✓ ok (5.1s)
2026-04-20 05:00:00  memory_consol ✓ ok (3.2s)
2026-04-20 04:00:00  dream         ✓ ok (1.4s)
2026-04-20 03:00:00  gym_classify  ✓ ok (0.9s)
2026-04-20 02:00:00  superbrain    ✓ ok (2.1s)
2026-04-20 01:00:00  gym_hypothesize  ✓ ok (45s)
```

## Gates System

Tasks only run when all gates pass:

### Time Gate

Every task has a minimum interval:

| Task | Interval | Meaning |
|------|----------|---------|
| `dream` | 4h | At least 4 hours between runs |
| `wiki_lint` | daily | Once per day |
| `pg_watchdog` | 15m | Every 15 minutes |

### Active Hours Gate

Some tasks only run during certain hours:

```bash
# Check active hours
makakoo sancho status --task arbitrage_tick
```

Output:
```
Task: arbitrage_tick
Interval: 5m
Active hours: 6-23 (local time)
Current: 14:32 ✓ within window
```

### Session Gate

Tasks with `session` gate only run when you're away:

```
gym_hypothesize: runs only when no active terminal session
```

### Lock Gate

Tasks with `lock` gate only run when screen is locked:

```
dream: runs only when screen is locked (user definitely away)
```

## Native Tasks

### dream

LLM reflection on recent decisions.

```bash
makakoo sancho status --task dream
```

**What it does:**
1. Reads recent journal entries
2. Asks LLM to reflect on patterns
3. Writes insights to Brain

**When:** Every 4 hours, active hours 1-4

---

### daily_briefing

Morning summary.

```bash
makakoo sancho status --task daily_briefing
```

**What it does:**
1. Summarizes yesterday's work
2. Identifies pending tasks
3. Sends to Telegram (if configured)

**When:** Every 8 hours, active hours 7-9

---

### wiki_lint

Brain hygiene.

```bash
makakoo sancho status --task wiki_lint
```

**What it does:**
1. Checks for broken wikilinks
2. Fixes orphaned references
3. Reports issues

**When:** Daily

---

### memory_consolidation

Optimize Brain storage.

```bash
makakoo sancho status --task memory_consolidation
```

**What it does:**
1. Removes duplicates
2. Optimizes indexes
3. Cleans old temp files

**When:** Daily

---

### superbrain_sync

Sync Brain to vector database.

```bash
makakoo sancho status --task superbrain_sync
```

**What it does:**
1. Indexes new journal entries
2. Updates vector embeddings
3. Optimizes search index

**When:** Hourly

---

### dynamic_checklist

Process HEARTBEAT.md tasks.

```bash
makakoo sancho status --task dynamic_checklist
```

**What it does:**
1. Reads `HEARTBEAT.md` in project
2. Checks each task for completion
3. Notifies if task is due

**When:** Every 6 hours

---

### gym_classify

Error classification.

```bash
makakoo sancho status --task gym_classify
```

**What it does:**
1. Reads error logs
2. Classifies by pattern
3. Updates error dashboard

**When:** Hourly

---

### pg_watchdog

PostgreSQL health check.

```bash
makakoo sancho status --task pg_watchdog
```

**What it does:**
1. Checks PG connections
2. Reports slow queries
3. Alerts on issues

**When:** Every 15 minutes

## Plugin Tasks

Plugins register their own tasks:

```toml
# plugin.toml
[sancho]
tasks = [
  { name = "arbitrage_tick", interval = "5m", active_hours = [6, 23] },
  { name = "hackernews_monitor", interval = "30m" },
]
```

### arbitrage_tick

Polymarket trading signal generation.

```bash
makakoo sancho status --task arbitrage_tick
```

**When:** Every 5 minutes, 6am-11pm

---

### hackernews_monitor

Monitor Hacker News for keywords.

```bash
makakoo sancho status --task hackernews_monitor
```

**When:** Every 30 minutes

## Custom Tasks

### Create HEARTBEAT.md Task

Create a task file:

```bash
cat > ~/projects/my-app/HEARTBEAT.md << 'EOF'
# Heartbeat Tasks

## Weekly Review
- [ ] Review PRs
- [ ] Update roadmap
- [ ] Check metrics
Schedule: every monday 09:00

## Deploy Check
- [ ] Check production health
- [ ] Review error rates
Schedule: every day 18:00
EOF
```

SANCHO reads this and reminds you when tasks are due.

## Configuration

### Disable a Task

```bash
# Via plugin disable
makakoo plugin disable watchdog-postgres

# Manual override
touch ~/.makakoo/sancho/disabled/dream
```

### Adjust Interval

Edit `~/.makakoo/config/sancho.toml`:

```toml
[tasks.dream]
interval = "2h"  # Override default 4h
active_hours = [0, 24]  # Run anytime
```

### Pause SANCHO

```bash
# Pause all tasks
makakoo sancho pause

# Resume
makakoo sancho resume
```

## Troubleshooting

### Task Not Running

```bash
# Check gates
makakoo sancho status --task <name>

# Check history
makakoo sancho history --task <name>

# Force run
makakoo sancho run <name> --force
```

### Too Many Tasks

```bash
# See what's running
makakoo sancho status

# Disable unnecessary
makakoo plugin disable <plugin>
```

### Task Failed

```bash
# View failure logs
cat ~/.makakoo/logs/sancho/<task>/error.log

# Run with verbose
makakoo sancho run <task> --verbose
```

## Interval Format

| Format | Example | Meaning |
|--------|---------|---------|
| `Nm` | `5m` | N minutes |
| `Nh` | `2h` | N hours |
| `Ns` | `300s` | N seconds |
| `Nd` | `1d` | N days |

## See Also

- [Concepts Overview](./index.md) — SANCHO in context
- [Plugin Guide](../plugins/index.md) — Plugin task registration
- [CLI Reference](../user-manual/makakoo-sancho.md) — Full command reference
