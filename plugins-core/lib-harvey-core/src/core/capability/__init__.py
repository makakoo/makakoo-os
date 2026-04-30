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
    decrement as rate_limit_decrement,
    default_rate_limit_path,
)
from core.capability.audit_client import (
    default_audit_path,
    log_audit,
    log_fs_write,
)
from core.capability.perms_core import (
    CONVERSATIONAL_CHANNELS,
    GrantArgs,
    ListArgs,
    PermsError,
    RevokeArgs,
    do_grant,
    do_list_grants,
    do_revoke,
    grant_success_msg,
    list_summary_msg,
    revoke_success_msg,
)
from core.capability.action_perms import (
    ActionGrantArgs,
    action_scope,
    grant_action,
    has_action_grant,
    list_action_grants,
    run_granted_shell_command,
    shell_command_block_reason,
)

__all__ = [
    "CONVERSATIONAL_CHANNELS",
    "Grant",
    "ActionGrantArgs",
    "GrantArgs",
    "ListArgs",
    "PermsError",
    "RateLimitExceeded",
    "RevokeArgs",
    "UserGrantsFile",
    "check_and_increment",
    "default_audit_path",
    "default_grants_path",
    "default_rate_limit_path",
    "do_grant",
    "do_list_grants",
    "do_revoke",
    "escape_audit_field",
    "action_scope",
    "grant_success_msg",
    "grant_action",
    "has_action_grant",
    "list_action_grants",
    "list_summary_msg",
    "log_audit",
    "log_fs_write",
    "new_grant_id",
    "rate_limit_decrement",
    "revoke_success_msg",
    "run_granted_shell_command",
    "shell_command_block_reason",
]
