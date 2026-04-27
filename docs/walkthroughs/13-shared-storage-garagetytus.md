# Walkthrough 13 — Shared S3 storage with garagetytus

## What you'll do

Put a file into a shared bucket and read it back from a different machine. The bucket is hosted by **garagetytus**, an S3-compatible daemon you can run two ways:

- **Flavor A — local laptop daemon.** One binary, `127.0.0.1:3900`, no internet exposure. For dev, CI, "S3 in a box."
- **Flavor B — Tytus shared service.** One garagetytus daemon hosted at `https://garagetytus.traylinx.com`, multi-tenant with per-bucket SigV4 keys. For sharing files between your Mac, Tytus pods, and other clients without a VPN.

Both flavors speak the same S3 wire protocol — boto3, aws-cli, rclone, MinIO clients all work.

**Time:** about 8 minutes (Flavor A) or 4 minutes (Flavor B if you already have credentials). **Prerequisites:** [Walkthrough 01](./01-fresh-install-mac.md). For Flavor B, a Tytus account with a provisioned bucket.

> **garagetytus is a separate product.** Makakoo works fine without it. This walkthrough is for users who want shared storage between agents and machines. Skip if you don't need it.

## What is garagetytus?

- **[Garage](https://garagehq.deuxfleurs.fr/)** — open-source S3-compatible daemon (AGPLv3). The actual storage engine.
- **garagetytus** — Traylinx's wrapper. Owns install, daemon lifecycle, bucket + grant primitives, and the public HTTPS endpoint.
- **Bucket** — named container of objects. You create them, set TTL + quota, mint per-grant credentials.
- **Grant** — time-limited SigV4 access key scoped to one bucket + one set of permissions (`read,write` etc).

Path-style addressing is mandatory: `http://endpoint/<bucket>/<key>`, never `<bucket>.endpoint`. Region is always `garage`.

---

## Flavor A — local laptop daemon

> **You may already be set up.** If you ran `makakoo install` with the `core` or `sebastian` distro (the default), the `garage-store` plugin is already registered — but the `garagetytus` binary itself is a separate one-line install. The plugin **soft-fails** without the binary (since 2026-04-27), so `makakoo` runs fine; you just can't use `makakoo bucket *` until step 1 below. If you're on `minimal`, `creator`, or `trader`, run `makakoo plugin install garage-store` first.

### 1. Install garagetytus

```sh
curl -fsSL --proto '=https' --tlsv1.2 \
  https://raw.githubusercontent.com/traylinx/garagetytus/main/install/install.sh | bash
```

The installer detects OS/arch, walks a 3-phase plan (env → Garage daemon → garagetytus binary), and offers an interactive first-run wizard. ~3-5 min on first run.

### 2. Start the daemon and bootstrap

```sh
garagetytus install
garagetytus start
garagetytus bootstrap
```

Expected output (last line):

```text
✓ bootstrap complete — service keypair stored in keychain
```

### 3. Create a bucket and mint a grant

```sh
garagetytus bucket create my-data --ttl 7d --quota 1G
GRANT=$(garagetytus bucket grant my-data --to "my-app" \
        --perms read,write --ttl 1h --json)
echo "$GRANT" | jq
```

Expected output:

```json
{
  "grant_id": "g_20260426_abc12def",
  "access_key": "GK6e7b459e9fe995a67e1fca6c",
  "secret_key": "160b5fe40d943794f76e48b082535d972f8ddfaf6bca752a37d09282bbf73610",
  "expires_at": "2026-04-26T15:30:00Z"
}
```

### 4. Put + get an object via boto3

```sh
ACCESS=$(echo "$GRANT" | jq -r .access_key)
SECRET=$(echo "$GRANT" | jq -r .secret_key)

python3 - <<PY
import boto3
from botocore.config import Config

s3 = boto3.client(
    "s3",
    endpoint_url="http://127.0.0.1:3900",
    region_name="garage",
    aws_access_key_id="$ACCESS",
    aws_secret_access_key="$SECRET",
    config=Config(s3={"addressing_style": "path"}),
)
s3.put_object(Bucket="my-data", Key="hello.txt", Body=b"world")
print(s3.get_object(Bucket="my-data", Key="hello.txt")["Body"].read())
PY
```

Expected output:

```text
b'world'
```

### 5. Inspect bucket state

```sh
garagetytus bucket info my-data
garagetytus bucket list-grants my-data
```

### 6. Revoke the grant when done

```sh
garagetytus bucket revoke g_20260426_abc12def
```

---

## Flavor B — Tytus shared service

> **Live since 2026-04-26.** The Tytus team operates one garagetytus daemon on a droplet, fronted by Caddy + Let's Encrypt at [`https://garagetytus.traylinx.com`](https://garagetytus.traylinx.com). Per-bucket SigV4 keys are minted by the Tytus orchestrator at pod-allocation time.

### 1. Get credentials

You receive an `access_key` + `secret_access_key` pair from your Tytus pod allocation (or directly from the Tytus team). They look like:

```text
access_key:    GK6e7b459e9fe995a67e1fca6c
secret_key:    160b5fe40d943794f76e48b082535d972f8ddfaf6bca752a37d09282bbf73610
bucket:        tytus-pod-02-shared
```

Per-user, scoped to one bucket, often time-limited.

### 2. Probe the endpoint (sanity check)

```sh
curl -i --max-time 5 https://garagetytus.traylinx.com/
```

Expected output:

```text
HTTP/2 403
content-type: application/xml

<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>AccessDenied</Code>
<Message>Forbidden: Garage does not support anonymous access yet</Message>
<Resource>/</Resource><Region>garage</Region></Error>
```

> **HTTP 403 with that XML body means the endpoint is healthy.** Garage requires SigV4 signatures for every request. Treat 403 as proof the daemon is up. Real outages look like `curl --max-time 5` exiting 7 (connection refused) or 28 (timeout), or HTTP 502 from Caddy.

### 3. Put + get via boto3

```sh
python3 - <<'PY'
import boto3
from botocore.config import Config

s3 = boto3.client(
    "s3",
    endpoint_url="https://garagetytus.traylinx.com",
    region_name="garage",
    aws_access_key_id="<ACCESS>",
    aws_secret_access_key="<SECRET>",
    config=Config(s3={"addressing_style": "path"}, signature_version="s3v4"),
)
s3.put_object(Bucket="<bucket>", Key="from-mac/hello.txt", Body=b"hi")
for obj in s3.list_objects_v2(Bucket="<bucket>").get("Contents", []):
    print(obj["Key"], obj["Size"])
PY
```

Expected output:

```text
from-mac/hello.txt 2
```

### 4. (Optional) Configure rclone

```sh
rclone config create garagetytus-public s3 \
    provider=Other \
    endpoint=https://garagetytus.traylinx.com \
    region=garage \
    access_key_id=<ACCESS> \
    secret_access_key=<SECRET>

rclone ls garagetytus-public:<bucket>
rclone copy ./local-file.txt garagetytus-public:<bucket>/from-mac/
```

### 5. (Optional) Mint a presigned download URL

For sharing a file with someone who doesn't have credentials:

```python
url = s3.generate_presigned_url(
    "get_object",
    Params={"Bucket": "<bucket>", "Key": "from-mac/hello.txt"},
    ExpiresIn=120,
)
print(url)
```

Anyone with the URL can `curl` the file for 2 minutes. After that, signed-URL expiry kicks in.

---

## What just happened?

- **You moved bytes through a self-hosted S3.** No AWS account, no AWS keys, no IAM. Garage-issued SigV4 keys, scoped per bucket.
- **Path-style addressing is non-negotiable.** Both flavors require `Config(s3={"addressing_style": "path"})`. Virtual-host style (`<bucket>.endpoint/<key>`) returns HTTP 400.
- **Same wire protocol, two reach paths.** Flavor A is local-only (`127.0.0.1:3900`). Flavor B is internet-reachable HTTPS. Your boto3 code is identical apart from `endpoint_url`.
- **Grants are time-limited.** Default TTL is 1 hour; rotation is the norm. If a credential leaks, revoke it with `garagetytus bucket revoke` (Flavor A) or ask Tytus support (Flavor B).

## If something went wrong

| Symptom | Likely cause | Fix |
|---|---|---|
| `boto3.exceptions.EndpointConnectionError` | Daemon not running (Flavor A) or DNS / TCP reach fail (Flavor B) | `garagetytus status` (Flavor A); `curl https://garagetytus.traylinx.com/` should return 403 within 5s. If it times out, that's a real outage. |
| HTTP 400 Bad Request on every call | Virtual-host addressing | Add `Config(s3={"addressing_style": "path"})` — mandatory for both flavors. |
| HTTP 403 SignatureDoesNotMatch | Clock skew >5 min, or wrong secret key | `sntp -sS time.apple.com` to fix Mac clock; double-check the secret key. |
| HTTP 403 AccessDenied on a real request | Grant lacks the perm (e.g. `read` only, you tried `PutObject`), or grant revoked / expired | Mint a fresh grant with `--perms read,write`. |
| HTTP 404 NoSuchBucket | Wrong bucket name, or TTL expired and watchdog cleaned it | `garagetytus bucket info <name>`; recreate if needed. |
| `Forbidden: Garage does not support anonymous access yet` on a curl probe | **This is the healthy response.** | No fix — it means the daemon is up. |

Full troubleshooting matrix: `garagetytus`'s [troubleshoot SKILL](https://github.com/traylinx/garagetytus/blob/main/skills/garagetytus-troubleshoot/SKILL.md).

## Next

- [Concept: shared storage](../concepts/shared-storage.md) — the architecture, the AGPL boundary, the multi-tenant story.
- [Walkthrough 12 — Octopus federation](./12-octopus-federation.md) (stub) — sharing memory across peers, separate from object storage.
