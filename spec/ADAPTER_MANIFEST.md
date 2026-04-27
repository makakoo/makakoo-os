# ADAPTER_MANIFEST.md — v1

**Status:** Locked 2026-04-21 as part of `MAKAKOO-OS-V0.3-UNIVERSAL-BRIDGE` Phase A.
**Schema version:** `manifest_schema = 1`.
**Parser:** `makakoo-core::adapter::manifest`.

The adapter manifest is the single source of truth for how an external AI agent (OpenClaw, hermes, Ollama, OpenRouter, a private pod, a custom CLI, …) plugs into Makakoo OS. One manifest, three consumers:

- **lope validation** — adapter becomes a `Validator` teammate in an ensemble.
- **Harvey delegation** — adapter becomes a `delegate` target for the chat router.
- **SANCHO swarm dispatch** — adapter becomes a `swarm_member` the queue can enqueue against.

Adding a new agent = dropping an `adapter.toml` into `~/.makakoo/adapters/registered/`. Writing Python in the host is an escape hatch reserved for adapters with custom output parsers.

The manifest schema is a PARALLEL format to `plugin.toml`. Plugins describe units of Makakoo capability (skills/agents/sancho-tasks/mcp-tools/mascots). Adapters describe external AI endpoints the bridge can call. Nothing prevents a plugin from shipping an adapter manifest as part of its payload.

---

## 1. File layout

Every adapter ships a single file at the root of its install dir:

```
my-adapter/
├── adapter.toml          # the manifest (this doc)
├── adapter.toml.sig      # Ed25519 signature (required for URL installs)
├── README.md             # optional, human-facing
└── …                      # adapter-specific artefacts (parsers, scripts, …)
```

Registered adapters live at:

```
~/.makakoo/adapters/registered/<name>.toml     # the active manifest
~/.makakoo/adapters/state/<name>/              # per-adapter scratch state
~/.makakoo/trust/adapters.json                 # per-adapter trust ledger
~/.makakoo/trust/keys/<publisher>.pub          # publisher public keys
```

## 2. Canonical schema

