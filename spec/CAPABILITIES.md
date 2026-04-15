# CAPABILITIES — Capability Verb Vocabulary + Enforcement

**Status:** v0.1 LOCKED — 2026-04-15
**Owner:** Makakoo kernel, `crates/core/src/capability/`
**Governs:** which verbs plugins can declare, how the kernel enforces
them, and the audit log format.

---

## 0. Purpose

This is the contract between the kernel and plugins around "what a
plugin is allowed to touch." Every plugin declares exactly which
capability verbs it needs in its manifest `[capabilities].grants`. The
kernel enforces those declarations via per-plugin Unix domain sockets
(named pipes on Windows) with PID-verified handshakes (see D11 in
ARCHITECTURE.md §4.4 and the full enforcement design in ARCHITECTURE.md
§8). Every call is audit-logged.

**Honesty boundary, not sandbox.** A plugin that wants to exfiltrate can
shell out to `curl` and bypass the entire system. That's accepted. The
capability system exists so that well-behaved plugins document what
they do and reviewers can audit the manifest without reading code. For
actual sandboxing we'd need OS-level isolation (Linux namespaces +
seccomp, macOS sandbox-exec, Windows AppContainer) which is a v0.2+
upgrade path (see ARCHITECTURE.md §8 "Future upgrade path").

## 1. The verb vocabulary (v0.1)

Every verb has a domain/name shape (`domain/name`) and an optional
scope (`domain/name:scope`). Unknown verbs are rejected at manifest
load time.

### 1.1 Brain

| Verb | Scope | Meaning |
|---|---|---|
| `brain/read` | — | Read Brain markdown files (journals, pages) and run FTS queries via the superbrain plugin |
| `brain/write` | — | Append journal entries, create new pages, update existing pages (no delete) |
| `brain/delete` | — | Delete pages from the Brain. Rare, always audit-logged with a red flag |
| `brain/query` | — | Run structured queries (semantic, wikilink walks) — implies `brain/read` |

**Scoping.** Brain verbs do not take scopes in v0.1. In v0.2+, scopes
could restrict to specific Brain namespaces (e.g. `brain/read:journals/*`,
`brain/write:pages/work/*`).

### 1.2 LLM

| Verb | Scope | Meaning |
|---|---|---|
| `llm/chat` | model glob (optional) | Call LLM chat completions through the Makakoo LLM client |
| `llm/embed` | model glob (optional) | Request embeddings |
| `llm/omni` | modality (optional) | Multimodal helpers: image/audio/video |

**Scoping.**
- `llm/chat` alone = call any LLM model available
- `llm/chat:minimax/ail-compound` = only that specific model
- `llm/chat:minimax/*` = any minimax model
- `llm/chat:*/ail-compound` = any provider, only ail-compound name
- `llm/omni:image` = only image understanding
- `llm/omni:*` = all modalities (equivalent to unscoped `llm/omni`)

**Why scoping matters:** LLM calls cost money. A trading agent that
only needs ail-compound shouldn't be able to accidentally burn tokens
on gpt-5.

### 1.3 Network

| Verb | Scope | Meaning |
|---|---|---|
| `net/http` | URL glob (optional) | Make HTTP(S) requests through the Makakoo HTTP helper |
| `net/tcp` | host:port glob (optional) | Raw TCP connection — rare |
| `net/udp` | host:port glob (optional) | Raw UDP — rarer |
| `net/ws` | URL glob (optional) | WebSocket connection |

**Scoping.** URL globs use `*` wildcards.
- `net/http` alone = any URL
- `net/http:https://api.example.com/*` = only that host
- `net/http:https://*.example.com/*` = any subdomain
- `net/http:https://api.example.com/v1/*` = only v1 paths

**Why scoping matters:** this is how we constrain an LLM-powered plugin
to only hit approved endpoints. Users reviewing a trading plugin's
manifest can verify at a glance that it only talks to polymarket.com,
not to random IPs.

### 1.4 State

| Verb | Scope | Meaning |
|---|---|---|
| `state/plugin` | — | Read/write own state dir (`$MAKAKOO_HOME/state/<plugin-name>/`) |
| `state/global` | path prefix | Read/write outside own state dir. Rare, always audit-logged |

