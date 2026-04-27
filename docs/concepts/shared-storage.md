# Shared storage (garagetytus)

> Shared S3-compatible object storage for humans and agents. Self-hosted, two reach paths, zero AWS lock-in.

> **Default-bundled in `core` + `sebastian` distros (since 2026-04-27).** The `garage-store` plugin is part of the standard install; `minimal`, `creator`, and `trader` distros opt in via `makakoo plugin install garage-store`. The plugin **soft-fails** when the `garagetytus` binary is absent — your CLI starts cleanly, and `makakoo bucket *` commands print the install hint with exit 127 if you invoke them without the binary. See sprint `2026-04-27-garagetytus-default-bundle.md` for the consensus + ACs.

Makakoo is memory-first — your Brain, journals, plugins, agents all live on your machine. Sometimes you also need to **move bytes between machines**: a file from your Mac to a Tytus pod, an agent's output to a teammate's laptop, a dataset shared between two of your own devices. That's what `garagetytus` is for.

## Why not just SCP / Dropbox / S3?

| Option | Problem |
|---|---|
| `scp` / `rsync` | Pairwise. Doesn't scale to N agents on M machines. No grant model. |
| Dropbox / iCloud | Closed-source, vendor lock-in, no programmatic SigV4 access for agents. |
| AWS S3 | Costs money. Requires an AWS account. Not local. |
| Plain Garage | Open-source S3 daemon, but ships with no install / lifecycle / grants surface — you'd build it yourself. |

`garagetytus` solves the last one: it's a single-binary wrapper around [Garage](https://garagehq.deuxfleurs.fr/) (AGPLv3 upstream daemon) with install, daemon lifecycle, bucket primitives, and a per-grant credential store baked in.

## Two flavors

```
┌───────────────────────────────────────────────────────────────────┐
│                         FLAVOR A — laptop daemon                  │
│                                                                   │
│   your Mac                                                        │
│   ┌────────────────────────────────────────────────────────┐      │
│   │  garagetytus binary  ──supervises──▶  garage child PID │      │
│   │  127.0.0.1:3900  ◀── boto3, rclone, aws-cli, ...       │      │
│   └────────────────────────────────────────────────────────┘      │
│   (no internet exposure, single user, MIT + AGPL clean)           │
└───────────────────────────────────────────────────────────────────┘
```

```
┌───────────────────────────────────────────────────────────────────┐
│                  FLAVOR B — Tytus shared service                  │
│                                                                   │
│   public internet                                                 │
│       │                                                           │
│       ▼                                                           │
│   ┌──────────────────────────────────────────────────────┐        │
│   │  garagetytus.traylinx.com  ◀── Caddy + Let's Encrypt │        │
│   │       │                                              │        │
│   │       ▼                                              │        │
│   │  garagetytus daemon  ──supervises──▶  garage child   │        │
│   │  127.0.0.1:3900   (on Tytus droplet)                 │        │
│   └──────────────────────────────────────────────────────┘        │
│       ▲                                                           │
│       │ HTTPS + SigV4 (per-bucket scoped keys)                    │
│   external clients (your Mac, Tytus pods, CI, third-party SDKs)   │
└───────────────────────────────────────────────────────────────────┘
```

| Property | Flavor A | Flavor B |
|---|---|---|
| Endpoint | `http://127.0.0.1:3900` | `https://garagetytus.traylinx.com` |
| TLS | none (loopback) | Let's Encrypt via Caddy |
| Multi-user | no — single laptop | yes — per-bucket SigV4 keys |
| Reach from Tytus pod | no | yes — same daemon, also reachable at `http://10.42.42.1:3900` from inside a pod |
| Operator | you | Traylinx |
| Cost | $0 (your disk) | bundled with Tytus plan |
| Use case | dev, CI, "S3 in a box" | sharing files between agents on different machines |

Both speak the **same S3 wire protocol**. boto3, aws-cli, rclone, MinIO clients, custom Go SDKs — point them at the relevant endpoint and they work. Only `endpoint_url` and the credentials change.

## Buckets and grants

A **bucket** is a named container of objects. You own it, set TTL + quota at create time, mint grants against it.

A **grant** is a time-limited SigV4 access key + secret key, scoped to **one bucket** + a permission set (`read`, `write`, `read,write`). Grants are how you hand out access — to your own apps, to other agents, to teammates. Default TTL is 1 hour; rotation is the norm.

```sh
# Create a bucket
garagetytus bucket create my-data --ttl 7d --quota 1G

# Mint a grant
garagetytus bucket grant my-data --to "my-app" --perms read,write --ttl 1h

# Inspect
garagetytus bucket info my-data
garagetytus bucket list-grants my-data

# Revoke (idempotent, bypassed by `--keep-data` on uninstall)
garagetytus bucket revoke g_20260426_abc12def
```