```toml
# ─── Identity (required) ─────────────────────────────────────────
[adapter]
name             = "openclaw"                    # kebab-case, unique within $MAKAKOO_HOME/adapters
version          = "1.4.2"                        # semver 2.0
manifest_schema  = 1                              # integer; bump only on breaking schema changes
description      = "OpenClaw agent with NemoClaw sandbox"
homepage         = "https://github.com/traylinx/openclaw"    # optional
license          = "MIT"                          # SPDX id, optional
authors          = ["Traylinx <ops@traylinx.com>"]           # optional

# ─── Host compatibility (required) ───────────────────────────────
[compatibility]
bridge_version   = "^2.0"                         # host-side bridge semver range
makakoo_version  = "^0.3"                         # optional: minimum Makakoo OS version
protocols        = ["openai-chat-v1"]             # at least one of:
                                                  #   openai-chat-v1 | mcp-stdio |
                                                  #   mcp-http | lope-verdict-block |
                                                  #   custom

# ─── How we talk to it (required) ────────────────────────────────
[transport]
kind             = "openai-compatible"            # openai-compatible | subprocess |
                                                  # mcp-stdio | mcp-http
base_url         = "http://127.0.0.1:3000/v1"     # required when kind = openai-compatible
# command        = ["openclaw", "agent", "--message", "{prompt}"]   # required when kind = subprocess
# stdin          = false                           # optional: send prompt via stdin instead of argv
# url            = "https://pod.example/mcp"       # required when kind = mcp-http
# model          = "ail-compound"                  # optional: default model to pass through

# ─── Auth (required if the transport needs it) ───────────────────
[auth]
scheme           = "bearer"                       # bearer | header | basic | none | oauth
key_env          = "OPENCLAW_API_KEY"             # env var name — NEVER hardcode the secret
# header_name    = "X-Custom-Auth"                 # required when scheme = header
# user_env       = "OPENCLAW_USER"                 # required when scheme = basic
# pass_env       = "OPENCLAW_PASS"                 # required when scheme = basic

# ─── Output contract (required) ──────────────────────────────────
[output]
format           = "lope-verdict-block"           # lope-verdict-block | openai-chat | plain | custom
# parser         = "./parsers/my_parser.py:parse"  # required when format = custom
# verdict_field  = "choices.0.message.content"     # required when format = openai-chat;
                                                   # dot-path into JSON response

# ─── Capabilities (locked at trust time) ─────────────────────────
[capabilities]
models           = ["ail-compound", "ail-image"]  # allowlist; empty vec = discover at runtime
features         = ["tool_use", "streaming", "vision"]       # free-form tags
max_context      = 200000                         # tokens
rate_limit_rpm   = 600                            # optional
supports_roles   = ["validator", "delegate", "swarm_member"]

# ─── Install source (required for URL installs) ──────────────────
[install]
source_type      = "git"                          # local | git | https-tarball | pypi | npm | binary
source           = "https://github.com/traylinx/openclaw-adapter.git"
ref              = "v1.4.2"                       # semver tag OR 40-char commit SHA; NEVER a branch
sha256           = "a7f2…e9"                     # required when source_type ∈ {https-tarball, binary}
entry_point      = "./bin/openclaw"               # optional: resolved inside the install dir
setup_cmd        = ["./install.sh"]               # optional one-time install hook (sandboxed)

# ─── Security (locked at trust time) ─────────────────────────────
[security]
requires_network     = true
allowed_hosts        = ["api.openrouter.ai", "127.0.0.1"]      # default DENY; allowlist only
requires_filesystem  = ["read:cwd", "write:$MAKAKOO_HOME/state/adapters/openclaw"]
requires_env         = ["AIL_API_KEY", "OPENCLAW_API_KEY"]
sandbox_profile      = "network-io"               # none | network-io | fs-read | isolated
signed_by            = "traylinx"                  # publisher key id at ~/.makakoo/trust/keys/<id>.pub

# ─── Health probe (optional, recommended) ────────────────────────
[health]
check_url        = "http://127.0.0.1:3000/v1/models"
timeout_ms       = 5000
expected_field   = "data.0.id"                    # dot-path — presence check, not equality
```

## 3. Field-by-field

### 3.1 `[adapter]` identity

| Field              | Required | Type     | Constraint                                                           |
| ------------------ | -------- | -------- | -------------------------------------------------------------------- |
| `name`             | yes      | string   | `^[a-z][a-z0-9-]{1,62}$` — kebab-case, globally unique               |
| `version`          | yes      | string   | semver 2.0                                                           |
| `manifest_schema`  | yes      | int      | currently `1`; host rejects higher values                             |
| `description`      | yes      | string   | 1-line human-facing summary                                          |
| `homepage`         | no       | string   | URL                                                                  |
| `license`          | no       | string   | SPDX id (e.g. `MIT`, `Apache-2.0`)                                   |
| `authors`          | no       | [string] | free-form (e.g. `"Name <email>"`)                                    |

### 3.2 `[compatibility]`

| Field             | Required | Type     | Constraint                                                                       |
| ----------------- | -------- | -------- | -------------------------------------------------------------------------------- |
| `bridge_version`  | yes      | string   | semver range (e.g. `^2.0`)                                                       |
| `makakoo_version` | no       | string   | semver range                                                                     |
| `protocols`       | yes      | [string] | non-empty, each ∈ {`openai-chat-v1`, `mcp-stdio`, `mcp-http`, `lope-verdict-block`, `custom`} |

### 3.3 `[transport]`

