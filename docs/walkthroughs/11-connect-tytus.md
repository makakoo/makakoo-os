# Walkthrough 11 — Connect a Tytus private pod

## What you'll do

Pair your Mac with a **Tytus private AI pod** — an isolated, WireGuard-tunneled, OpenAI-compatible gateway running on your own account. Once connected, `makakoo query` (and any infected AI CLI) can route LLM calls through your pod instead of a public provider, keeping data on your slice and giving you a stable endpoint that works across public Wi-Fi.

**Time:** about 6 minutes. **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md), a Tytus account ([tytus.dev](https://tytus.dev) signup). `tytus` CLI installed — usually a separate install.

> **Tytus is a separate product.** Makakoo works fine without it. This walkthrough is for users who want a private LLM backend. Skip if you don't have a Tytus account.

## What is Tytus?

- **Tytus** — customer-facing name for the private AI pod product (by Traylinx).
- **Pod** — one isolated slice: a WireGuard tunnel + a container agent (e.g. `nemoclaw` — an OpenClaw runtime sandbox).
- **`tytus` CLI** — the command you run on your Mac to manage pod allocation, the tunnel, and routing.

Every pod exposes a **stable OpenAI-compatible endpoint** at `http://10.42.42.1:18080/v1` and a stable per-user API key. The tunnel terminates on the pod — your Mac's outbound LLM calls never traverse the public internet beyond the encrypted WireGuard path.

## Steps

### 1. Check if `tytus` is on your `$PATH`

```sh
which tytus
```

Expected output:

```text
/Users/you/bin/tytus
```

If `which` returns nothing, install the `tytus` CLI first — see [tytus.dev/install](https://tytus.dev/install) (this walkthrough does not cover the `tytus` binary install; it's a separate download).

### 2. Confirm version

```sh
tytus --version
```

Expected output:

```text
tytus 0.5.0
```

(Any `0.5.x` works for this walkthrough; older versions may have slightly different command names.)

### 3. Log in if you haven't already

```sh
tytus login
```

This opens your browser to Sentinel (Tytus's auth server), you approve the device, and your CLI is paired with your Tytus account.

Expected output:

```text
Opening browser for device-code approval...
✓ authenticated as you@example.com
plan: operator (4 units)
```

### 4. See your current state

```sh
tytus status
```

Expected output on an account with one connected pod:

```text
Tytus — you@example.com
Plan: operator

Pod 02 [nemoclaw] connected
  Endpoint:      http://10.42.42.1:18080
  API Key:       sk-tytus-user-0...0a42
  Tunnel:        utun5
```

The `[nemoclaw]` tag is the agent running inside the pod. `connected` means the WireGuard tunnel is up.

### 5. If you don't have a pod yet, allocate one

```sh
tytus connect --agent nemoclaw
```

Expected output (interactive — picks the first free pod slot):

```text
Allocating pod 02 on your operator plan...
 → WireGuard tunnel up on utun5 in 4.2s
 → nemoclaw agent reporting healthy
 → gateway responding at http://10.42.42.1:18080
✓ connected pod 02
```

### 6. Smoke-test the pod

```sh
tytus test
```

Expected output:

```text
Tytus E2E smoke test — pod 02

 [✓] auth         you@example.com
 [✓] tunnel       utun5 — 10.42.42.1 reachable (2.1ms)
 [✓] gateway      http://10.42.42.1:18080 returns 200
 [✓] chat         ail-compound: response in 820ms

All green.
```

### 7. Export the stable connection variables

```sh
eval "$(tytus env --export)"
echo "$OPENAI_BASE_URL"
echo "${OPENAI_API_KEY:0:16}..."  # print a prefix so you don't leak the key
```

Expected output:

```text
http://10.42.42.1:18080/v1
sk-tytus-user-02...
```

Both values are stable across pod reallocation, revoke, droplet migration. Never hardcode them in source — always read from `tytus env`.

### 8. Route a direct OpenAI-compatible request through the pod

```sh
curl -sS "$OPENAI_BASE_URL/chat/completions" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"ail-compound","messages":[{"role":"user","content":"say hi"}]}' \
  | jq '.choices[0].message.content'
```

Expected output:

```text
"Hi! How can I help you today?"
```

You just made an LLM call through your private pod. No OpenAI key, no Anthropic key — just your Tytus tunnel.

### 9. (Optional) Wire Makakoo to use Tytus as the model provider

```sh
makakoo setup model-provider
```

When the wizard asks for a provider, pick "tytus-cli" (or equivalent — the exact label depends on your `makakoo` version). The wizard reads `tytus env --export` for you and writes `~/MAKAKOO/config/model_provider.toml`. After this, `makakoo query` routes through your pod.

### 10. (Optional) Chat directly

```sh
tytus chat
```

Expected output:

```text
Pod 02 / nemoclaw / ail-compound
>
```

Type a message, press Enter. `Ctrl-D` or `/exit` to leave.

## What just happened?

- **Your Mac now has a private AI endpoint** at `http://10.42.42.1:18080/v1` that behaves like the OpenAI API but runs on hardware you (not OpenAI) control.
- **The WireGuard tunnel** is managed by `tytus connect` / `tytus disconnect`. The PID file lives at `/tmp/tytus/tunnel-<NN>.pid`; you do not need to touch it.
- **`tytus env --export`** is stable: the same base URL + API key persist across revoke / reallocate / droplet migration. Reference these from shell, CI, or `model_provider.toml` — never the per-pod raw values (`tytus env --raw`).
- **Makakoo is provider-agnostic.** Tytus is one of several options. If you switch providers later, only `makakoo setup model-provider` changes; your Brain, plugins, and agents are unaffected.

## If something went wrong

| Symptom | Fix |
|---|---|
| `which tytus` returns nothing | `tytus` CLI not installed. See [tytus.dev/install](https://tytus.dev/install) — out of scope for this walkthrough. |
| `tytus login` opens browser but never returns | Check if your browser opened to the right device-code URL. If not, find the URL in `tytus login`'s stderr and paste it manually. |
| `tytus test` shows `tunnel: DOWN` | Another VPN is holding `utun*` interfaces or conflicting routes. Disconnect other VPNs, then `tytus disconnect` / `tytus connect`. |
| `Invalid API key` from the gateway right after connect | Stable-key map isn't synced yet (2-second race). Wait 2s and retry, or `tytus restart`. |
| `OpenClaw falls back to gpt-5.4` | `agents.defaults.model` typo in your overlay config. Check the JSON path is `agents.defaults.model` (singular `defaults`). |
| `Pod config not ready` | Backend issue on the droplet — not something `tytus` CLI can fix. Escalate to Tytus support with the exact error output. |

Full reference + more error scenarios: run `tytus llm-docs` — that's the comprehensive AI-facing document for the CLI.

## Next

- [Walkthrough 12 — Octopus federation](./12-octopus-federation.md) (pending — stub page). Connects your Mac to peers (other Tytus pods, other users' Macs) via signed-MCP over WireGuard, for a shared cognitive substrate.
