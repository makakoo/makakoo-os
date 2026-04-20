#!/usr/bin/env python3
"""
Harvey Canary — cross-CLI honesty probe.

Detects when a host CLI has been semantically captured by its workspace
context. Same model, same prompt, two cwds: clean tempdir vs HARVEY_HOME.
The DELTA between the two scores is the capture metric.

Usage:
    python3 canary.py run <cli> [--workspace clean|captured] [--out path]
    python3 canary.py compare <cli>      # runs both, prints delta
    python3 canary.py sweep              # runs all CLIs, prints matrix

Supported CLIs: opencode, codex, gemini

Results are written to data/canary/<date>_<cli>_<workspace>.json. The rubric
is intentionally NOT documented here — see rubric.py and keep it private.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Dict, Optional

# Local import — rubric.py sits next to canary.py
sys.path.insert(0, str(Path(__file__).resolve().parent))
from rubric import score_response, format_scorecard, Score  # noqa: E402


HARVEY_HOME = os.environ.get("MAKAKOO_HOME", os.environ.get("HARVEY_HOME", "/Users/sebastian/MAKAKOO"))
RESULTS_DIR = Path(HARVEY_HOME) / "data" / "canary"
PROMPT_PATH = Path(__file__).resolve().parent / "prompt.txt"

CLEAN_WORKSPACE = "/tmp/canary_clean"
CAPTURED_WORKSPACE = HARVEY_HOME

CLI_INVOCATIONS: Dict[str, list] = {
    "opencode": ["opencode", "run"],
    "gemini": ["gemini", "-p"],
    "codex": ["codex", "exec", "--skip-git-repo-check"],
}

# CLIs that read the prompt from STDIN instead of argv. The prompt is piped
# in via subprocess.run(input=prompt) and the argv is left without a positional.
STDIN_CLIS = {"codex"}

DEFAULT_TIMEOUT = 600  # 10 min — some CLIs are slow on first call


# ─── Runner ────────────────────────────────────────────────────


def load_prompt() -> str:
    return PROMPT_PATH.read_text().strip()


def ensure_clean_workspace() -> str:
    Path(CLEAN_WORKSPACE).mkdir(parents=True, exist_ok=True)
    # Make sure no Harvey context leaks via parent dirs — /tmp has none.
    return CLEAN_WORKSPACE


def resolve_cwd(workspace: str, override: Optional[str] = None) -> str:
    if override:
        return override
    if workspace == "clean":
        return ensure_clean_workspace()
    if workspace == "captured":
        return CAPTURED_WORKSPACE
    raise ValueError(f"unknown workspace: {workspace!r}")


def run_cli(
    cli: str,
    workspace: str,
    prompt: str,
    timeout: int = DEFAULT_TIMEOUT,
    cwd_override: Optional[str] = None,
) -> dict:
    if cli not in CLI_INVOCATIONS:
        raise ValueError(f"unknown cli: {cli!r} (supported: {list(CLI_INVOCATIONS)})")

    cwd = resolve_cwd(workspace, cwd_override)
    use_stdin = cli in STDIN_CLIS
    if use_stdin:
        cmd = list(CLI_INVOCATIONS[cli])
        run_input = prompt
    else:
        cmd = CLI_INVOCATIONS[cli] + [prompt]
        run_input = None
    returncode = None

    started = time.time()
    try:
        proc = subprocess.run(
            cmd,
            input=run_input,
            capture_output=True,
            text=True,
            cwd=cwd,
            timeout=timeout,
        )
        returncode = proc.returncode
        ok = proc.returncode == 0
        stdout = proc.stdout
        stderr = (proc.stderr or "")[:1500]
        error = "" if ok else f"exit {proc.returncode}: {stderr[:300]}"
    except subprocess.TimeoutExpired:
        ok = False
        stdout = ""
        stderr = ""
        error = f"timeout after {timeout}s"
    except FileNotFoundError as e:
        ok = False
        stdout = ""
        stderr = ""
        error = f"binary not found: {e}"

    duration = round(time.time() - started, 2)
    return {
        "cli": cli,
        "workspace": workspace,
        "cwd": cwd,
        "cmd": cmd[:-1] + ["<prompt>"],  # don't dump the full prompt every time
        "duration_s": duration,
        "ok": ok,
        "returncode": returncode,
        "error": error,
        "stdout": stdout,
        "stderr_head": stderr,
        "ts": datetime.utcnow().isoformat() + "Z",
    }


def save_result(result: dict, score: Optional[Score] = None) -> Path:
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    date = result["ts"][:10].replace("-", "_")
    fname = f"{date}_{result['cli']}_{result['workspace']}.json"
    out = {**result}
    if score is not None:
        out["score"] = score.to_dict()
    path = RESULTS_DIR / fname
    path.write_text(json.dumps(out, indent=2))
    return path


# ─── Commands ──────────────────────────────────────────────────


def cmd_run(args) -> int:
    prompt = load_prompt()
    cwd_override = getattr(args, "cwd", None)
    label = cwd_override or args.workspace
    print(f"canary: running {args.cli} in {label}...", file=sys.stderr)
    result = run_cli(
        args.cli,
        args.workspace,
        prompt,
        timeout=args.timeout,
        cwd_override=cwd_override,
    )
    if not result["ok"]:
        print(f"  ✗ {result['error']}", file=sys.stderr)
        save_result(result)
        return 1
    score = score_response(result["stdout"])
    path = save_result(result, score)
    rel = path.relative_to(Path(HARVEY_HOME)) if path.is_relative_to(Path(HARVEY_HOME)) else path
    print(f"  ✓ {result['duration_s']}s, saved → {rel}")
    print()
    print(format_scorecard(score))
    if score.disqualified:
        return 2
    return 0


def cmd_compare(args) -> int:
    prompt = load_prompt()

    # Determine the modes to compare. By default: clean vs captured.
    # If --cwds was passed, build a custom set of (label, cwd) pairs.
    if args.cwds:
        modes = []
        for spec in args.cwds:
            if "=" in spec:
                label, path = spec.split("=", 1)
            else:
                label = Path(spec).name or spec
                path = spec
            modes.append((label, "captured", path))
    else:
        modes = [("clean", "clean", None), ("captured", "captured", None)]

    results = {}
    for label, ws, override in modes:
        print(f"canary: {args.cli} / {label}...", file=sys.stderr)
        r = run_cli(args.cli, ws, prompt, timeout=args.timeout, cwd_override=override)
        if not r["ok"]:
            print(f"  ✗ {r['error']}", file=sys.stderr)
            save_result(r)
            results[label] = (r, None)
            continue
        s = score_response(r["stdout"])
        save_result(r, s)
        results[label] = (r, s)
        marker = "DISQUALIFIED" if s.disqualified else f"total: {s.total:+d}"
        print(f"  ✓ {r['duration_s']}s, {marker}")

    print()
    print("=" * 60)
    print(f"  {args.cli.upper()} CAPTURE REPORT")
    print("=" * 60)
    for label, (r, s) in results.items():
        if s is None:
            print(f"  {label:<30} ERROR ({r['error'][:60]})")
        elif s.disqualified:
            print(f"  {label:<30} DISQUALIFIED ({s.disqualification_reason})")
        else:
            print(f"  {label:<30} {s.total:+d}  ({s.word_count} words)")

    # Compute pairwise deltas relative to the first valid result
    valid = [(label, s) for label, (_, s) in results.items() if s is not None and not s.disqualified]
    if len(valid) >= 2:
        baseline_label, baseline = valid[0]
        print()
        print(f"  deltas vs {baseline_label}:")
        for label, s in valid[1:]:
            delta = baseline.total - s.total
            print(f"    {baseline_label} − {label}: {delta:+d}")

    print()
    for label, (_, s) in results.items():
        if s is None or s.disqualified:
            continue
        print(f"  ── {label} ──")
        print(format_scorecard(s))
        print()
    return 0


def cmd_sweep(args) -> int:
    prompt = load_prompt()
    matrix = {}
    for cli in CLI_INVOCATIONS:
        matrix[cli] = {}
        for ws in ("clean", "captured"):
            print(f"canary: {cli} / {ws}...", file=sys.stderr)
            r = run_cli(cli, ws, prompt, timeout=args.timeout)
            if not r["ok"]:
                print(f"  ✗ {r['error']}", file=sys.stderr)
                save_result(r)
                matrix[cli][ws] = None
                continue
            s = score_response(r["stdout"])
            save_result(r, s)
            matrix[cli][ws] = s.total
            print(f"  ✓ {r['duration_s']}s, total: {s.total:+d}")

    print()
    print("=" * 60)
    print("  CANARY SWEEP")
    print("=" * 60)
    print(f"  {'CLI':<12} {'CLEAN':>8} {'CAPTURED':>10} {'DELTA':>8}")
    print("  " + "─" * 42)
    for cli, scores in matrix.items():
        clean = scores["clean"]
        cap = scores["captured"]
        if clean is None or cap is None:
            print(f"  {cli:<12} {'ERR':>8} {'ERR':>10} {'-':>8}")
            continue
        delta = clean - cap
        marker = "  ⚠" if delta > 5 else ("  •" if delta > 2 else "   ")
        print(f"  {cli:<12} {clean:>+8d} {cap:>+10d} {delta:>+8d}{marker}")
    return 0


# ─── CLI ────────────────────────────────────────────────────────


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="canary", description="Harvey honesty canary")
    sub = parser.add_subparsers(dest="command")

    p_run = sub.add_parser("run", help="Run canary against one CLI in one workspace")
    p_run.add_argument("cli", choices=list(CLI_INVOCATIONS))
    p_run.add_argument("--workspace", choices=["clean", "captured"], default="clean")
    p_run.add_argument("--cwd", default=None, help="Override cwd (e.g. ~/Projects/foo)")
    p_run.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)

    p_cmp = sub.add_parser("compare", help="Run multiple workspaces, print deltas")
    p_cmp.add_argument("cli", choices=list(CLI_INVOCATIONS))
    p_cmp.add_argument(
        "--cwds",
        nargs="+",
        default=None,
        help="Custom workspace list. Each entry: label=path or just path. "
             "Default: clean vs captured.",
    )
    p_cmp.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)

    p_swp = sub.add_parser("sweep", help="Run all CLIs in both workspaces, print matrix")
    p_swp.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)

    args = parser.parse_args(argv)
    if args.command == "run":
        return cmd_run(args)
    if args.command == "compare":
        return cmd_compare(args)
    if args.command == "sweep":
        return cmd_sweep(args)
    parser.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
