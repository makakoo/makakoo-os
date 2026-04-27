"""Per-peer rate limiting for the Octopus HTTP shim.

Sliding window, 30 writes/min per peer by default — tuned for SME load
(10 teammates actively writing to the shared Brain). 30/min/peer means
a 10-peer team can burst to 300 writes/min, which Phase 1's flock
interlock absorbs cleanly. Above that the shim returns HTTP 429 so
misbehaving peers get visible back-pressure instead of silently
starving each other.

Why in-memory + sliding window (not Redis, not leaky-bucket):
    The shim is single-process; in-memory state is the right shape for
    a single-host peer endpoint. Redis would buy shared state across
    replicas we don't have. A token-bucket is equally valid but has
    more parameters (burst capacity, refill rate); the sprint asks for
    a simple writes-per-minute cap and sliding windows give that with
    one knob.

    A sliding window counter is a short deque of recent timestamps per
    peer. On each request we prune entries older than the window and
    count what's left. Under the spec's load (10 peers × 30 writes/min
    = 300 events/min total), the per-peer deque max size stays ≤ 30 and
    cleanup is O(30) per call — cheap.

Thread safety:
    The HTTP shim is `ThreadingMixIn` (one thread per request). We take
    a single module-level `threading.Lock` around the deque mutation.
    Contention is negligible (the critical section is sub-microsecond).

Scope of "write":
    The interface takes a boolean `is_write` from the caller so the
    enforce layer can decide what counts. Currently the shim's
    intercept layer (`_handle_brain_write_journal`) is the only "write"
    endpoint; everything else (brain_tail, search, etc.) is classified
    as `is_write=False` and bypasses the limit. This is the minimum
    surface that satisfies the sprint's "30 writes/min" spec without
    penalizing read-heavy peers.
"""

from __future__ import annotations

import collections
import os
import threading
import time
from dataclasses import dataclass

DEFAULT_WRITES_PER_MIN = int(os.environ.get("MAKAKOO_OCTOPUS_WRITES_PER_MIN", "30"))
"""Per-peer write budget. Overridable via env for load-testing or for
lean single-peer deployments that want a tighter cap."""

DEFAULT_WINDOW_S = 60
"""Sliding window in seconds. 60 matches the /min spec verbiage."""


@dataclass
class RateLimitDecision:
    allowed: bool
    remaining: int
    reset_after_s: float


class PeerRateLimiter:
    """Sliding-window per-peer rate limiter.

    One instance is owned by the HTTP shim and shared across threads.
    :func:`check` is the only method callers touch; it returns a
    decision and records the write atomically.
    """

    def __init__(
        self,
        *,
        writes_per_min: int = DEFAULT_WRITES_PER_MIN,
        window_s: int = DEFAULT_WINDOW_S,
    ) -> None:
        if writes_per_min < 1:
            raise ValueError(f"writes_per_min must be ≥ 1 (got {writes_per_min})")
        if window_s < 1:
            raise ValueError(f"window_s must be ≥ 1 (got {window_s})")
        self.limit = writes_per_min
        self.window_s = window_s
        self._buckets: dict[str, collections.deque[float]] = {}
        self._lock = threading.Lock()

    def check(
        self,
        peer_name: str,
        *,
        is_write: bool,
        now: float | None = None,
    ) -> RateLimitDecision:
        """Return whether ``peer_name``'s current request should be allowed.

        Non-writes bypass the limit entirely (reads are cheap and not
        covered by the sprint spec). Write requests consume one slot;
        if the sliding window already holds ``limit`` writes, we
        refuse with ``allowed=False`` and compute ``reset_after_s`` as
        time-until-the-oldest-write-falls-out-of-the-window.

        On allow, the current timestamp is appended to the bucket
        atomically under the lock — the caller cannot race two writes
        into the same slot.
        """
        now = time.time() if now is None else now
        if not is_write:
            return RateLimitDecision(allowed=True, remaining=self.limit, reset_after_s=0.0)

        cutoff = now - self.window_s
        with self._lock:
            bucket = self._buckets.get(peer_name)
            if bucket is None:
                bucket = collections.deque()
                self._buckets[peer_name] = bucket

            # Prune expired entries. O(k) where k is the number of old
            # entries — cheap because the deque only holds recent ones.
            while bucket and bucket[0] < cutoff:
                bucket.popleft()

            if len(bucket) >= self.limit:
                reset_after = (bucket[0] + self.window_s) - now
                # Small floor so callers can sleep() on it meaningfully.
                reset_after = max(reset_after, 0.0)
                return RateLimitDecision(
                    allowed=False,
                    remaining=0,
                    reset_after_s=reset_after,
                )

            bucket.append(now)
            remaining = self.limit - len(bucket)

        return RateLimitDecision(allowed=True, remaining=remaining, reset_after_s=0.0)

    def snapshot(self) -> dict[str, int]:
        """Return a map of `peer_name → current-window use count`. Copy-
        out is safe without the lock because the returned dict is a
        fresh materialization."""
        with self._lock:
            return {k: len(v) for k, v in self._buckets.items()}

    def reset(self) -> None:
        """Drop all buckets. Test-only — call this between tests to
        isolate per-peer state."""
        with self._lock:
            self._buckets.clear()
