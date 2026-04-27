# `makakoo adapter` — CLI reference

Adapters are the routing layer between Makakoo agents and the LLM backends
they call. Each adapter is a small manifest (`adapter.toml`) that tells
Makakoo how to reach a backend: `openai-compat` for any OpenAI-compatible
gateway, `subprocess` for a local model wrapper, `mcp-stdio` for another
MCP server, or `peer-makakoo` for a federated Makakoo install. Once
installed and enabled, agents, lope validators, and the swarm route through
it by name. The `model-provider` section of `makakoo setup` picks which
adapter is the primary.

## Subcommand overview

| Subcommand | Purpose |
|---|---|
| `adapter list [--json] [--include-bundled]` | All registered adapters. `--include-bundled` also shows shipped-but-not-installed reference adapters. |
| `adapter info <name>` | Parsed manifest + canonical hash. |
| `adapter spec` | Dump the full `adapter.toml` schema to stdout. |
| `adapter install <source> [--bundled] [--pack]` | Install from a local dir or a bundled reference. `--pack` installs every `<subdir>/adapter.toml` under a directory. |
| `adapter update <name>` | Re-run install against the current recorded source (detects capability drift). |
| `adapter remove <name> [--purge]` | Remove. `--purge` also wipes the adapter's state dir. |
| `adapter enable <name>` | Re-enable a soft-disabled adapter. |
| `adapter disable <name>` | Soft-disable without removing. |
| `adapter status` | Table of last-call outcome, timestamp, and last error across all adapters. |
| `adapter doctor <name> [--json]` | Env check + auth smoke + health check + signature verify, each with a remediation hint. |
| `adapter search <term>` | Fuzzy name filter across registered + bundled adapters. |
| `adapter call <name> [--prompt "..."] [--timeout N]` | Call an adapter with a prompt; returns a `ValidatorResult` JSON — the lope interop seam. |
| `adapter migrate-config <file>` | Convert legacy `~/.lope/config.json` provider entries into `.toml` manifests. |
| `adapter export <name> [--sign]` | Dump a signed tarball (`adapter.toml` + `.sig`) for sharing with peers. |
| `adapter trust` | Manage the Ed25519 peer trust store (`$MAKAKOO_HOME/config/peers/trusted.keys`). |
| `adapter self-pubkey` | Print this install's Ed25519 public key (generates keypair on first call). |
| `adapter gen --template <T> --name <N> [flags]` | Scaffold a new adapter from a template and install it. Templates: `openai-compat`, `subprocess`, `mcp-stdio`, `peer-makakoo`. |

## Key use patterns

### Install the bundled switchAILocal adapter and test it

```sh
# see which bundled adapters ship with this version
makakoo adapter list --include-bundled

# install the switchAILocal reference adapter
makakoo adapter install switchailocal --bundled

# verify env, auth, and health in one shot
makakoo adapter doctor switchailocal

# send a test prompt
echo "ping" | makakoo adapter call switchailocal
```

### Scaffold a custom OpenAI-compatible adapter

```sh
# scaffold, install, and run doctor in one command
makakoo adapter gen \
  --template openai-compat \
  --name my-gateway \
  --url http://localhost:18080/v1 \
  --key-env MY_GATEWAY_API_KEY \
  --model ail-compound

# confirm the adapter is live and healthy
makakoo adapter status
```

## Related commands

- [`makakoo-mcp.md`](makakoo-mcp.md) — MCP stdio server that adapters can back
- [`makakoo-plugin.md`](makakoo-plugin.md) — plugins that ship bundled adapters
- [`makakoo-infect.md`](makakoo-infect.md) — infect injects adapter refs into CLI hosts
- [`../adapter-publishing.md`](../adapter-publishing.md) — full adapter authoring guide
- [`../adapters.md`](../adapters.md) — concept overview and bundled adapter catalog

## Common gotcha

**`adapter doctor` fails on the auth smoke step even though the env var is set.**
The most common cause: the env var exists in your shell session but not in the
daemon's environment (the LaunchAgent is launched at login, before your shell
profile runs). Fix: store the key with `makakoo secret set <KEY_NAME>` and
reference it as `secret_ref` in the adapter manifest. The daemon reads the OS
keyring directly without needing the env var to be present.
