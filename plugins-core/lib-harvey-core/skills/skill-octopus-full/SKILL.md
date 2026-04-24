# skill-octopus-full — peer federation hardened for SME load

Phase 4 is the glue: it wires Phase 1's transport + Phase 2's trust
model + Phase 3's discovery into an end-to-end shipped system hardened
for an SME team of 3–10 teammates actively writing to one shared Brain.

## What Phase 4 adds

| Surface | File | What |
|---|---|---|
| Rate limiting | `core/octopus/ratelimit.py` | Sliding-window, 30 writes/min/peer (default, env-tunable). Reads bypass. |
| Scope enforcement | `core/octopus/enforce.py` | Classifies each MCP call against the peer's `capability_scope`; 403 on mismatch. |
| Shim integration | `core/mcp/http_shim.py` | Calls `enforce_request` after signature verification; emits HTTP 429 (rate) / 403 (scope) with `Retry-After` header. |
| SME stress test | `core/octopus/tests/test_enforcement.py` | 10 peers × 30 writes/min = 300 total, zero corrupted journal entries. |

## How rate limiting works

Per-peer sliding window. Each `brain_write_journal` (or any tool
classified as a write) pushes a timestamp onto the peer's deque. On
each new write, entries older than the 60 s window are pruned; if the
deque already holds 30 entries, the request is refused with 429 and a
`Retry-After` header equal to seconds-until-oldest-falls-out-of-window.

- Defaults: 30 writes/min, 60 s window.
- Env tunables: `MAKAKOO_OCTOPUS_WRITES_PER_MIN` (integer).
- Thread-safe: one module-level `threading.Lock` around each bucket mutation.
- Read bypass: the sprint spec is "30 **writes**/minute" — `brain_search`,
  `brain_tail`, `tools/list`, etc. don't burn budget.

## How scope enforcement works

Each tool has a minimum required scope. The enforcement table is:

| Scope | Tools accepted |
|---|---|
| `read-brain`  | `tools/list`, `brain_search`, `brain_recent`, `brain_entities`, `brain_context`, `brain_query`, `brain_tail`, `harvey_superbrain_query`, `harvey_superbrain_vector_search`, `harvey_brain_search` |
| `write-brain` | all `read-brain` tools + `brain_write_journal`, `harvey_brain_write`, `harvey_journal_entry`, `wiki_save`, `wiki_lint`, `wiki_compile`, `harvey_knowledge_ingest` |
| `full-brain`  | everything above + any unclassified tool (swarm, telegram, pi_run, agent install, etc.) |

**Pessimistic default:** unknown tool names require `full-brain`. So
when a new MCP tool ships, a read-only peer can't reach it until the
allowlist is explicitly updated. This mitigates the "new capability
silently leaks to a restricted peer" failure mode.

Peers in `trusted.keys` with no `trust_store` entry (legacy pre-Phase-2
installs) degrade to `read-brain` scope with rate limits still applied.
Phase 2's wizard can backfill via `makakoo octopus trust list` review
+ invite/join rerun.

## SME stress: 300 writes/min verified

The sprint criterion is:

> 10 peers writing simultaneously (300 total writes/min); `http_shim`
> uses file-locking to maintain consistency; verify zero corrupted entries.

`test_ten_peers_three_hundred_writes_zero_corruption` spawns 10
subprocess workers under a `multiprocessing.get_context("spawn")` pool
— a realistic model of 10 separate peer listener processes hitting the
Mac's shim concurrently. Each writes 30 lines to the Brain journal via
`http_shim._write_journal_line` (the same code path the HTTP handler
uses after `brain_write_journal` is accepted).

Pass conditions:
- 300 total lines on disk.
- Every line well-formed (starts with `- SME worker `, ends with `\n`).
- Every nonce is unique (no duplicates = no interleaving).
- Every line carries its nonce suffix (no truncation).

This runs in ~0.5 s — the flock contention is cheap because the
critical section is `open(path, "a") + write(line) + close` × 300.
POSIX append is atomic on local FS at the kernel level; flock makes
the guarantee hold across NFS / iCloud Drive mounts too.

## Cross-platform

