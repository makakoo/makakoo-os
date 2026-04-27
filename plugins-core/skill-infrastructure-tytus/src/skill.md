---
name: tytus
description: >
  Private AI pod infrastructure via WireGuard tunnels. Manage pod lifecycle
  (login, allocate, connect, revoke) via the `tytus` CLI. Provides an encrypted
  OpenAI-compatible gateway with 383+ models on your own infrastructure.
  Use when: (1) user mentions "tytus", "private pod", "my AI pod", (2) needs a
  private/encrypted AI endpoint, (3) wants to allocate or connect pods,
  (4) references switchAILocal on a remote droplet, or (5) needs TYTUS_* env vars.
metadata:
  {
    "openclaw":
      {
        "emoji": "🔒",
        "requires": { "bins": ["tytus"] },
        "install":
          [
            {
              "id": "tytus-setup",
              "kind": "bash",
              "script": "cd /Users/sebastian/Projects/makakoo/api/ProjectWannolot/services/tytus-cli && cargo build --release -p atomek-cli && sudo cp target/release/tytus /usr/local/bin/",
              "label": "Build and install tytus CLI"
            }
          ]
      }
  }
---

# Tytus Skill — Private AI Pod Infrastructure

Tytus gives agents a **private, encrypted AI gateway** via WireGuard tunnels. Each pod runs SwitchAILocal (383+ models) + an agent container (OpenClaw or Hermes) on isolated infrastructure.

**Think of it as:** switchAILocal, but remote, private, and encrypted — accessible from anywhere.

## When to Use

**USE this skill when:**
- User says "connect to tytus", "my AI pod", "private pod", "allocate pod"
- You need a remote OpenAI-compatible endpoint (not localhost)
- User wants encrypted AI access via WireGuard
- You see `TYTUS_*` environment variables
- User mentions pod allocation, unit budgets, or agent types (OpenClaw, Hermes)