| Field      | Required | Type     | Notes                                                                       |
| ---------- | -------- | -------- | --------------------------------------------------------------------------- |
| `kind`     | yes      | string   | enum: `openai-compatible` \| `subprocess` \| `mcp-stdio` \| `mcp-http`      |
| `base_url` | cond     | string   | required when `kind = openai-compatible`; must start with `http://` or `https://` |
| `command`  | cond     | [string] | required when `kind ∈ {subprocess, mcp-stdio}`; `{prompt}` substituted at call time |
| `stdin`    | no       | bool     | `subprocess` only; when true, prompt is passed via stdin                    |
| `url`      | cond     | string   | required when `kind = mcp-http`                                              |
| `model`    | no       | string   | default model name passed through to OpenAI-compat requests                 |

### 3.4 `[auth]`

| Field         | Required | Type   | Notes                                                                    |
| ------------- | -------- | ------ | ------------------------------------------------------------------------ |
| `scheme`      | yes      | string | enum: `bearer` \| `header` \| `basic` \| `none` \| `oauth` (oauth is opaque, driven by custom parser) |
| `key_env`     | cond     | string | env var name, required for `bearer` and `header`                         |
| `header_name` | cond     | string | required for `header`                                                    |
| `user_env`    | cond     | string | required for `basic`                                                     |
| `pass_env`    | cond     | string | required for `basic`                                                     |

**The manifest stores env var NAMES, never values.** The bridge resolves env vars at call time inside the adapter sandbox.

### 3.5 `[output]`

| Field           | Required | Type   | Notes                                                                |
| --------------- | -------- | ------ | -------------------------------------------------------------------- |
| `format`        | yes      | string | enum: `lope-verdict-block` \| `openai-chat` \| `plain` \| `custom`    |
| `parser`        | cond     | string | `module.py:fn` form — required when `format = custom`                |
| `verdict_field` | cond     | string | dot-path — required when `format = openai-chat`                       |

### 3.6 `[capabilities]`

| Field            | Required | Type     | Notes                                                              |
| ---------------- | -------- | -------- | ------------------------------------------------------------------ |
| `models`         | no       | [string] | allowlist; empty = discover at runtime                             |
| `features`       | no       | [string] | free-form tags (e.g. `tool_use`, `vision`, `streaming`, `fs_write`) |
| `max_context`    | no       | int      | tokens                                                             |
| `rate_limit_rpm` | no       | int      | advisory, the host doesn't enforce                                 |
| `supports_roles` | yes      | [string] | non-empty, subset of {`validator`, `delegate`, `swarm_member`}     |

### 3.7 `[install]`

| Field         | Required | Type     | Notes                                                                           |
| ------------- | -------- | -------- | ------------------------------------------------------------------------------- |
| `source_type` | yes      | string   | enum: `local` \| `git` \| `https-tarball` \| `pypi` \| `npm` \| `binary`         |
| `source`      | cond     | string   | required unless `source_type = local`; URL or path                              |
| `ref`         | cond     | string   | required when `source_type = git`; must match `^v?\d+\.\d+\.\d+…$` or `^[a-f0-9]{40}$` |
| `sha256`      | cond     | string   | required when `source_type ∈ {https-tarball, binary}`; 64 lowercase hex chars    |
| `entry_point` | no       | string   | relative path inside install dir                                                |
| `setup_cmd`   | no       | [string] | runs once, sandboxed, at install time                                           |

Branch names are **explicitly rejected** as `ref` values. Typosquat + branch-move attacks foiled at parse time.

### 3.8 `[security]`

| Field                 | Required | Type     | Notes                                                              |
| --------------------- | -------- | -------- | ------------------------------------------------------------------ |
| `requires_network`    | yes      | bool     |                                                                    |
| `allowed_hosts`       | cond     | [string] | required when `requires_network = true`; default-deny allowlist    |
| `requires_filesystem` | no       | [string] | items like `read:<path>` or `write:<path>`; `$MAKAKOO_HOME` expansion |
| `requires_env`        | no       | [string] | env vars the adapter needs at call time                            |
| `sandbox_profile`     | yes      | string   | enum: `none` \| `network-io` \| `fs-read` \| `isolated`; default `network-io` |
| `signed_by`           | cond     | string   | publisher key id; required for URL installs                        |

