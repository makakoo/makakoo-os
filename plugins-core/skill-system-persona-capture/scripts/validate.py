#!/usr/bin/env python3
"""Validate persona-capture plugin structure and Python syntax."""
from __future__ import annotations

import json
import py_compile
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = [
    ROOT / "plugin.toml",
    ROOT / "SKILL.md",
    ROOT / "src" / "persona_capture.py",
    ROOT / "references" / "schema.md",
]


def main() -> int:
    missing = [str(p) for p in REQUIRED if not p.exists()]
    if missing:
        print("missing: " + ", ".join(missing), file=sys.stderr)
        return 1
    py_compile.compile(str(ROOT / "src" / "persona_capture.py"), doraise=True)
    text = (ROOT / "SKILL.md").read_text(encoding="utf-8")
    for needle in ["persona_registry.json", "persona_context.md", "Primary persona", "entrypoint:"]:
        if needle not in text:
            print(f"SKILL.md missing {needle!r}", file=sys.stderr)
            return 1
    print("ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
