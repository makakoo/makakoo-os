"""Identity alias helpers for Cortex memory.

The storage-backed implementation lives on CortexMemory so aliases share the
same SQLite connection/db_path. This module keeps the sprint's public file
surface explicit.
"""

from __future__ import annotations


def default_person_id(channel: str, channel_user_id: str) -> str:
    return f"channel:{channel}:{channel_user_id}"
