# `agent-octopus-peer`

**Summary:** Octopus signed-MCP peer listener. Runs the Python HTTP shim on
port `8765`, a pool of `makakoo-mcp` stdio workers, and the optional
`harvey-listen.js` loop.

**Kind:** Agent plugin · **Language:** Shell + Python + Node · **Source:**
`plugins-core/agent-octopus-peer/`

## When to use

Use this low-level agent when this host should **receive** signed peer MCP calls.
For normal operators, prefer the safe wrapper:

```bash
makakoo distro install federation
makakoo network activate --peer-name <node> --bind tailscale
makakoo network deactivate
```

The default `core` distro does not activate this listener.

## Start / stop

```bash
makakoo plugin install --core agent-octopus-peer
makakoo agent start agent-octopus-peer
makakoo agent health agent-octopus-peer
makakoo agent stop agent-octopus-peer
```

`makakoo network activate` writes the persistent env file consumed by this agent:

```text
$MAKAKOO_HOME/config/brain-network/octopus-peer.env
```

## State and config

- Identity: `$MAKAKOO_HOME/keys/octopus-identity.json`
- Trust grants: `$MAKAKOO_HOME/keys/trust_store.json`
- Onboarding tokens: `$MAKAKOO_HOME/keys/onboarding/`
- Shim trust file: `$MAKAKOO_HOME/config/peers/trusted.keys`
- Agent logs: `$MAKAKOO_HOME/state/agent-octopus-peer/logs/`

## Network defaults

| Env var | Default | Notes |
|---|---|---|
| `MAKAKOO_MCP_HTTP_BIND` | `127.0.0.1` | Safe loopback default. Use `makakoo network activate --bind tailscale` for private remote access. |
| `MAKAKOO_MCP_HTTP_PORT` | `8765` | Signed MCP HTTP shim port. |
| `HARVEY_LISTEN_INTERVAL_S` | `30` | Listener poll cadence. |

Do not bind to `0.0.0.0` unless you have a firewall boundary and understand the
risk. Ed25519 signatures authenticate the channel; remote Brain content is still
untrusted input.

## Health

```bash
makakoo octopus doctor
makakoo agent health agent-octopus-peer
curl -sS http://127.0.0.1:8765/rpc -X POST -d '{}'
# expected unsigned rejection: X-Makakoo-Peer header required
```

## Capability surface

- `net/http:127.0.0.1:8765` / configured bind address — signed MCP listener.
- `brain/read` and `brain/write` — enforced by Octopus trust scopes.
- `exec/binary:node`, `exec/binary:python3`, `exec/binary:systemctl`,
  `exec/binary:launchctl` — runtime/lifecycle.

## Remove permanently

```bash
makakoo plugin uninstall agent-octopus-peer --purge
```

`--purge` deletes agent state. Revoke Octopus trust explicitly with:

```bash
makakoo octopus trust revoke <peer-name> --reason "removed peer"
```

## Related

- [`makakoo network`](../user-manual/makakoo-network.md)
- [Walkthrough 12 — Octopus signed peer trust](../walkthroughs/12-octopus-federation.md)
- [Walkthrough 14 — Brain Network](../walkthroughs/14-brain-network.md)