**Default grant:** every plugin with a `[state]` table in its manifest
automatically gets `state/plugin` on its own directory. Declaration is
only needed to make it explicit (recommended for readability).

**state/global scoping.** `state/global:$MAKAKOO_HOME/data/Brain/` would
let a plugin write anywhere under Brain (equivalent to a broad
`brain/write`). In practice we route most of these through the Brain
verbs. `state/global` is the escape hatch for plugins that manage
non-Brain global state.

### 1.5 Filesystem

| Verb | Scope | Meaning |
|---|---|---|
| `fs/read` | path glob | Read files outside state dir (e.g. user's code repo) |
| `fs/write` | path glob | Write files outside state dir. Rare, audit-logged |

**Scoping is required.** Unscoped `fs/read` or `fs/write` is rejected
at manifest load. The scope must be a glob of absolute paths (tilde
and env vars expanded at grant time).

**Examples:**
- `fs/read:~/code/**` — read any file under the user's code dir
- `fs/write:~/.config/makakoo/plugins/arbitrage/output/*` — write only
  into a specific output folder inside the plugin's expected dir

**Why this is rare:** most plugins shouldn't need arbitrary filesystem
access. If they do, the manifest clearly shows it and the user can
reject the install.

### 1.6 Execution

| Verb | Scope | Meaning |
|---|---|---|
| `exec/binary` | binary allowlist | Can spawn a subprocess running a specific binary |
| `exec/shell` | — | Can run arbitrary shell commands (`bash -c '...'`). DANGEROUS, rare |

**Scoping is required for `exec/binary`.**
- `exec/binary:git` — can spawn `git` with any args
- `exec/binary:git,curl` — can spawn either
- `exec/binary:*` — equivalent to `exec/shell`; use `exec/shell` explicitly for clarity

**Why this matters:** a plugin that shells out to `curl` effectively
bypasses the `net/http` capability system. Making `exec/binary` a
first-class declaration means the manifest is honest about the
bypass route. `exec/shell` is a red flag on install.

### 1.7 Secrets

| Verb | Scope | Meaning |
|---|---|---|
| `secrets/read` | key allowlist | Read specific keys from the OS keyring |
| `secrets/write` | key allowlist | Write keys to the OS keyring (rare — usually user-only via `makakoo secret set`) |

**Scoping is required.** The scope is a comma-separated allowlist of
key names.

**Examples:**
- `secrets/read:AIL_API_KEY`
- `secrets/read:GITHUB_TOKEN,NOTION_API_KEY`
- `secrets/read:*` — rejected at manifest load (too broad)

Key names follow uppercase SCREAMING_SNAKE_CASE convention.

### 1.8 MCP

| Verb | Scope | Meaning |
|---|---|---|
| `mcp/register` | tool name | Register a specific MCP tool with the gateway |

**Scoping required.** Plugins declare exactly which tool names they
register:
- `mcp/register:arbitrage_status`
- `mcp/register:github_issue_list,github_issue_create`

Kernel refuses to let a plugin register a tool name not in its
declared scope, even if the plugin tries to register via a different
code path.

### 1.9 SANCHO

| Verb | Scope | Meaning |
|---|---|---|
| `sancho/register` | task name | Register a specific SANCHO task |

Same pattern as `mcp/register`. Plugins declare which task names they
register.

### 1.10 Infect (rare, internal)

| Verb | Scope | Meaning |
|---|---|---|
| `infect/contribute` | — | Contribute a Bootstrap Block fragment via `[infect.fragments]` |

This is granted automatically to any plugin that declares
`[infect.fragments]` in its manifest. No need to list it in
`[capabilities].grants`.

## 2. The grant resolution algorithm

At plugin start, the kernel:

1. Reads `[capabilities].grants` from the plugin's manifest
2. Normalizes each entry (expands env vars, resolves tildes, splits
   scopes on commas)
3. Builds a `HashMap<Verb, Vec<Scope>>` for the plugin's session
4. Auto-adds default grants:
   - `state/plugin` on `$MAKAKOO_HOME/state/<plugin-name>/` if
     `[state]` is present
   - `infect/contribute` if `[infect.fragments]` is present
   - `mcp/register:<tool-name>` for every tool in `[mcp.tools]`
   - `sancho/register:<task-name>` for every task in `[sancho.tasks]`
5. Opens the per-plugin Unix socket with this grant table in memory
6. Handshake: spawn plugin → plugin connects → kernel verifies PID
7. Ready to serve

When the plugin makes a helper call (`client.brain_read(...)`, etc):

1. Request arrives on the socket as JSON-RPC
2. Kernel extracts the requested verb + scope from the method name +
   params
3. Check against the grant table:
   - Exact verb match → check scopes (if any)
   - For scoped verbs: requested scope must match at least one granted
     scope (glob match)
4. If allowed → serve the call, return result
5. If denied → return error `{"error": {"code": -32001, "message":
   "capability denied: <verb>:<scope>"}}`
6. Either way: write audit log entry

## 3. The audit log

**Path:** `$MAKAKOO_HOME/logs/audit.jsonl`
**Format:** one JSON object per line
**Rotation:** rotate at 100 MB, archive to `audit.jsonl.<timestamp>.gz`, keep last 7 days by default

**Schema:**

```json
{
  "ts": "2026-04-15T17:35:22.123Z",
  "plugin": "agent-arbitrage",
  "plugin_version": "0.3.1",
  "verb": "net/http",
  "scope_requested": "https://clob.polymarket.com/books/BTC",
  "scope_granted": "https://clob.polymarket.com/*",
  "result": "allowed",
  "duration_ms": 142,
  "bytes_in": 1204,
  "bytes_out": 0,
  "correlation_id": "abc123"
}
```

**Fields:**
- `ts` — ISO 8601 UTC timestamp
- `plugin` — plugin name from manifest
- `plugin_version` — semver from manifest
- `verb` — capability verb being checked
- `scope_requested` — the specific thing the plugin asked for (URL,
  path, key name, etc.)
- `scope_granted` — which declared grant matched (null if denied)
- `result` — `allowed` | `denied` | `error`
- `duration_ms` — time from request to response
- `bytes_in` / `bytes_out` — data size for network calls
- `correlation_id` — lets us group related calls (e.g. a single
  `llm_chat` that also triggers `llm_embed` internally)

**User-facing:** `makakoo audit` prints recent entries. `makakoo audit
--plugin <name>` filters by plugin. `makakoo audit --denied` shows only
denials (for debugging misconfigured manifests).

## 4. Enforcement across the 4 OSes

### 4.1 macOS and Linux

Native Unix domain sockets via `std::os::unix::net::UnixListener`.
Per-plugin socket path: `$MAKAKOO_HOME/run/plugins/<name>.sock`.

PID verification:
- macOS: `LOCAL_PEERPID` socket option
- Linux: `SO_PEERCRED` socket option (`getsockopt` + `ucred` struct)

### 4.2 Windows

Named pipes via `std::os::windows::io::FromRawHandle` + Windows API.
Per-plugin pipe name: `\\.\pipe\makakoo-<name>`.

PID verification: `GetNamedPipeClientProcessId` after `ConnectNamedPipe`.

### 4.3 Redox (future)

Redox uses channel schemes (`chan:`) natively. The capability socket
becomes `chan:makakoo-plugin-<name>` — same primitive, different name.
PID verification is native to Redox's IPC model.

**This is the whole reason we chose Unix sockets over HMAC tokens:** the
enforcement primitive is already a scheme shape that matches Redox's
native IPC, so the port doesn't require rewriting the capability layer.

## 5. Client libraries

Three ship with the kernel:

### 5.1 Rust (`makakoo-client` crate)

```rust
use makakoo_client::Client;

let client = Client::connect_from_env()?;
let recent = client.brain_recent(10, None)?;
let reply = client.llm_chat("minimax/ail-compound", messages)?;
client.state_write("my-data.json", bytes)?;

// Secrets are read through the socket, never via env var.
// The grant must include secrets/read:<KEY_NAME>.
let api_key = client.secret_read("AIL_API_KEY")?;
```

The client reads `MAKAKOO_SOCKET_PATH` from env, opens the socket,
handshakes, and exposes typed methods for each verb.

### 5.2 Python (`makakoo` package)

```python
from makakoo import Client

client = Client.connect_from_env()
recent = client.brain_recent(limit=10)
reply = client.llm_chat("minimax/ail-compound", messages)
client.state_write("my-data.json", data)

# Secret reads go through the capability socket.
with client.secret_read("AIL_API_KEY") as secret:
    # context manager auto-zeroes the value on exit
    make_request(auth=secret.value)
```

Same shape as the Rust client. Install target:
`$MAKAKOO_HOME/plugins/<name>/.venv/bin/python` via `pip install
makakoo` (or embedded in the plugin's venv at install time).

### 5.3 Node (`@makakoo/client` npm package)

```javascript
import { Client } from "@makakoo/client";

const client = await Client.connectFromEnv();
const recent = await client.brainRecent(10);
const reply = await client.llmChat("minimax/ail-compound", messages);
await client.stateWrite("my-data.json", data);
```

### 5.4 Shell / binary plugins

Speak the JSON-RPC protocol directly over the socket, or shell out to
a helper binary:

```sh
$MAKAKOO_BIN/makakoo-client brain-recent --limit 10
```

The helper binary wraps the socket handshake + RPC call and prints
the result.

## 6. Forbidden patterns

These will make the kernel reject a plugin at install time or load time:

1. **Unscoped `fs/read` or `fs/write`** — must have a path glob
2. **Unscoped `exec/binary`** — must have a binary allowlist
3. **Unscoped `secrets/read` or `secrets/write`** — must have a key allowlist
4. **`exec/shell` + `[state.retention] = "keep"`** — a plugin that can
   run arbitrary shell AND keeps state across uninstalls is too dangerous
   for v0.1. (Remove this restriction after sandbox hardening in v0.2.)
5. **Grant list referencing unknown verbs** — typos, misspellings, or
   verbs from future versions
6. **Overlapping scopes that can be simplified** — manifest lint: if
   both `net/http:https://api.example.com/*` and `net/http` are
   present, the unscoped entry dominates; we flag as warning (not error)
   and suggest removing the narrower one

## 7. User-facing capability review

At every plugin install, the user sees a summary of the grants:

```
Installing agent-arbitrage (v0.3.1)
  brain:       read, write
  llm:         chat (minimax/ail-compound only)
  network:     https://clob.polymarket.com/*
               https://data-api.polymarket.com/*
  secrets:     POLYMARKET_API_KEY
  state:       own state dir
  sancho:      arbitrage_tick (every 5m), arbitrage_evening_report (daily)
  mcp:         arbitrage_status, arbitrage_tick_now

Proceed? [y/N]
```

For trusted sources (plugins-core shipped with the kernel), the prompt
is skipped. For community plugins, it's always shown. `--yes` flag
skips for automation use cases.

## 8. Revocation

If a plugin is found to be misbehaving post-install, the user can
revoke capabilities without uninstalling:

```sh
makakoo capability revoke agent-arbitrage net/http
```

This edits the plugin's manifest in place (or maintains an override in
`$MAKAKOO_HOME/config/capability-overrides.toml`), restarts the plugin,
and the new grant table excludes the revoked verb. The plugin might
stop working correctly; that's visible immediately and the user can
uninstall or restore.

**This is v0.2 scope** — v0.1 just ships the grant system + audit log.
Revoke-without-uninstall is a nice-to-have.

## 9. Future verbs (v0.2+)

Not in v0.1 but reserved for future:

- `telemetry/*` — explicit opt-in for plugins that want to report usage
  (rare, user-consent prompt)
- `cloud/*` — outbound to user-configured cloud services (for plugins
  that need to sync state across machines)
- `device/*` — hardware access (camera, microphone, sensors) for
  multimodal plugins
- `share/*` — sharing Brain pages / fragments with other users

All of these will go through the same Unix-socket enforcement primitive.
Adding a new verb is a capability vocabulary bump, not a kernel
architecture change.

## 10. Versioning this doc

**v0.1:** the vocabulary shipped in Phase A (this doc).
**v1.0:** promoted after Phase E when the enforcement layer has run
against at least 20 real plugins with no schema changes needed.

Breaking changes to the vocabulary (renamed verb, removed verb, changed
scope semantics) require a major version bump and a migration path for
existing plugins.

---

**Status:** v0.1 LOCKED. Next review at Phase E when enforcement lands.
