#!/usr/bin/env python3
"""
run-sancho-task.py — dispatch shim for Rust SANCHO SubprocessHandler.

The Rust daemon owns the schedule (TimeGate + ActiveHoursGate) but
delegates the actual task work to Python via this shim until the full
task set is ported to native Rust. Usage:

    run-sancho-task.py --task gym_classify
    run-sancho-task.py --task gym_morning_report

Exits 0 on success, 1 on handler failure, 2 on unknown task.
Prints the handler's result dict as JSON to stdout so the Rust side
can capture it in the task journal. Never raises — all failures are
caught and reported via a non-zero exit + stderr message.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import traceback
from pathlib import Path

MAKAKOO_HOME = os.environ.get("MAKAKOO_HOME") or os.environ.get(
    "HARVEY_HOME", os.path.expanduser("~/MAKAKOO")
)
sys.path.insert(0, str(Path(MAKAKOO_HOME) / "harvey-os"))


def _load_handler(name: str):
    from core.sancho import handlers as H

    func_name = f"handle_{name}"
    handler = getattr(H, func_name, None)
    if handler is None or not callable(handler):
        return None
    return handler


def main() -> int:
    parser = argparse.ArgumentParser(prog="run-sancho-task")
    parser.add_argument("--task", required=True, help="handle_<task> to invoke")
    parser.add_argument("--quiet", action="store_true", help="suppress stdout JSON")
    args = parser.parse_args()

    handler = _load_handler(args.task)
    if handler is None:
        sys.stderr.write(f"run-sancho-task: unknown task {args.task!r}\n")
        return 2

    try:
        result = handler()
    except Exception as exc:
        sys.stderr.write(
            f"run-sancho-task: {args.task} failed: {type(exc).__name__}: {exc}\n"
        )
        traceback.print_exc(file=sys.stderr)
        return 1

    if not args.quiet:
        try:
            json.dump(result, sys.stdout, default=str, ensure_ascii=False)
            sys.stdout.write("\n")
        except Exception:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
