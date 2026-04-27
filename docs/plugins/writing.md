# Writing Plugins

Create your own Makakoo plugins.

## Overview

A plugin is a directory with a `plugin.toml` and entrypoint.

```
my-plugin/
├── plugin.toml          # Manifest
├── src/
│   └── main.py         # Entry point
└── README.md           # Documentation
```

## Minimal Example

### 1. Create Manifest

```toml
# plugin.toml
[plugin]
name = "my-plugin"
version = "0.1.0"
kind = "skill"
language = "python"
summary = "My first plugin"

[source]
path = "."  # Local plugin

[abi]
skill = "^0.1"

[depends]
python = ">=3.11"
```

### 2. Create Entrypoint

```python
#!/usr/bin/env python3
# src/main.py

import sys
import os

def run():
    """Called when task runs"""
    print("Hello from my plugin!")
    return 0

def health():
    """Called for health checks"""
    print("ok")
    return 0

if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "run"
    
    if cmd == "run":
        sys.exit(run())
    elif cmd == "health":
        sys.exit(0 if health() else 1)
    else:
        print(f"Unknown command: {cmd}")
        sys.exit(1)
```

### 3. Install

```bash
makakoo plugin install ./my-plugin --allow-unsigned
```

## Plugin Types

### Skill

Skills are reusable prompts/workflows.

```toml
[plugin]
name = "my-skill"
version = "0.1.0"
kind = "skill"
summary = "Does X task"
```

Add a `SKILL.md`:

```markdown
# My Skill

Use this skill when the user asks about X.

## Steps
1. Do this
2. Then that
3. Return results
```

### Agent

Agents are long-running processes.

```toml
[plugin]
name = "my-agent"
version = "0.1.0"
kind = "agent"
summary = "Autonomous worker"

[entrypoint]
start = ".venv/bin/python -m my_agent.main --start"
stop = ".venv/bin/python -m my_agent.main --stop"
health = ".venv/bin/python -m my_agent.main --health"
```

### SANCHO Task

Scheduled tasks.

```toml
[plugin]
name = "my-watchdog"
version = "0.1.0"
kind = "sancho-task"
summary = "Monitors something"

[sancho]
tasks = [
  { name = "my_check", interval = "5m", active_hours = [0, 24] }
]

[entrypoint]
run = ".venv/bin/python -m my_watchdog.run"
```

## Capabilities

Declare what your plugin needs:

```toml
[capabilities]
grants = [
  "brain/read",                    # Read memory
  "brain/write",                   # Write journals
  "llm/chat",                      # Call LLM
  "net/http:https://api.example/*", # Network
  "secrets/read:MY_API_KEY",       # Secrets
  "state/plugin",                  # Own state dir
]
```

### Available Capabilities

| Verb | Scope | Purpose |
|------|-------|---------|
| `brain/read` | — | Read Brain |
| `brain/write` | — | Write journals |
| `llm/chat` | model (optional) | Call LLM |
| `llm/embed` | model (optional) | Get embeddings |
| `net/http` | URL glob | HTTP requests |
| `secrets/read` | key name | Read secrets |
| `state/plugin` | — | Own state dir |
| `fs/read` | path glob | Read files |
| `fs/write` | path glob | Write files |

## Environment Variables

Plugins receive these:

```bash
MAKAKOO_HOME          # ~/MAKAKOO
MAKAKOO_SOCKET_PATH   # For capability socket
PLUGIN_NAME           # my-plugin
PLUGIN_ROOT           # /path/to/plugin
```

## Using Capabilities

### Python Client

```python
from makakoo import Client

client = Client.connect_from_env()

# Read Brain
journals = client.brain_recent(10)

# Write to Brain
client.brain_write("- Did something")

# Call LLM
response = client.llm_chat("minimax/ail-compound", [
    {"role": "user", "content": "Hello"}
])

# Read secret
with client.secret_read("MY_API_KEY") as key:
    print(f"Key: {key.value}")
```

### Without Client

```python
import os
import json
import urllib.request

MAKAKOO_SOCKET = os.environ["MAKAKOO_SOCKET_PATH"]

def call_capability(method, params):
    request = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    }
    
    # Send to socket
    data = json.dumps(request).encode()
    with urllib.request.urlopen(f"unix://{MAKAKOO_SOCKET}", data) as response:
        return json.loads(response.read())
```

