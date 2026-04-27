# System Health Dashboard — "Is Harvey healthy?"

## Overview

Real-time monitoring dashboard for Harvey services and system resources with auto-refresh capability.

## Features

- **Harvey services status**: switchAILocal, and optional services (Qdrant, PostgreSQL, Logseq)
- **System resource monitoring**: CPU, memory, disk usage with progress bars
- **Color-coded indicators**: ✅ running / ❌ stopped
- **Interactive refresh**: Press 'r' to refresh, 'q' to quit
- **Auto-detect**: Only monitors services that are configured

## Requirements

- Python 3.8+
- Core module: http, socket, subprocess
- Optional: `psutil` for advanced system monitoring (`pip install psutil`)
- Gracefully degrades without psutil

## Data Volume

Health logs are written to: `$HARVEY_HOME/data/system-health/`
- Health snapshots: `health_YYYY_MM_DD.json` (hourly snapshots)
- Service history: `service_availability.log`

## Execution

```bash
# Run dashboard
python3 -m skills.system.health.health_dashboard

# Or direct invocation
cd $HARVEY_HOME/harvey-os
python3 -c "from skills.system.health.health_dashboard import health_dashboard_main; health_dashboard_main()"
```

## Integration

Can be called programmatically:

```python
from skills.system.health.health_dashboard import HealthMonitor

monitor = HealthMonitor()
monitor.display_dashboard()  # One-time display
# or
monitor.interactive_dashboard()  # Interactive with refresh
```

## Example Output

```
╔════════════════════════════════════════╗
║  System Health                          ║
║  Harvey Services & Resources            ║
╚════════════════════════════════════════╝

Checking services...
[████████░░░░░░░░░░░░] 20%

=== Harvey Services ===

✅ switchAILocal                  [success]
✅ Qdrant                         [success]
❌ PostgreSQL                     [error]
✅ Logseq (optional)               [success]

=== System Resources ===

CPU:    [████████░░░░░░░░░░░░] 45.2%
Memory: [██████████████░░░░░░░] 62.3%
Disk:   [██████████████████░░░] 78.5%

Options:
  r - Refresh | q - Quit

Choice (r/q):
```

## Service Checks

### HTTP Services
Health check via HTTP endpoint:
- switchAILocal: `http://localhost:18080/health`
- Qdrant: `http://localhost:6333/health`
- Logseq (optional): `http://127.0.0.1:12315/version`

### PostgreSQL
Socket connection check to `localhost:5434`

### System Resources

- **CPU**: Average CPU usage over last check interval
- **Memory**: Percentage of RAM in use
- **Disk**: Percentage of root filesystem used
- **Available**: Absolute available memory/disk space

## Monitoring Intervals

- **Service checks**: 2-second timeout per service
- **System resources**: Sampled every 500ms for CPU
- **Refresh rate**: User-triggered or automatic (if integrated with scheduler)

## Integration with Watchdogs

This dashboard complements the existing watchdogs:
- `core/watchdogs/switchailocal_watchdog.py` — auto-restarts switchAILocal if dead
- `core/watchdogs/harveychat_watchdog.py` — auto-restarts HarveyChat if dead

The health dashboard provides visibility into what the watchdogs are monitoring.

## Logging

Health snapshots are logged periodically:

```markdown
- [[System Health]] snapshot: 2026-04-10 12:34:56
  - switchAILocal: ✅ running
  - Qdrant: ✅ running
  - PostgreSQL: ❌ stopped
  - Logseq (optional): ✅ running
  - CPU: 45.2%
  - Memory: 62.3%
  - Disk: 78.5%
```

## Troubleshooting

**psutil not installed**: Dashboard will show "N/A" for system resources.
```bash
pip install psutil
```

**Services showing as stopped when they're actually running**: Check:
1. Correct host/port in SERVICES config
2. Firewall isn't blocking the check
3. Service is actually listening on the configured port

**PostgreSQL shows stopped**: 
- Ensure PostgreSQL is running on port 5434
- Check POSTGRES_PORT in ~/.env

## Performance

- Lightweight — each check completes in < 2 seconds
- Non-blocking — doesn't lock up if a service is hung
- Graceful degradation — works even if some services are unavailable
