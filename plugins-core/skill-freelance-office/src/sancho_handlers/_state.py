"""Shared JSON-state helpers for SANCHO handlers bundled with
skill-freelance-office.

State files live under
``$MAKAKOO_HOME/state/skill-freelance-office/`` — the same tree
``md_table.state_dir()`` points at. Writes go through a tmp-file +
``os.replace`` so a mid-flight crash can't leave torn JSON on disk.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Dict

from ..core import md_table


def state_path(name: str) -> Path:
    return md_table.state_dir() / name


def load(name: str) -> Dict[str, Any]:
    p = state_path(name)
    if not p.is_file():
        return {}
    try:
        with p.open("r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, json.JSONDecodeError):
        return {}
    if not isinstance(data, dict):
        return {}
    return data


def save(name: str, data: Dict[str, Any]) -> Path:
    p = state_path(name)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(p.suffix + ".tmp")
    with tmp.open("w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, p)
    return p
