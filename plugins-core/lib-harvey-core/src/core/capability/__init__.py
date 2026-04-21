"""
core.capability — user-managed runtime grant layer.

Phase B of MAKAKOO-OS-V0.3-USER-GRANTS. Pairs with the Rust mirror
at `makakoo-core/src/capability/` — both read AND write the same
`$MAKAKOO_HOME/config/user_grants.json` via the sidecar-lock
protocol. See `spec/USER_GRANTS.md` for the authoritative schema
and lock contract.

Prefer the package-level API:

    from core.capability import (
        Grant, UserGrantsFile, RateLimitExceeded,
        check_and_increment, default_grants_path,
    )
"""

from core.capability.user_grants import (
    Grant,
    UserGrantsFile,
    default_grants_path,
    escape_audit_field,
    new_grant_id,
)
from core.capability.rate_limit import (
    RateLimitExceeded,
    check_and_increment,
    default_rate_limit_path,
)

__all__ = [
    "Grant",
    "UserGrantsFile",
    "RateLimitExceeded",
    "check_and_increment",
    "default_grants_path",
    "default_rate_limit_path",
    "escape_audit_field",
    "new_grant_id",
]