Grants live in `~/.garagetytus/grants.json` on the host that minted them. Other Makakoo + Tytus tooling reads this file (read-only) and honours its grants — see `docs/integrate/{makakoo,tytus}.md` in the garagetytus repo.

## Path-style addressing (mandatory)

`garagetytus` locks **path-style** addressing — `http://endpoint/<bucket>/<key>` — and refuses virtual-host style (`http://<bucket>.endpoint/<key>`). This is non-negotiable: it's the only style that works against `127.0.0.1` (no DNS) and against the Tytus public endpoint (Caddy doesn't have a wildcard cert for `*.garagetytus.traylinx.com`).

Every SDK call must include:

```python
config=Config(s3={"addressing_style": "path"})
```

or its native-language equivalent. Virtual-host requests get HTTP 400.

## Health-check semantics

A naked anonymous probe to either endpoint returns:

```text
HTTP/1.1 403 Forbidden
Content-Type: application/xml

<Error><Code>AccessDenied</Code>
<Message>Forbidden: Garage does not support anonymous access yet</Message>
...
```

**This is the healthy response.** Garage requires SigV4 signatures on every request including `/health`. An anonymous probe gets a structured 403 — proof the daemon is up, listening, and responding.

Real outages look like:

- `curl --max-time 5` exits 7 (connection refused) or 28 (timeout)
- HTTP 502 from Caddy (Garage daemon dead behind it)
- boto3 `EndpointConnectionError` / `ConnectTimeoutError`

Symptoms that are **not** outages: `403 AccessDenied`, `403 SignatureDoesNotMatch` (clock skew or wrong key), `404 NoSuchBucket`, empty bucket list.

## AGPL boundary

Garage is AGPLv3-or-later. `garagetytus` is MIT. The boundary holds because Garage runs as a **child process** — `garagetytus` never links against any `garage-*` crate. Three CI gates enforce this on every PR (contract test, AGPL grep, `cargo-deny` resolver ban).

For your app: **as long as you talk to Garage over the S3 wire protocol, you have zero AGPL exposure**. Your code can be MIT, GPL, proprietary, anything. The AGPL fence sits at the subprocess boundary on the host that runs `garagetytus`.

## When to use which flavor

**Use Flavor A (laptop daemon) when:**

- You're developing locally and want S3 with no setup overhead.
- You're running CI that needs an S3-compatible store.
- You want zero internet exposure for the data.
- You're a single user — no need to share with anyone.

**Use Flavor B (Tytus shared service) when:**

- You want to share files between your Mac and a Tytus pod (or between multiple Tytus pods).
- You're handing credentials to an external agent / teammate / CI runner that isn't on your laptop.
- You want a stable HTTPS endpoint that survives laptop sleep / IP changes.
- You don't want to run a daemon yourself.

Mix-and-match is fine: many users run Flavor A for dev, Flavor B for shared/production data.

## Watchdog signals

Both flavors expose:

- `GET /metrics` (Prometheus text format) — disk-free percentage, mode (rw|ro), uptime, watchdog tick timestamp, unclean-shutdown counter.
- `<state-dir>/watchdog.json` — atomic-write JSON mirror for callers that don't speak Prometheus.

For Flavor A, hit `http://127.0.0.1:3903/metrics`. For Flavor B, `/metrics` is intentionally not exposed publicly — Tytus operators monitor it internally.

## Recovery from unclean shutdown

If `garagetytus` (Flavor A) is killed mid-write or the host loses power:

1. The daemon's watchdog detects the missing `sentinel.lock` on next boot.
2. `garagetytus_unclean_shutdown_total` increments.
3. The daemon flips into auto-recovery mode for ~30 seconds: replays the last journal segment, verifies bucket layout, rebuilds the grant index.
4. Status returns to normal; no operator intervention needed for routine cases.

If recovery fails, `garagetytus uninstall --keep-data` + reinstall + re-bootstrap rebuilds config + service files without touching the data dir. Bucket data persists; grants do not (they live in the config dir, not the data dir).

## Pointers

| For | Read |
|---|---|
| Step-by-step recipe | [Walkthrough 13](../walkthroughs/13-shared-storage-garagetytus.md) |
| Full operator manual | [`garagetytus/docs/MANUAL.md`](https://github.com/traylinx/garagetytus/blob/main/docs/MANUAL.md) |
| Public endpoint integration | [`garagetytus/docs/integrate/external-app.md`](https://github.com/traylinx/garagetytus/blob/main/docs/integrate/external-app.md) |
| Troubleshooting | [`garagetytus/skills/garagetytus-troubleshoot/SKILL.md`](https://github.com/traylinx/garagetytus/blob/main/skills/garagetytus-troubleshoot/SKILL.md) |
| Tytus pod context | [Walkthrough 11 — connect a Tytus pod](../walkthroughs/11-connect-tytus.md) |
