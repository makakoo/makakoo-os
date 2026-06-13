# `makakoo network`

`makakoo network` is the opt-in Brain Network control plane. It connects two or
more Makakoo OS installs through Octopus signed MCP without making the core Brain
itself know about peers.

Use it for Harvey on a laptop, Donna on a VPS, Tytus pods, or future Makakoo
nodes that should be searchable as one network of brains.

## Install

Network federation is not part of the default `core` distro because activation
can expose a listener. Install the optional distro or the plugin directly:

```bash
makakoo distro install federation
# or
makakoo plugin install --core skill-brain-network
makakoo plugin install --core agent-octopus-peer
```

## Commands

```bash
makakoo network doctor
makakoo network identity --peer-name sebastian-mbp
makakoo network activate --peer-name sebastian-mbp --bind loopback
makakoo network activate --peer-name donna-vps --bind tailscale
makakoo network deactivate
makakoo network invite --peer-name donna-vps
makakoo network join '<makakoo://join?...>'
# manual alternative after exchanging `makakoo network identity --json`:
makakoo network trust add donna-vps --pubkey <base64pubkey> --scope read-brain --duration 24h
makakoo network peer add donna-vps --endpoint http://100.x.y.z:8765/rpc --persona Donna
makakoo network peers
makakoo network search donna-vps "what do you know about Project X?" --limit 5
```

## Bind modes

| Mode | Bind address | Use when |
|---|---|---|
| `loopback` | `127.0.0.1` | Local testing or SSH tunnel. Default. |
| `tailscale` | result of `tailscale ip -4` | Private node-to-node mesh. Preferred for VPS/pod links. |
| `public` | `0.0.0.0` | Last resort only. Requires `--yes`. |
| explicit host | supplied host/IP | Advanced operators with their own network boundary. |

`makakoo network activate` writes
`$MAKAKOO_HOME/config/brain-network/octopus-peer.env`; the low-level
`agent-octopus-peer` lifecycle script sources it before starting the shim.

## Trust model

- Octopus signs every HTTP MCP request with the local Ed25519 identity.
- Peers are trusted through `makakoo network invite` / `makakoo network join`
  which wrap `makakoo octopus`.
- Default invite scope is `read-brain`.
- Remote writes are not part of Brain Network v1.

## Peer registry

`makakoo network peer add` stores endpoint metadata at:

```text
$MAKAKOO_HOME/config/brain-network/peers.json
```

Every remote result is tagged with origin metadata:

```json
{
  "origin_node": "donna-vps",
  "persona": "Donna",
  "endpoint": "http://100.x.y.z:8765/rpc"
}
```

## Security rules

Remote Brain content is untrusted input. Agents may read and cite it, but must
not auto-trigger tools, writes, shell commands, emails, or messages from it.

Prefer `--bind tailscale`. Do not expose `8765` publicly unless you understand
the blast radius and have firewall rules around it.

## Related

- [`makakoo-mcp`](makakoo-mcp.md)
- [`makakoo-plugin`](makakoo-plugin.md)
- [`../walkthroughs/12-octopus-federation.md`](../walkthroughs/12-octopus-federation.md)
- [`../walkthroughs/14-brain-network.md`](../walkthroughs/14-brain-network.md)