## SANCHO Task Contract

### Run Command

When SANCHO fires your task:

```bash
python -m my_watchdog.run --task my_check
```

### Output Format

Return JSON to stdout:

```python
import json
import sys

result = {
    "status": "ok",
    "duration_ms": 1234,
    "items_processed": 42,
    "message": "Checked X items"
}
print(json.dumps(result))
```

### Status Values

| Status | Meaning |
|--------|---------|
| `ok` | Task succeeded |
| `partial` | Task partially succeeded |
| `skipped` | Gates prevented execution |

## Agent Contract

### Entrypoint Commands

```python
# main.py

import sys

def start():
    """Called on startup"""
    print("Starting agent...")
    # Fork to background or enter main loop
    return 0

def stop():
    """Called on shutdown"""
    print("Stopping agent...")
    return 0

def health():
    """Called periodically"""
    return 0

if __name__ == "__main__":
    cmd = sys.argv[1]
    
    if cmd == "--start":
        sys.exit(start())
    elif cmd == "--stop":
        sys.exit(stop())
    elif cmd == "--health":
        sys.exit(health())
```

### Health Response

```python
def health():
    health_data = {
        "status": "ok",
        "uptime_s": 3600,
        "tasks_processed": 42
    }
    print(json.dumps(health_data))
    return 0
```

## Publishing

### Directory Structure

```
my-package/
├── plugin.toml
├── src/
│   └── main.py
├── SKILL.md          # For skill plugins
├── README.md
└── tests/
    └── test_main.py
```

### Publishing Steps

1. Create `plugin.toml` with git source:

```toml
[source]
git = "https://github.com/user/makakoo-my-plugin"
rev = "v0.1.0"
blake3 = "..."  # Hash of tree
```

2. Test locally:

```bash
makakoo plugin install ./my-package --allow-unsigned
```

3. Push to GitHub with tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

4. Share your plugin URL:

```
https://github.com/user/makakoo-my-plugin
```

## Examples

### Hello World Skill

```
hello-skill/
├── plugin.toml
└── SKILL.md
```

```toml
# plugin.toml
[plugin]
name = "hello-skill"
version = "0.1.0"
kind = "skill"
summary = "Says hello"

[source]
path = "."

[abi]
skill = "^0.1"
```

```markdown
# Hello Skill

Use this skill when you want to greet someone.

## Steps
1. Respond with a friendly greeting
2. Ask how you can help
```

### Weather Watchdog

```
weather-watchdog/
├── plugin.toml
└── src/
    └── main.py
```

```toml
# plugin.toml
[plugin]
name = "weather-watchdog"
version = "0.1.0"
kind = "sancho-task"
summary = "Checks weather API"

[source]
path = "."

[abi]
sancho-task = "^0.1"

[capabilities]
grants = [
  "net/http:https://api.weather.com/*",
]

[sancho]
tasks = [
  { name = "weather_check", interval = "30m" }
]

[entrypoint]
run = ".venv/bin/python -m src.main run"
```

```python
# src/main.py
import json
import urllib.request
import os

def run_task():
    # Check weather API
    url = "https://api.weather.com/v3/wx/conditions/current"
    # ... make request ...
    
    return {
        "status": "ok",
        "message": "Weather check complete"
    }

if __name__ == "__main__":
    if sys.argv[1] == "run":
        print(json.dumps(run_task()))
```

## Testing

### Local Install

```bash
# Install locally
makakoo plugin install ./my-plugin --allow-unsigned --skip-health-check

# Check it loaded
makakoo plugin list

# Run task manually
makakoo sancho run my_check --force
```

### Debug Mode

```bash
# Run with verbose
RUST_LOG=debug makakoo sancho run my_check

# Check logs
tail -f ~/.makakoo/logs/sancho/my_check/*.log
```

## See Also

- [Plugin Guide](./index.md) — Using plugins
- [Capabilities Reference](../spec/CAPABILITIES.md) — Full capability list
- [Plugin Manifest Schema](../spec/PLUGIN_MANIFEST.md) — TOML reference
