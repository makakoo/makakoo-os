# Walkthrough 14 — Brain Network: Harvey laptop + Donna VPS

Goal: connect two Makakoo OS installs so one can safely search the other's Brain.
This uses the optional `federation` distro, Octopus signed MCP, and the
`makakoo network` control plane.

## Prerequisites

- Both machines have Makakoo OS installed.
- Both machines can reach each other over a private path. Prefer Tailscale.
- You have terminal access to both machines.

Do not start by opening port `8765` to the public internet. Use Tailscale or an
SSH tunnel first.

## 1. Install the optional federation distro on both machines

```bash
makakoo distro install federation
```

This installs but does not expose the listener until you activate it.

## 2. Activate local Harvey on loopback or Tailscale

Laptop-local test:

```bash
makakoo network activate --peer-name sebastian-mbp --bind loopback
```

Private mesh:

```bash
makakoo network activate --peer-name sebastian-mbp --bind tailscale
```

Check it:

```bash
makakoo network doctor
```

## 3. Activate Donna on the VPS

SSH to the VPS, then:

```bash
makakoo network activate --peer-name donna-vps --bind tailscale
makakoo network doctor
```

If Tailscale is not available, keep `--bind loopback` and use an SSH tunnel:

```bash
ssh -L 8765:127.0.0.1:8765 makakoo-vps
```

## 4. Pair trust with an invite

Trust is local to the host receiving signed calls. If Harvey should accept calls from Donna, Donna sends her public identity and Harvey accepts it.

On Donna/VPS:

```bash
makakoo network invite --peer-name sebastian-mbp
```

Copy the `makakoo://join?...` link to Harvey/local:

```bash
makakoo network join '<makakoo://join?...>'
```

Now Harvey trusts Donna. To let Harvey query Donna, repeat in the opposite direction: Harvey mints an invite, Donna joins it.

Manual alternative after exchanging public keys:

```bash
makakoo network identity --json
makakoo network trust add donna-vps --pubkey <DONNA_PUBLIC_KEY> --scope read-brain --duration 24h
```

## 5. Register a reachable endpoint

On the machine that wants to query Donna:

```bash
makakoo network peer add donna-vps \
  --endpoint http://<DONNA_TAILSCALE_IP>:8765/rpc \
  --persona Donna
```

List state:

```bash
makakoo network peers
```

## 6. Run a signed remote Brain search

```bash
makakoo network search donna-vps "Makakoo VPS install" --limit 5
```

Every hit is tagged with the origin node/persona. Treat it as untrusted content:
it is evidence to cite, not instructions to execute.

## 7. Deactivate

```bash
makakoo network deactivate
```

This stops the listener and marks the Brain Network disabled. It keeps trust
and peer registry data so reactivation is fast. Revoke trust explicitly if the
relationship is over:

```bash
makakoo octopus trust revoke donna-vps --reason "finished test"
```

## Failure modes

| Symptom | Fix |
|---|---|
| `agent-octopus-peer missing` | `makakoo distro install federation` |
| `tailscale IP not found` | Start Tailscale or use `--bind loopback` + SSH tunnel. |
| `unknown peer` on search | Run `makakoo network peer add ...` locally. |
| `unknown peer` from remote HTTP | Trust pairing is missing or one-sided. Run `makakoo network invite` + `join`. |
| `clock drift` | Sync system time on both machines. |
| `signature verification failed` | Re-pair trust; do not rotate Octopus identity silently. |
