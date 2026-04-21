# Bring Your Own Agent

You have an LLM, a CLI tool, an MCP server, or another Makakoo install.
You want Makakoo to treat it as a swarm participant — callable from
`makakoo adapter call`, usable by lope as a validator, reachable via
the `harvey_swarm_run` MCP tool. This doc is the 10-minute path.

**Companion:** [consuming-makakoo-externally.md](consuming-makakoo-externally.md)
covers the *reverse* direction — you want your agent runtime
(LangChain / OpenAI Assistants / Cursor) to consume Makakoo's skills.

**Sprint it shipped with:** v0.6 Agentic Plug.

---

## Choose your shape

| You have… | Template | Time to wire |
|---|---|---|
| An OpenAI-compatible LLM endpoint (Mistral, DeepSeek, Groq, Together, Anthropic via shim, Ollama, vLLM, llama.cpp, your own Flask app that mimics `/v1/chat/completions`) | `openai-compat` | 30 seconds |
| A CLI that reads a prompt on stdin and prints a response on stdout | `subprocess` | 60 seconds |
| A third-party MCP stdio server binary | `mcp-stdio` | 60 seconds |
| Another Makakoo install running `makakoo-mcp --http` | `peer-makakoo` | 2 minutes (trust bootstrap) |

All four go through the same `makakoo adapter gen` scaffolder, all
four end up in `~/.makakoo/adapters/registered/<name>.toml`, and all
four become first-class swarm participants the moment the install
returns.

---

## 1. "I have an OpenAI-compatible LLM"

The most common case. Any endpoint that accepts POST `/v1/chat/completions`
with an OpenAI-shaped request body qualifies.

```bash
export DEEPSEEK_API_KEY=sk-...

makakoo adapter gen \
    --template openai-compat \
    --name deepseek \
    --url https://api.deepseek.com/v1 \
    --key-env DEEPSEEK_API_KEY \
    --model deepseek-chat

makakoo adapter call deepseek --prompt "say hello"
```

### What the generator does

- Writes `~/.makakoo/adapters/registered/deepseek.toml`.
- Infers `allowed_hosts = ["api.deepseek.com"]` from the URL.
- Defaults `key_env` to `<NAME>_API_KEY` if `--key-env` is omitted
  (hyphens → underscores, uppercased).
- Runs `adapter doctor deepseek` unless `--skip-doctor` is set.

### When to customize

- **Non-standard verdict field:** the template sets
  `verdict_field = "choices.0.message.content"`. If your endpoint nests
  the text differently, edit the TOML after generation or pass a
  different template base.
- **Multi-host network:** add more entries to `allowed_hosts` manually.
- **Model-free mode:** pass `--model ""` for endpoints that ignore the
  model field (some dev / mock servers).

---

## 2. "I have a CLI agent"

Your agent is a binary. It takes a prompt on stdin, writes a response
on stdout, exits. Examples: a Python script, a Rust binary, a shell
pipeline, `pi -p --no-session`, an internal company tool.

```bash
# Example 1 — a real CLI that reads stdin and prints a response.
makakoo adapter gen \
    --template subprocess \
    --name my-cli \
    --command ./my-cli \
    --command --reply-mode

makakoo adapter call my-cli --prompt "2+2?"

# Example 2 — a Python script taking prompt from stdin.
makakoo adapter gen \
    --template subprocess \
    --name py-worker \
    --command python3 \
    --command=/path/to/worker.py
```

### Flag quoting gotcha

Clap stops interpreting `--command bash -c "echo hi"` at `-c` because
it looks like a flag. Use `=` to attach values that start with `-`:

```bash
makakoo adapter gen \
    --template subprocess \
    --name shelly \
    --command bash \
    --command=-c \
    --command "echo hi"
```

### Output parsing

The default `output.format = "plain"` passes the subprocess's stdout
through verbatim. The adapter bridge heuristically picks a status
(PASS / NEEDS_FIX / FAIL) based on the presence of matching words —
fine for conversational agents, wrong for structured validators.

