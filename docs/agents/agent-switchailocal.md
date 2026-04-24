# `agent-switchailocal`

**Summary:** switchAILocal — unified local LLM gateway on port `18080`. Installs the `@traylinx/switchailocal` npm package + launchd / systemd service.
**Kind:** Agent (plugin) · **Language:** Node (via npm) · **Source:** `plugins-core/agent-switchailocal/`

## When to use

When you want a **single OpenAI-compatible endpoint** on `localhost:18080` that routes to multiple model providers (Anthropic, OpenAI, local Ollama, Xiaomi MiMo, …) depending on the `model` alias in the request.

Makakoo's `makakoo query` and the multimodal omni tools assume this endpoint is available. Install this agent if you don't have your own gateway.

**Alternative:** you can point Makakoo at a different gateway by running `makakoo setup model-provider` and choosing a custom base URL.

## Prerequisites

- Node.js (any recent LTS). If not present, `install.sh` installs it via `nvm` into `~/MAKAKOO/state/agent-switchailocal/`.
- API keys for whichever upstream providers you want to route to, stored in the Makakoo keyring:

  ```sh
  makakoo secret set anthropic.api_key
  makakoo secret set openai.api_key
  # ...
  ```

## Start / stop

Managed by a launchd service (macOS) or systemd unit (Linux) spawned by `install.sh`:

```sh
makakoo plugin info agent-switchailocal
```

Manual control:

```sh
cd ~/MAKAKOO/plugins/agent-switchailocal
./install.sh start    # brings up launchd/systemd service
./install.sh stop
./install.sh status
```

## Where it writes

- **State:** `~/MAKAKOO/state/agent-switchailocal/` — nvm Node, npm cache, service configs.
- **Logs:** `~/Library/Logs/switchailocal.err.log` (macOS) or `~/MAKAKOO/data/logs/switchailocal.err.log` (Linux).
- **Service manifest:** `~/Library/LaunchAgents/com.traylinx.switchailocal.plist` (macOS).

## Health signals

- `curl -sS http://localhost:18080/v1/models | jq '.data | length'` — non-zero.
- `curl -sS http://localhost:18080/healthz` — returns `ok`.
- `makakoo plugin info agent-switchailocal` — `enabled: yes`.

## Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `curl: (7) Failed to connect to localhost:18080` | Service not running | `cd ~/MAKAKOO/plugins/agent-switchailocal && ./install.sh start`. Check `err.log` if that doesn't work. |
| `401 unauthorized` from the gateway | Upstream API key missing or invalid | `makakoo secret set <provider>.api_key`. |
| `unknown provider for model <alias>` | Alias not registered with the gateway | Open `~/.switchailocal/config.json` and add the mapping, or switch to a default alias. |
| Port 18080 already in use | Another service on that port | Either stop the other service or change switchAILocal's port in its config (then update `makakoo setup model-provider`). |

## Capability surface

- `net/http:0.0.0.0:18080` — the gateway listen port.
- `net/http:*` — outbound to LLM providers.
- `secret/read:*.api_key` — provider credentials.
- `fs/read` + `fs/write` — own state dir.
- `exec/shell` — running Node + npm.

## Remove permanently

```sh
makakoo plugin uninstall agent-switchailocal --purge
```

`--purge` tears down the launchd/systemd service and deletes the nvm install. After removal, point Makakoo at another gateway via `makakoo setup model-provider`.