**DON'T use this skill when:**
- User wants local AI (use switchAILocal skill instead)
- User is asking about non-AI infrastructure
- The endpoint is already `localhost:18080` (that's local switchAILocal, not Tytus)

---

## Agent Auto-Setup (Zero-Config Bootstrap)

Follow these steps IN ORDER to get Tytus working from scratch.

### Step 1 — Check if installed

```bash
which tytus && tytus --version || echo "NOT INSTALLED"
```

### Step 2 — Install if missing

```bash
cd /Users/sebastian/Projects/makakoo/api/ProjectWannolot/services/tytus-cli
cargo build --release -p atomek-cli -p tytus-mcp
sudo cp target/release/tytus target/release/tytus-mcp /usr/local/bin/
```

### Step 3 — Check login status

```bash
tytus status --json 2>/dev/null | head -1
```

If `"logged_in":false` or no output:
```bash
tytus login
# Browser opens → user approves → tokens saved
```

### Step 4 — Check for existing pods

```bash
tytus status --json | jq '.pods[]'
```

### Step 5 — Connect (if no active tunnel)

```bash
# Needs sudo for WireGuard TUN device
sudo tytus connect                    # OpenClaw agent (1 unit)
sudo tytus connect --agent hermes     # Hermes agent (2 units)
sudo tytus connect --pod 02           # Reconnect existing pod
```

**IMPORTANT:** `sudo tytus connect` runs in the foreground. It blocks until Ctrl+C. Run it in a **separate terminal** or background it.

### Step 6 — Load connection info

```bash
eval $(tytus env --export)
echo "Gateway: $TYTUS_AI_GATEWAY"
echo "API Key: $TYTUS_API_KEY"
```

### Step 7 — Verify

```bash
curl -s "$TYTUS_AI_GATEWAY/v1/models" \
  -H "Authorization: Bearer $TYTUS_API_KEY" | head -c 200
```

---

## Critical: Endpoint Format

Tytus endpoints are on the WireGuard subnet, NOT localhost.

| Wrong | Correct | Why |
|-------|---------|-----|
| `http://localhost:18080` | `http://10.18.1.1:18080` | That's LOCAL switchAILocal, not Tytus |
| `http://10.18.1.1:18080/chat` | `http://10.18.1.1:18080/v1/chat/completions` | Must include `/v1` prefix |
| No auth header | `Authorization: Bearer $TYTUS_API_KEY` | API key required |

**Always get the exact endpoint from `tytus env`** — don't guess the IP.

---

## Complete Command Reference

### Authentication

```bash
tytus login              # Device auth (opens browser, one-time)
tytus login --json       # JSON output
tytus logout             # Revoke all pods + clear credentials
```

### Pod Lifecycle

```bash
sudo tytus connect                     # Allocate new OpenClaw pod (1 unit)
sudo tytus connect --agent hermes      # Allocate Hermes pod (2 units)
sudo tytus connect --pod 02            # Reconnect existing pod
sudo tytus connect --json              # JSON: pod info to stdout, progress to stderr
tytus revoke 02                        # Release pod, free units
```

### Status & Info

```bash
tytus status             # Human-readable plan + pods
tytus status --json      # JSON (for scripts)
tytus env                # KEY=VALUE connection vars
tytus env --export       # export KEY=VALUE (source-able)
tytus env --pod 02       # Specific pod
tytus env --json         # Full pod as JSON
```

### Disconnect & Cleanup

```bash
tytus disconnect             # Clear all tunnel state
tytus disconnect --pod 02    # Clear specific pod tunnel state
tytus logout                 # Revoke all + logout
```

### Global Flag

`--json` works on ALL commands. Outputs structured JSON for programmatic use.

---

## Agent Types

| Agent | Flag | Units | Port | Health | Best For |
|-------|------|-------|------|--------|----------|
| **OpenClaw** | `--agent nemoclaw` | 1 | 3000 | `/healthz` | Simple tasks, tight budget |
| **Hermes** | `--agent hermes` | 2 | 8642 | `/health` | Coding, research, 60+ tools |

### Unit Budgets

| Plan | Units | Example Allocations |
|------|-------|-------------------|
| Explorer | 1 | 1 OpenClaw |
| Creator | 2 | 2 OpenClaw OR 1 Hermes |
| Operator | 4 | 4 OpenClaw OR 2 Hermes OR mix |

---

## Endpoints After Connection

Once `sudo tytus connect` is running:

| Service | URL Pattern | Protocol |
|---------|-------------|----------|
| **AI Gateway** | `http://10.{octet}.{pod}.1:18080` | OpenAI-compatible REST |
| **OpenClaw Agent** | `http://10.{octet}.{pod}.1:3000` | REST |
| **Hermes Agent** | `http://10.{octet}.{pod}.1:8642` | REST |

Get exact URLs: `tytus env`

### Environment Variables

| Variable | Example | Description |
|----------|---------|-------------|
| `TYTUS_AI_GATEWAY` | `http://10.18.1.1:18080` | OpenAI-compatible LLM gateway |
| `TYTUS_AGENT_API` | `http://10.18.1.1:3000` | Agent API endpoint |
| `TYTUS_API_KEY` | `sk-566cecd...09a0` | Bearer token for gateway |
| `TYTUS_AGENT_TYPE` | `nemoclaw` | Running agent type |
| `TYTUS_POD_ID` | `01` | Pod identifier |

---

## API Usage

### List Models

```bash
curl "$TYTUS_AI_GATEWAY/v1/models" \
  -H "Authorization: Bearer $TYTUS_API_KEY"
```

### Chat Completion

```bash
curl "$TYTUS_AI_GATEWAY/v1/chat/completions" \
  -H "Authorization: Bearer $TYTUS_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-8b",
    "messages": [{"role": "user", "content": "Hello from my private pod!"}]
  }'
```

### Streaming

```bash
curl "$TYTUS_AI_GATEWAY/v1/chat/completions" \
  -H "Authorization: Bearer $TYTUS_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "qwen3-8b",
    "messages": [{"role": "user", "content": "Explain WireGuard"}],
    "stream": true
  }'
```

### Python

```python
from openai import OpenAI
import subprocess, json

pod = json.loads(subprocess.check_output(["tytus", "env", "--json"]))
client = OpenAI(
    base_url=pod["ai_endpoint"] + "/v1",
    api_key=pod["pod_api_key"],
)
response = client.chat.completions.create(
    model="qwen3-8b",
    messages=[{"role": "user", "content": "Hello!"}]
)
print(response.choices[0].message.content)
```

### Node.js

```javascript
const { execSync } = require('child_process');
const OpenAI = require('openai');

const pod = JSON.parse(execSync('tytus env --json').toString());
const client = new OpenAI({
  baseURL: pod.ai_endpoint + '/v1',
  apiKey: pod.pod_api_key,
});
const r = await client.chat.completions.create({
  model: 'qwen3-8b',
  messages: [{ role: 'user', content: 'Hello!' }],
});
```

---

## Integration with AI CLIs

### Claude Code

Add to project `CLAUDE.md`:
```markdown
## Private AI (Tytus)
Tunnel must be running: `sudo tytus connect` in separate terminal.
Load env: `eval $(tytus env --export)`
Gateway: $TYTUS_AI_GATEWAY/v1 (OpenAI-compatible)
Key: $TYTUS_API_KEY
```

### Codex / OpenAI CLI

```bash
eval $(tytus env --export)
export OPENAI_API_KEY=$TYTUS_API_KEY
export OPENAI_BASE_URL=${TYTUS_AI_GATEWAY}/v1
codex  # Now routes through your private pod
```

### Gemini CLI

```bash
eval $(tytus env --export)
# Configure as custom OpenAI endpoint in your Gemini CLI config
```

### Any OpenAI-compatible tool

```bash
eval $(tytus env --export)
export OPENAI_API_KEY=$TYTUS_API_KEY
export OPENAI_BASE_URL=${TYTUS_AI_GATEWAY}/v1
# Done — any tool reading these env vars routes through Tytus
```

---

## Decision Tree

```
What do you need?
├─ Private encrypted AI endpoint (not localhost)
│   └─ Tytus: sudo tytus connect
├─ Local AI (no internet needed)
│   └─ switchAILocal: ail start (use switchailocal skill)
├─ Already have TYTUS_* env vars set
│   └─ Just use them: curl $TYTUS_AI_GATEWAY/v1/...
├─ Need to check pod status
│   └─ tytus status --json
├─ Need more capacity
│   └─ tytus revoke old-pod && sudo tytus connect --agent hermes
└─ Something broke
    └─ tytus logout && tytus login && sudo tytus connect
```

---

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `TUN device requires root` | `sudo tytus connect` |
| `Not logged in` | `tytus login` |
| `No Tytus subscription` | Upgrade at traylinx.com |
| `plan_limit_reached` | `tytus revoke <pod>` then retry |
| `Config download failed` | Pod provisioning — wait 10s, retry |
| `Token refresh failed` | `tytus logout && tytus login` |
| `Connection refused on endpoint` | Tunnel not running — `sudo tytus connect` |
| No `TYTUS_*` vars | `eval $(tytus env --export)` |
| Tunnel dropped | Ctrl+C was pressed — re-run `sudo tytus connect --pod XX` |

### Debug

```bash
RUST_LOG=debug sudo tytus connect     # Verbose
RUST_LOG=trace sudo tytus connect     # Very verbose (packet loop)
```

---

## Cross-Cutting Rules (ALL AGENTS MUST FOLLOW)

1. **NEVER guess endpoint IPs** — always get them from `tytus env`.
2. **ALWAYS include `Authorization: Bearer $TYTUS_API_KEY`** in requests.
3. **ALWAYS use `/v1/` prefix** in API paths (e.g., `/v1/chat/completions`).
4. **Tunnel runs in foreground** — needs a separate terminal or background process.
5. **`sudo` is required** for `tytus connect` — TUN devices need root.
6. **Check status before connecting** — `tytus status --json` to see existing pods.
7. **Don't allocate duplicate pods** — check existing pods first, reconnect with `--pod`.
8. **Handle Ctrl+C gracefully** — tunnel shuts down cleanly on signal.
9. **State is at `~/.config/tytus/state.json`** — never edit manually.
10. **Tytus = remote switchAILocal** — same API, different network (WireGuard vs localhost).

---

## Relationship to switchAILocal

| Aspect | switchAILocal | Tytus |
|--------|--------------|-------|
| Location | Local (`localhost:18080`) | Remote (WireGuard subnet) |
| Network | No encryption needed | WireGuard encrypted tunnel |
| Models | Your local providers | 383+ models on Tytus droplet |
| Auth | `sk-test-123` (any key) | Pod-specific API key (`$TYTUS_API_KEY`) |
| Setup | `ail start` | `sudo tytus connect` |
| Cost | Free (your hardware) | Subscription (Explorer/Creator/Operator) |
| API | OpenAI-compatible | OpenAI-compatible (same) |
| Agent | Optional | OpenClaw (port 3000) or Hermes (port 8642) |

**Both use the same API format.** Code that works with `http://localhost:18080/v1` works with `$TYTUS_AI_GATEWAY/v1` — just swap the base URL and add the API key.

---

*Private. Encrypted. Your AI, your rules.* 🔒