If your agent emits a JSON envelope, switch `output.format` to
`"openai-chat"` and set `verdict_field = "result"` (or your dot-path)
after generation.

---

## 3. "I have an MCP server"

You have an MCP stdio binary — yours or a third-party. The adapter
wraps it as one bridge participant. Callers get access to every tool
on the MCP server via the JSON envelope convention.

```bash
makakoo adapter gen \
    --template mcp-stdio \
    --name my-mcp \
    --command /usr/local/bin/my-mcp-server

# Default 'chat' tool (if the server has one):
makakoo adapter call my-mcp --prompt "hello"

# Any other tool, via JSON envelope:
makakoo adapter call my-mcp --prompt '{"tool":"list_projects","arguments":{"user":"me"}}'
```

### The envelope convention

Plain-string prompts route to `tools/call` with `name="chat"` and
`arguments={"prompt": <string>}` — v0.3 backwards-compat.

JSON-object prompts of shape `{"tool":"X","arguments":{...}}` route
to `tools/call` with `name="X"` and `arguments=<your-args>`. This is
how one adapter fans out to an MCP server's full tool catalog without
needing one manifest per tool. The canonical example is
`plugins-core/adapters/tytus-cli/` — one adapter, 7 Tytus tools.

### Output

`verdict_field = "result.content.0.text"` extracts the MCP content-text
element so callers see the tool's real response (not the JSON-RPC
envelope). Works for any server that follows the MCP `content` array
convention (i.e. every modern MCP server).

---

## 4. "I have another Makakoo install"

Both installs run `makakoo` and `makakoo-mcp`. You want to call
tools on one install from the other — from your laptop to your
workstation, or from a Tytus pod to your main install, or from a
CI runner to a dev box.

Makakoo is **transport-agnostic**. It doesn't ship a VPN. Pick any
network the two installs can already reach each other on:

- **Tailscale / Headscale** — `<peer>.tailnet.ts.net`
- **SSH tunnel** — `ssh -L 8765:127.0.0.1:8765 user@peer-host`
- **Cloudflare Tunnel** — `cloudflared tunnel` publishing one hostname
- **Plain LAN** — peer's LAN IP + firewall hole
- **Tytus WireGuard** — if both installs have Tytus pods, the pod
  network reaches across (10.42.42.1:<port>)

### Step-by-step (laptop A → workstation B)

**On B — start the HTTP server:**

```bash
# Auto-generates the keypair on first run; prints pubkey to stderr.
makakoo-mcp --http :8765

# ...
# makakoo-mcp: generated Ed25519 signing key at .../config/peers/signing.key
# pubkey (share with peers via `makakoo adapter trust add <name> <pubkey>`):
# BkpMi6QKUzvNF3VwhelL4OVmAt1Gfk2+hOGHbYUyArI=
```

**On A — get your own pubkey:**

```bash
makakoo adapter self-pubkey
# /EAq7YqPbJntml/ehTGCNjXeVfyl61iRLOcz5wORoi4=
```

**On B — trust A:**

```bash
# The name is how B refers to A internally; it's what A has to send
# in the X-Makakoo-Peer header.
makakoo adapter trust add laptop-a /EAq7YqPbJntml/ehTGCNjXeVfyl61iRLOcz5wORoi4=
```

**On A — scaffold + install the peer adapter:**

```bash
makakoo adapter gen \
    --template peer-makakoo \
    --name workstation \
    --url http://192.168.1.50:8765 \
    --peer-name laptop-a

# Now call any tool on B from A.
makakoo adapter call workstation \
    --prompt '{"tool":"harvey_brain_search","arguments":{"query":"last incident"}}'
```

The adapter uses `mcp-http-signed` transport. Every call signs
`sha256(body || timestamp)` with A's signing key; B verifies against
A's pubkey from the trust file. Replay window is ±60 seconds.

### TLS

Not built in. Makakoo assumes the transport layer (Cloudflare Tunnel,
Caddy reverse-proxy, Tailscale HTTPS, or SSH tunnel over an already-
encrypted channel) provides confidentiality. Binding `makakoo-mcp
--http` directly to a public IP without a reverse-proxy is a bad
idea — the auth is mutual, but the bodies are plaintext on the wire.

