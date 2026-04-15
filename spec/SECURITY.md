# SECURITY — Threat Model & Defenses

**Status:** v0.1 LOCKED — 2026-04-15
**Owner:** Makakoo kernel, `crates/core/src/security/`
**Governs:** what Makakoo OS protects against, what it explicitly
doesn't, and how each defense is implemented.

---

## 0. One-sentence summary

Makakoo OS is a **capability-declared honesty boundary** running as the
user on their machine — not a sandbox. It defends against supply chain
tampering, plugin misbehavior bugs, secret leakage, and infection
permanence bugs. It does NOT defend against a malicious plugin author
who wants to exfiltrate data (they can always shell out to `curl`), OS
privilege escalation (we don't run privileged), or network
man-in-the-middle attacks (we rely on HTTPS).

## 1. The three questions

Every security decision in Makakoo gets filtered through three questions:

1. **Does this decision serve the Genie promise?** (Harvey's soul lives
   on the user's machine and nowhere else — this is the promise)
2. **Would a well-meaning plugin author understand this decision?** (If
   yes, it's an honesty boundary. If no, it's too invasive.)
3. **Does this decision cost us the ability to upgrade to a real sandbox
   later?** (If yes, reject. Every v0.1 defense must survive the v0.2+
   OS-isolation upgrade.)

## 2. Threat model

### 2.1 What we defend against

| Threat | Example | Our defense |
|---|---|---|
| **Supply chain tampering** | A plugin is modified between publish and install (GitHub account hijack, DNS poisoning, MITM) | blake3 hash pinning in distro files. Kernel refuses install if hash doesn't match |
| **Well-meaning plugin misbehavior** | A plugin accidentally logs its `AIL_API_KEY` to stdout | Secrets never cross plugin env — delivered through the capability socket on demand |
| **Plugin overreach** | A trading plugin accidentally writes to the user's Documents folder | `fs/write` requires explicit path glob. Default state dir is the only writable path |
| **Infection permanence bugs** | Bootstrap Block stuck after plugin uninstall | Every infection backed up. `uninfect` restores backup. Audit log tracks every change |
| **Secret leakage to child subprocesses** | A plugin forks a child and leaks its capability token to the child's env | Unix domain socket + PID verification — no env-var tokens to leak (D11) |
| **Plugin crash corrupting state** | A plugin crashes mid-write and leaves half-written files | Atomic writes (tmpfile + rename) in every state-writing helper |
| **Two plugins conflicting** | Two plugins both try to name the same bootstrap fragment | D14 — strict append-order + conflict refusal at install time |
| **Malicious distro file** | Community-shared distro with tampered plugin hashes | User-facing capability review before install. Unknown distros default-deny |

### 2.2 What we don't defend against

| Non-threat | Why we don't defend |
|---|---|
| **Malicious plugin author** | A plugin that wants to exfiltrate can shell out to `curl` and bypass the entire capability system. Users must audit plugin sources before installing. We're an honesty boundary, not a sandbox. |
| **OS privilege escalation** | We run as the user. We never use sudo. A bug in the kernel cannot escalate beyond user permissions. |
| **Network MITM** | We rely on HTTPS to verify endpoints. If the user's network is compromised at the TLS level, we can't save them. |
| **Compromised host OS** | If the user's macOS is rooted, Makakoo cannot protect Harvey's Brain. Full-disk encryption is the user's responsibility. |
| **Hardware keylogger** | Out of scope. |
| **Physical access** | Someone who can sit at the user's keyboard can do anything the user can. Out of scope. |

## 3. Supply chain defenses

### 3.1 blake3 hash pinning

Every plugin listed in a distro file is pinned by `blake3` hash. When
the kernel downloads a plugin, it computes the hash of the downloaded
tree and compares. Mismatch → refusal with a clear error.

**Hash algorithm:** blake3 (faster than sha256, safer than sha1).

**What's hashed:**
- Walk plugin tree in lexicographic order, excluding `.git/`, `target/`,
  `node_modules/`, `__pycache__/`, `.venv/`
- For each file: hash `(relative_path, blake3(content))`
- Combine all per-file hashes into the tree hash

**Deterministic:** two independent downloads of the same git tag
produce the same hash.

### 3.2 Distro file signing (v0.2)

Not in v0.1. In v0.2+ we'll sign distro files with a kernel-team GPG
key and verify at install time. This catches the "compromised
distros server" case. For v0.1, blake3 pinning alone is enough because
distro files themselves are small and users can review them by eye.

### 3.3 Source verification at install

When installing from git:
1. Clone the pinned rev
2. Compute the tree hash
3. Compare to the distro's declared hash
4. Refuse if mismatch

When installing from a local path:
- No hash check at install time (local paths are user-owned)
- But `makakoo plugin list` displays the current tree hash so users
  can verify it matches the one they'd pin in a distro file

### 3.4 Plugin author identity

v0.1 does not verify plugin authors cryptographically. The `authors`
field in `plugin.toml` is informational. Users are expected to review
the source (or trust the distro file's curator) before installing.

v0.2+ could add GPG signature verification of commits. Deferred.

## 4. Runtime defenses

### 4.1 Unix domain socket + PID verification (D11)

Every plugin gets a per-plugin socket at
`$MAKAKOO_HOME/run/plugins/<name>.sock` (named pipe on Windows). At
plugin start:

1. Kernel creates socket
2. Kernel spawns plugin with `MAKAKOO_SOCKET_PATH=<socket>` in env
3. Plugin's client library connects
4. Kernel reads the connecting PID via `getpeereid`/`SO_PEERCRED`/
   `LOCAL_PEERPID`/`GetNamedPipeClientProcessId`
5. Kernel verifies the PID matches the spawned child
6. Mismatch → refuse. Match → grant-table session established

**Why this beats HMAC tokens:**
- Nothing to leak (no token exists)
- Plugin's forked children have different PIDs, fail the check
- Socket lifetime = plugin lifetime; no revocation needed
- Maps 1:1 to Redox channel schemes for the future port

### 4.2 Capability enforcement

See `CAPABILITIES.md` for the full verb vocabulary and enforcement
semantics. Summary:

- Every plugin declares `[capabilities].grants` in its manifest
- Grants are parsed into a per-session grant table at plugin start
- Every helper call (brain, llm, net, state, secrets) goes through the
  socket and the kernel checks the grant table
- Denied calls return a JSON-RPC error, are audit-logged, and do not
  serve data

### 4.3 Audit log

**Path:** `$MAKAKOO_HOME/logs/audit.jsonl`
**Rotation:** 100 MB per file, archived as `audit.jsonl.<timestamp>.gz`,
default 7-day retention, configurable via
`$MAKAKOO_HOME/config/audit.toml`

**Format:** one JSON object per line. Schema:

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

**User-facing:** `makakoo audit` prints recent entries. `makakoo audit
--denied` filters to denials (debug misconfigured manifests).

### 4.4 Plugin isolation (subprocess model)

Every plugin runs as a separate child process (D5). Crashes are
isolated — one plugin falling over doesn't crash the daemon or other
plugins. The SANCHO scheduler restarts crashed plugins with exponential
backoff.

**No dynamic library loading.** A plugin cannot corrupt the kernel's
memory space because it doesn't share one.

## 5. Secret management

### 5.1 OS keyring

API keys, tokens, passwords live in the OS-native keyring:
- **macOS:** Keychain Services
- **Linux:** Secret Service (libsecret) — fallback to encrypted file if
  Secret Service unavailable (headless systems)
- **Windows:** Credential Manager

**Library:** `keyring` crate handles all three.

### 5.2 User interface

```sh
makakoo secret set AIL_API_KEY       # prompts for value via stdin, never echoes
makakoo secret get AIL_API_KEY       # prints to stdout
makakoo secret delete AIL_API_KEY
makakoo secret list                   # names only, never values
```

Already shipped in v0.0 — documented here as the canonical flow.

### 5.3 Plugin access

Plugins declare `secrets/read:AIL_API_KEY` in their manifest. At runtime,
the plugin calls `client.secret_read("AIL_API_KEY")` through the socket.
The kernel:
1. Verifies the plugin has that specific key in its grant list
2. Reads the secret from the keyring
3. Returns it via the socket (never via env var)
4. Logs the access

**Key never crosses env boundaries.** No `MAKAKOO_AIL_API_KEY` env var.
No `.env` file. The only time the secret touches memory outside the
keyring is in the plugin's own address space, and only at the moment of
use.

### 5.4 Secret lifetime in plugin memory

Best practice (not enforced): plugins should zero secrets from memory
after use. We can't enforce this without dynamic analysis, but the
client library documents the pattern and ships helpers:

```rust
let secret = client.secret_read("AIL_API_KEY")?;
// ... use secret ...
secret.zero();                        // explicit zeroization
```

```python
with client.secret_read("AIL_API_KEY") as secret:
    # secret is a context manager, auto-zeroized on exit
    make_request(auth=secret.value)
```

## 6. Infection safety

### 6.1 Backup before every infection

See `PARASITE.md §2 Step 1`. Every infection backs up the host's
global instructions file to
`$MAKAKOO_HOME/infect/backups/<host>/<timestamp>/<filename>` before
writing the Bootstrap Block. Backups are keyed by timestamp so multiple
infections accumulate.

**Retention:** indefinite by default. Users can prune via `makakoo
infect clean-backups --older-than 90d`.

### 6.2 Atomic writes

All host file writes use `write to tmpfile + fsync + rename`. A crash
mid-write leaves the original file intact.

### 6.3 Reversibility

`makakoo uninfect <host>` restores the most recent backup and removes
our symlinks + MCP server entry. `makakoo uninfect --all` does this for
every infected host. No manual cleanup required.

### 6.4 Audit log for infections

`$MAKAKOO_HOME/logs/infect.jsonl` records every infect/uninfect/refresh
event with timestamp, host, file path, and a hash of what was written.
Separate from the capability audit log so infection history stays
visible even if capability logs rotate.

## 7. Network boundary

### 7.1 Daemon listens on localhost only

The Makakoo daemon binds its MCP gateway to `127.0.0.1` and its plugin
sockets to Unix sockets (not TCP). **No TCP binds to 0.0.0.0, ever.**

This is enforced by a CI check that greps for `bind` calls with
non-localhost addresses in the kernel source.

### 7.2 Plugin network calls through capability helpers

Plugins that need HTTP/TCP/UDP go through `client.http_get()` etc.
which checks the `net/http:<glob>` grant before serving. Unauthorized
or scope-mismatched calls return an error.

### 7.3 No cloud services

Makakoo does not have a cloud backend. There is no `api.makakoo.com`.
There is no telemetry endpoint. There is no usage counter. Plugins
can opt in to cloud services, but the kernel itself never makes a
network call except:
- Fetching plugin sources from declared git/tar URLs at install time
- Fetching LLM responses via user-configured providers (local or remote)
- Fetching update manifests (v0.2+, opt-in)

### 7.4 Telemetry CI check (D17)

Grep-based CI check that fails if any Rust file outside
`crates/platform/` contains network-sending calls not routed through
the capability helper layer. Catches accidental telemetry before it
ships.

```sh
# Example CI command
rg 'reqwest::Client|ureq::get|TcpStream::connect' crates/core crates/cli \
  | grep -v '// expected' && exit 1 || exit 0
```

## 8. Local-first is sacred (D17)

Harvey's memory of the user NEVER leaves the user's machine unless
they explicitly opt in to a specific export. This is enforced by:

1. **No cloud default.** Fresh install stores everything locally. No
   "sync to cloud" toggle in setup.
2. **No telemetry.** CI-enforced (see 7.4).
3. **No training data uploads.** The kernel has no code path that sends
   conversation data anywhere.
4. **Export is explicit.** `makakoo harvey export` produces a local
   tarball. The user moves it manually (USB drive, their own S3,
   whatever). The kernel doesn't push anywhere.
5. **Cloud sync is a plugin.** If a user wants cross-device sync, they
   install a plugin that declares `net/http` to their own provider.
   Plugin installation shows the capability grants; user sees exactly
   where their data would go.

## 9. Trust model

### 9.1 What the user trusts

- **The Makakoo kernel binary** (compiled from signed source, distributed
  via signed artifacts)
- **The core distro** (`plugins-core/`, curated by the kernel team)
- **Any community plugin they choose to install** (reviewed at install
  time, hash-pinned)

### 9.2 What the kernel trusts

- **The user** (runs as their uid, does what they ask)
- **The OS keyring** (via `keyring` crate)
- **Signed release artifacts** (via Apple notarization on macOS,
  Authenticode on Windows, GPG-signed deb/rpm on Linux)

### 9.3 What the kernel DOESN'T trust

- **Plugins** (even core ones are capability-scoped)
- **Network responses** (TLS verified, response bodies validated against
  declared schemas where applicable)
- **User input** (path traversal checks, escape all shell invocations)
- **Infected host files** (read as opaque strings, modify only between
  sentinels)

## 10. Known gaps (to fix in v0.2+)

1. **Distro file signing** — user trusts whoever curated the distro file
   visually. v0.2 should add GPG signing.
2. **Plugin author identity** — no cryptographic verification that the
   plugin really came from the declared author.
3. **OS-level sandbox** — v0.1 is honesty-boundary only. v0.2+ should
   add Linux namespaces + seccomp, macOS sandbox-exec, Windows
   AppContainer per plugin process.
4. **Egress firewall** — plugins can `exec/binary:curl` to bypass `net/
   http` scoping. v0.2+ should firewall the plugin subprocess at the OS
   level.
5. **State integrity** — we don't checksum plugin state dirs. A plugin
   that crashes mid-write can leave torn files. v0.2 should add
   per-plugin state journaling.
6. **Audit log integrity** — the audit log is plain JSONL. A plugin with
   `fs/write` to the logs dir could tamper with its own history. v0.2
   should lock the log to kernel-only writes.

All six are on the v0.2 roadmap but do not block v0.1 ship.

## 11. Security incidents process

If a security bug is found in a shipped version:

1. **Private disclosure:** `security@makakoo.com` (or whatever the
   project email becomes). GPG-encrypted report preferred.
2. **Assessment within 72h:** triage the report, reproduce, score
   severity.
3. **Fix within 14 days** for high/critical severity. 30 days for
   medium. 90 days for low.
4. **Coordinated disclosure:** CVE if warranted, advisory in
   GitHub Security tab, blog post + tweet at release.
5. **Patch release:** kernel version bump, new signed artifacts, update
   install one-liner.

Users on the affected version see a warning on next daemon start
pointing at the advisory.

## 12. Versioning this doc

**v0.1:** current threat model (this doc).
**v1.0:** after Phase E + capability enforcement is battle-tested.

Changes to the threat model are not versioned at the doc level; they
update the current doc in place with a "Changed" section at the top.

---

**Status:** v0.1 LOCKED. Next review at Phase E when capability
enforcement lands.
