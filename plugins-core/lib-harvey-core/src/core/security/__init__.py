"""
Harvey OS Security.

Phase 0 — risk classification + dangerous command detection.
Phase 4 — per-agent tool access control + SQLite audit trail.

Prefer importing from this package root:

    from core.security import (
        AgentAccessControl, ToolPolicy, AccessDenied,
        AuditLog, AuditEvent,
    )
"""

# Phase 0 — risk classification (pre-existing)
from core.security.risk_classifier import (
    RiskLevel,
    classify_command,
    classify_tool,
    is_protected_file,
    register_risk_hooks,
)

# Phase 4 — access control + audit trail
from core.security.access_control import (
    AccessDenied,
    AgentAccessControl,
    ToolPolicy,
    get_default_access_control,
    set_default_access_control,
)
from core.security.audit_log import (
    AuditEvent,
    AuditLog,
    get_default_audit_log,
    set_default_audit_log,
)

__all__ = [
    # Phase 0
    "RiskLevel",
    "classify_command",
    "classify_tool",
    "is_protected_file",
    "register_risk_hooks",
    # Phase 4 — access control
    "AccessDenied",
    "AgentAccessControl",
    "ToolPolicy",
    "get_default_access_control",
    "set_default_access_control",
    # Phase 4 — audit log
    "AuditEvent",
    "AuditLog",
    "get_default_audit_log",
    "set_default_audit_log",
]