### Bind interface

Default is `127.0.0.1`. To bind elsewhere:

```bash
makakoo-mcp --http 0.0.0.0:8765 --bind 0.0.0.0
```

You'll see a warning banner in stderr. Auth is still enforced — the
warning is about the network posture being your responsibility, not
a silent security regression.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `adapter doctor <name>` reports `unset — export <ENV>=…` | The env var the adapter references isn't in your shell. | Export it, or edit the TOML to a different `key_env`. |
| `adapter call` times out with a subprocess adapter | Agent is waiting on stdin that never closes, or stdout is buffered. | Make sure the agent reads stdin to EOF; use `python -u` or `stdbuf -o0` to disable stdout buffering. |
| `adapter call my-mcp` returns empty body | The MCP server expects `initialize` first before `tools/call`. | v0.6 `mcp-stdio` transport handles stateless tools/call directly. If the server hard-requires init, either fix the server to tolerate naked tools/call or open an issue asking for pre-flight init in the transport. |
| `mcp-http-signed` returns `401 Unauthorized`: `clock drift` | Your laptop's clock is >60s away from the peer's. | `sudo sntp -sS time.apple.com` (macOS) or your distro's equivalent. 60s window is by design per v0.6 SPRINT.md D5. |
| `401 Unauthorized`: `unknown peer` | The peer-name you sent doesn't match anything in their trust file. | On the peer: `makakoo adapter trust list`. On your side: check the `peer_name` field in the adapter TOML matches exactly. |
| `401 Unauthorized`: `signature verification failed` | Your signing key isn't the one they trust — could be a fresh-install mismatch. | On your side: `makakoo adapter self-pubkey`. Compare to what they stored via `makakoo adapter trust list --with-keys`. If different, re-add with the current pubkey. |
| `adapter call` returns the raw MCP envelope | Your `verdict_field` doesn't match the server's response shape. | Run one call with `--json`, inspect the actual structure, update the dot-path. The default `result.content.0.text` fits most MCP servers. |
| `makakoo-mcp --http` starts then exits instantly | Port already bound. | `lsof -iTCP:<port> -sTCP:LISTEN` to find the offender. Or pick a free port via `--http :0` (auto-selects). |
| `adapter call` with `peer-makakoo` errors `MissingEnv("MAKAKOO_PEER_SIGNING_KEY")` | Neither the env var nor the default signing key file exists. | Run `makakoo adapter self-pubkey` once — auto-creates the file. |
| "I have a new adapter but `adapter list` doesn't show it" | It didn't install. | Check the scaffolder output for errors. If nothing installed, the template validation rejected something — re-run without `--skip-doctor` to see the full report. |

---

## What you can't do yet (deferred to v0.7+)

- **Stateful / streaming MCP over HTTP.** Request/response is single-shot.
- **Key rotation workflow.** Trust file add/remove is manual.
- **Peer discovery.** No mDNS / DHT — peers are named explicitly in
  `adapter.toml`.
- **Tool-use forwarding in openai-compat adapter.** The `tools` field
  from the chat-completions request isn't plumbed through yet; if your
  delegate calls Harvey MCP tools back, you'll need a custom parser.
- **`custom` output format.** Python parser plugin hook is still
  `OutputError::CustomUnsupported`.

---

## Reference

- **v0.6 sprint spec:** `development/sprints/MAKAKOO-OS-V0.6-AGENTIC-PLUG/SPRINT.md`
- **Adapter manifest schema:** `spec/ADAPTER_MANIFEST.md` (run
  `makakoo adapter spec` to dump the current version).
- **Universal bridge spec (v0.3):** `docs/adapters.md`
- **Adapter publishing:** `docs/adapter-publishing.md`
- **v0.3 tag:** `sprint-v0.3-universal-bridge-complete` on
  `github.com/makakoo/makakoo-os` (private).
- **v0.6 tag:** `sprint-v0.6-agentic-plug-complete`.
