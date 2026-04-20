# Agent Discovery Skill

**Purpose:** Dynamic agent registration and discovery — agents register themselves on startup and other agents can discover them by capability.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│         Agent Discovery Service (port 18081)         │
├──────────────┬───────────────┬────────────────────────┤
│  Registry    │   Heartbeat  │   Capability Index    │
│  SQLite      │   Monitor    │   (inverted index)    │
└──────────────┴───────────────┴────────────────────────┘
          ▲ register/heartbeat ▲
          │                    │
    ┌─────┴─────┐         ┌─────┴─────┐
    │  Agent A  │         │  Agent B  │
    └───────────┘         └───────────┘
```

## Components

| Component | File | Purpose |
|-----------|------|---------|
| `models.py` | `AgentRecord` dataclass | Data model |
| `store.py` | `RegistryStore` | SQLite-backed registry |
| `api.py` | HTTP server | Registration + discovery API |
| `monitor.py` | `HeartbeatMonitor` | Background stale cleanup |
| `client.py` | `DiscoveryClient` | Agent SDK |
| `cli.py` | CLI tool | Management commands |

## Starting the Service

```bash
# Start the API server
python3 harvey-os/skills/orchestration/agent_discovery/api.py

# Or run as module
python3 -m harvey_os.skills.orchestration.agent_discovery.api
```

The server runs on `http://localhost:18081` by default.

## API Endpoints

### Registration

```bash
# Register an agent
curl -X POST http://localhost:18081/agents/register \
  -H "Content-Type: application/json" \
  -d '{
    "agent_id": "harvey-primary",
    "name": "Harvey Primary Agent",
    "capabilities": ["reasoning", "planning", "code"],
    "skills": ["/plan", "/investigate", "/ship"],
    "endpoint": "http://localhost:8080",
    "ttl_seconds": 300
  }'

# Heartbeat to refresh lease
curl -X POST http://localhost:18081/agents/heartbeat/harvey-primary \
  -H "Content-Type: application/json" \
  -d '{"ttl_seconds": 300}'

# Deregister
curl -X DELETE http://localhost:18081/agents/harvey-primary
```

### Discovery

```bash
# List all agents
curl http://localhost:18081/agents

# Find agents by capability
curl "http://localhost:18081/agents?capability=code-review"

# Get specific agent
curl http://localhost:18081/agents/harvey-primary

# Health stats
curl http://localhost:18081/agents/health
```

## Client SDK Usage

```python
from agent_discovery import DiscoveryClient

# Context manager — auto-registers and deregisters
with DiscoveryClient(
    agent_id="harvey-001",
    name="Harvey Primary",
    capabilities=["reasoning", "planning"],
    skills=["/plan", "/investigate", "/ship"],
    endpoint="http://localhost:8080",
) as client:
    # Find agents with code-review capability
    reviewers = client.find("code-review")
    print(f"Found {len(reviewers)} reviewers")

    # Find agents with a specific skill
    planners = client.find_by_skill("/plan")
```

## CLI

```bash
# List all registered agents
python3 harvey-os/skills/orchestration/agent_discovery/cli.py list

# Filter by capability
python3 harvey-os/skills/orchestration/agent_discovery/cli.py list --capability code-review

# Health statistics
python3 harvey-os/skills/orchestration/agent_discovery/cli.py status
```

## Agent Startup Integration

To auto-register on startup, add to agent initialization:

```python
from agent_discovery import DiscoveryClient
import atexit

client = DiscoveryClient(
    agent_id="my-agent",
    name="My Agent",
    capabilities=["reasoning"],
    skills=["/some-skill"],
)
client.register()
client.start_heartbeat()
atexit.register(lambda: client.deregister())
```

## TTL Behavior

- Default TTL: 300 seconds (5 minutes)
- Agents must heartbeat before TTL expires
- Stale agents (missed heartbeat) are auto-removed by the monitor
- Monitor runs every 30 seconds

## Data Directory

Registry SQLite database: `~/MAKAKOO/data/agent_discovery/registry.db`