### 3.9 `[health]`

| Field            | Required | Type   | Notes                                                |
| ---------------- | -------- | ------ | ---------------------------------------------------- |
| `check_url`      | no       | string | URL hit during install-time sandboxed health check   |
| `timeout_ms`     | no       | int    | default 5000                                         |
| `expected_field` | no       | string | dot-path whose presence in the JSON response is asserted |

## 4. Canonical hashing

`Manifest::canonical_hash()` returns the sha256 of a deterministic serialization of the parsed manifest:

1. Every struct is serialized via `serde_json::to_value`.
2. Object keys are emitted in sorted order (serde_json default on BTreeMap, we normalize elsewhere).
3. The resulting JSON is hashed with sha256.

The same manifest on any machine produces the same hash. The trust ledger stores this hash at trust time; any change on update triggers re-prompt.

## 5. Backward compatibility rules

- **Adding a new optional field** in schema `v1` (e.g. a new sub-field in `[capabilities]` that doesn't invalidate old manifests) does NOT bump `manifest_schema`.
- **Removing a field, changing its semantics, or making an optional field required** bumps `manifest_schema` to `2`. v1 parsers reject v2 manifests cleanly.
- **Adding a new value to an enum** (e.g. a 5th transport kind) bumps `manifest_schema` because old parsers can't understand it.
- **The `[health]` block, the `ref` format rules, and the signing contract** are considered semver-governed: any breaking change requires a schema bump.

## 6. What v1 does NOT include

Intentionally deferred:

- `[session]` block — stateful session TTL like the legacy OpenClawBridge. Left for v0.4 if a real use-case bites. The new bridge is stateless by default.
- `[quota]` block — per-adapter metered billing. v0.4+ observability phase.
- `[gateway]` block — routing multiple upstream providers behind one adapter.
- `[events]` block — adapter-emitted events feeding the Makakoo event bus.

These can be added in future without bumping `manifest_schema` because they're additive optional blocks.

## 7. Rationale pointers

Every field choice traces back to a specific precedent or incident:

- `manifest_schema` int — HuggingFace learned the hard way that schema evolution must be orthogonal to adapter versioning.
- `compatibility.bridge_version` semver range — npm `engines` + VS Code `engines.vscode` pattern. Refuses incompatible adapters at install, not at runtime.
- `install.ref` branch-rejection — PyPI typosquat + Ollama branch-move attacks.
- `install.sha256` required for non-git — Ollama pull CVE class.
- `security.allowed_hosts` default-deny — Anivia/OctoRAT 2025-11 post-compromise C2 beaconing.
- `security.sandbox_profile` default `network-io` — MCP `0.0.0.0`-bind CVE class.
- `security.signed_by` Ed25519 — GlassWorm 2026-03 (VS Code ext), Cursor CVE-2025-54136.
- Re-prompt on `capabilities.*` or `security.*` diff — direct Cursor CVE mitigation.

## 8. Consumer contract

Any adapter that declares `supports_roles = [..., "validator", ...]` can be used by lope as:

```
lope negotiate --validators claude,<adapter-name> …
```

Any adapter with `supports_roles = [..., "delegate", ...]` can be used by the Harvey chat router when `HARVEY_ROUTER_USE_ADAPTERS=1`.

Any adapter with `supports_roles = [..., "swarm_member", ...]` can be used by the swarm dispatch queue:

```
makakoo swarm enqueue agent <adapter-name> "<prompt>" --adapter <adapter-name>
```

Roles not listed in `supports_roles` are unavailable to that consumer even if the adapter technically could serve them — the manifest is the allowlist, not a capability hint.
