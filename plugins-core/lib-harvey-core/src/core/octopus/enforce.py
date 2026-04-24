"""Capability + rate-limit enforcement for the Octopus HTTP shim.

The shim (Phase 1) already verifies Ed25519 signatures against
`trusted.keys` — that's authentication. This module adds the two
*authorization* layers the sprint requires:

1. **Scope enforcement** — a peer with `read-brain` scope can only
   invoke read-shaped tool calls; `write-brain` adds brain mutations;
   `full-brain` is unrestricted. Classification is explicit and
   pessimistic: unknown tool names default to "needs full-brain scope"
   so a newly-added MCP tool can't accidentally leak to a read-only
   peer before the allowlist is updated.

2. **Per-peer rate limiting** — write calls are bounded at
   :data:`ratelimit.DEFAULT_WRITES_PER_MIN` (30/min) per peer. Reads
   bypass. Exceeding the limit returns HTTP 429 so the peer gets a
   visible back-pressure signal.

Wire-up:
    The shim's `RpcHandler.do_POST` calls :func:`enforce_request`
    after signature verification passes and before dispatching the
    intercept or the stdio pool. Enforcement lives here rather than
    inline in `http_shim.py` so (a) it's unit-testable without
    spinning up the HTTP server and (b) a future Rust re-implementation
    of the shim can call this same Python module (or port it).

Design: the module is intentionally side-effect-free on the "deny"
path — a denied request touches NEITHER the rate-limit bucket nor the
trust store. That way a scope-violating write doesn't also burn a
rate-limit slot on the peer's bucket.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from . import CAPABILITIES
from .ratelimit import PeerRateLimiter, RateLimitDecision
from . import trust_store


# ────────────────────────── tool classification ─────────────────────

# Tool names → minimum scope required. Ordered so a scope-check is a
# simple CAPABILITIES-index comparison: peer's scope index must be ≥
# required tool's scope index (where read-brain=0, write-brain=1,
# full-brain=2).

_READ_TOOLS = frozenset({
    "tools/list",
    "brain_search",
    "brain_recent",
    "brain_entities",
    "brain_context",
    "brain_query",
    "brain_tail",
    "harvey_superbrain_query",
    "harvey_superbrain_vector_search",
    "harvey_brain_search",
})

_WRITE_TOOLS = frozenset({
    "brain_write_journal",
    "harvey_brain_write",
    "harvey_journal_entry",
    "wiki_save",
    "wiki_lint",
    "wiki_compile",
    "harvey_knowledge_ingest",
})

# Everything else → requires full-brain by default (pessimistic). This
# includes execution tools (swarm, telegram send, pi_run, outbound_draft
# that posts to LinkedIn), system-state mutations (agent install/remove),
# etc. Opening one of these up to narrow scope is a deliberate PR — the
# default is closed.


def _scope_rank(scope: str) -> int:
    """Return the 0-based index of ``scope`` in :data:`CAPABILITIES`."""
    return CAPABILITIES.index(scope)


def _tool_min_scope(method: str, name: str | None) -> str:
    """Classify a JSON-RPC call into the minimum capability scope required.

    ``method`` is the JSON-RPC method (e.g. ``tools/list``,
    ``tools/call``). ``name`` is the ``params.name`` of a
    ``tools/call`` invocation; None for non-call methods.
    """
    if method != "tools/call":
        # tools/list, initialize, ping, etc. are all read-shaped probes.
        # If new non-call RPC methods appear that mutate state, we gate
        # them here on an explicit allowlist — not the default.
        return "read-brain"
    if name is None:
        return "full-brain"
    if name in _READ_TOOLS:
        return "read-brain"
    if name in _WRITE_TOOLS:
        return "write-brain"
    return "full-brain"


# ────────────────────────── enforce ─────────────────────────────────

@dataclass
class EnforceDecision:
    allowed: bool
    http_status: int           # 200 on allow; 403 scope; 429 rate limit; 401 unknown peer
    error_message: str | None  # plain-text reason for the client
    retry_after_s: float       # only meaningful for 429


def enforce_request(
    *,
    peer_name: str,
    rpc_method: str,
    rpc_params: dict[str, Any] | None,
    limiter: PeerRateLimiter,
    now: float | None = None,
) -> EnforceDecision:
    """Apply scope + rate-limit checks and return a go/no-go decision.

    Call this AFTER the shim's signature verification passes. The
    signature verifies the peer is who they say they are; enforcement
    decides whether they're allowed to do what they're asking.

    Args:
        peer_name: verified `X-Makakoo-Peer` from the request header.
        rpc_method: JSON-RPC `method` field (e.g. ``tools/call``).
        rpc_params: JSON-RPC `params` object; may be None.
        limiter: shared :class:`PeerRateLimiter` instance.
        now: override current time for tests.

    Returns:
        :class:`EnforceDecision` with http_status to send back.
    """
    grant = trust_store.get(peer_name)
    if grant is None:
        # Race: peer was in trusted.keys (signature verified) but the
        # trust_store doesn't know them. This can happen for legacy
        # trusted.keys entries that predate the Octopus trust store.
        # Fail open to `read-brain` scope + rate-limit them so legacy
        # peers keep working but can't escalate. Phase 4 migration
        # step: `makakoo octopus trust migrate` will backfill these.
        peer_scope = "read-brain"
    else:
        peer_scope = grant.capability_scope

    name = None
    if rpc_params:
        name = rpc_params.get("name")
    required = _tool_min_scope(rpc_method, name)

    if _scope_rank(peer_scope) < _scope_rank(required):
        return EnforceDecision(
            allowed=False,
            http_status=403,
            error_message=(
                f"peer {peer_name!r} has scope {peer_scope!r}; "
                f"tool {name or rpc_method!r} requires {required!r}"
            ),
            retry_after_s=0.0,
        )

    # Rate-limit is only applied to actual writes. Reads through a
    # write-capable or full-capable grant are also free (the spec is
    # "30 writes/min", not "30 requests/min").
    is_write = required in ("write-brain", "full-brain")
    decision: RateLimitDecision = limiter.check(peer_name, is_write=is_write, now=now)
    if not decision.allowed:
        return EnforceDecision(
            allowed=False,
            http_status=429,
            error_message=(
                f"peer {peer_name!r} exceeded {limiter.limit} writes/minute; "
                f"retry in {decision.reset_after_s:.1f}s"
            ),
            retry_after_s=decision.reset_after_s,
        )

    return EnforceDecision(
        allowed=True, http_status=200, error_message=None, retry_after_s=0.0,
    )