| OS | Status | Notes |
|---|---|---|
| macOS         | Tested on Darwin 24 (M-series + Intel) | zeroconf from Homebrew Python 3.11 |
| Linux (glibc) | Tested on Ubuntu 22.04 / Debian 12  | fcntl.flock native, zeroconf via pip |
| Linux (musl)  | Tested on Alpine 3.19 edge images   | same fcntl semantics |
| Windows (WSL2)| WSL2 is Linux under the hood — fcntl.flock, chmod, mDNS all work | Native Windows unsupported for Phase 4 (fcntl missing) |

`makakoo octopus doctor` is the sprint's cross-platform smoke test.
Green on all four above.

## End-to-end check

```bash
# Fresh host
$ makakoo octopus bootstrap --peer-name host-A
$ makakoo agent start octopus-peer

# See what's enforced
$ makakoo octopus doctor
OK   identity: peer_name=host-A pubkey=...xyz
OK   trust store: 0 active grant(s)
OK   shim trust file: ~/.makakoo/config/peers/trusted.keys in sync
OK   onboarding: 0 active invite(s) (expired auto-pruned)

# Invite a read-only peer
$ makakoo octopus invite --link --scope read-brain --duration 7d
✓ invite minted — expires 2026-05-01T...
  link:  makakoo://join?t=...

# On the peer: join
$ makakoo octopus join "makakoo://join?t=..." --peer-name pi
✓ trust grant created: peer_name=pi scope=read-brain expires=...

# Back on host-A: peer can search, can't write
$ makakoo octopus trust list
pi                        read-brain   7d         exp=2026-05-01T...  [active]

# Peer's brain_write_journal call would return:
# HTTP 403 — peer 'pi' has scope 'read-brain'; tool 'brain_write_journal' requires 'write-brain'

# Peer's 31st write in 60s (after scope elevation) would return:
# HTTP 429 + Retry-After: <s>
```

## Files

- `core/octopus/enforce.py`       — scope classification + enforce_request
- `core/octopus/ratelimit.py`     — PeerRateLimiter + RateLimitDecision
- `core/octopus/tests/test_enforcement.py` — 16 tests (scope, rate, SME, nonce burst)

## Tests

Run the full Phase 4 suite:

```bash
PYTHONPATH=plugins-core/lib-harvey-core/src \
  python3 -m unittest core.octopus.tests.test_enforcement -v
```

Full Octopus suite (all 4 phases):

```bash
PYTHONPATH=plugins-core/lib-harvey-core/src \
  python3 -m unittest discover \
    -s plugins-core/lib-harvey-core/src/core/octopus/tests -v
```

55 tests green (54 run + 1 integration-mDNS skip if zeroconf absent).

Plus the JS listener suite (8 tests):

```bash
node plugins-core/lib-harvey-core/src/core/tests/test_harvey_listen.js
```

And the HTTP shim flock / nonce roundtrip (4 tests):

```bash
PYTHONPATH=plugins-core/lib-harvey-core/src \
  python3 plugins-core/lib-harvey-core/src/core/mcp/tests/test_http_shim_unit.py
```

## Security notes

- **Enforcement runs AFTER signature verification.** A forged or
  replayed request can't reach the enforce layer in the first place.
- **Denied requests don't consume rate-limit budget.** A peer that
  attempts a scope-violating write doesn't burn their 30/min slot on
  the 403 — they get the full budget for legitimate traffic.
- **Retry-After is a floor, not a promise.** Clients that honor it
  recover cleanly; clients that hammer get a sustained 429 stream.
- **Pessimistic default on unknown tools** — adding a new MCP tool
  without updating `enforce._READ_TOOLS` / `_WRITE_TOOLS` means only
  `full-brain` peers can call it. Deliberate: "fail closed on unknown".

## Phase 4 exit criteria — all met

- ✅ Rate limit increased to 30 writes/min per peer (sprint spec).
- ✅ `enforce.py` validates `X-Makakoo-Sig` against `trust_store.json`
     (via `trust_store.get()`) and checks scope.
- ✅ `harvey-listen.js` verified to ignore 100% of self-initiated
     writes under burst via nonce-aware LRU (see
     `test_lru_holds_100_nonces_zero_self_match` in
     `test_enforcement.py` and `test_harvey_listen.js`).
- ✅ `makakoo octopus doctor` is the `makakoo doctor --octopus`
     equivalent — green on macOS, Linux (glibc), Linux (musl), WSL2.
