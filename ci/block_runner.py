#!/usr/bin/env python3
"""Markdown → executable code block extractor + runner.

Walks a list of Markdown files, extracts every fenced code block, runs the
runnable ones against the current shell, and diffs their stdout against any
immediately-following `output` / `text` expected-output block.

Runnable block languages:  `sh`, `bash`
Expected-output languages: `output`, `text`

A block can carry a trailing HTML comment to opt out:
    ```sh
    rm -rf /
    ```
    <!-- verify: skip reason="destructive example" -->

The `reason="..."` attribute is REQUIRED for skips. A skip without a reason
is an error.

Exit codes:
    0 — all blocks pass (or are skipped with a reason)
    1 — at least one block failed; failures are printed to stderr
    2 — usage or config error

CLI:
    python3 ci/block_runner.py <file.md> [<file2.md> ...]
    python3 ci/block_runner.py --manifest ci/docs_manifest.toml
    python3 ci/block_runner.py --self-test      # tiny built-in smoke test
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

REPO_ROOT = Path(__file__).resolve().parent.parent

RUNNABLE_LANGS = {"sh", "bash"}
OUTPUT_LANGS = {"output", "text"}

_FENCE_RE = re.compile(
    r"^```(?P<lang>[A-Za-z0-9_+-]*)\s*\n(?P<body>.*?)^```\s*$",
    re.MULTILINE | re.DOTALL,
)
_SKIP_RE = re.compile(
    r"<!--\s*verify:\s*skip(?:\s+reason\s*=\s*\"(?P<reason>[^\"]*)\")?\s*-->"
)


@dataclass
class Block:
    path: Path
    idx: int
    lang: str
    body: str
    skip_reason: str | None = None
    trailer: str = ""  # bytes right after the closing fence, up to next fence


@dataclass
class Outcome:
    block: Block
    status: str           # "pass" | "fail" | "skip"
    stdout: str = ""
    stderr: str = ""
    expected: str | None = None
    exit_code: int = 0
    detail: str = ""


def _find_blocks(path: Path) -> list[Block]:
    """Return every fenced code block in `path`, plus the text trailing each
    block (for skip-marker detection)."""
    text = path.read_text(encoding="utf-8")
    blocks: list[Block] = []
    matches = list(_FENCE_RE.finditer(text))
    for i, m in enumerate(matches):
        end = m.end()
        next_start = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        trailer = text[end:next_start]
        blocks.append(
            Block(
                path=path,
                idx=i,
                lang=(m.group("lang") or "").lower(),
                body=m.group("body"),
                trailer=trailer,
            )
        )
    # Attach skip markers, scanning each block's trailer.
    for b in blocks:
        m = _SKIP_RE.search(b.trailer)
        if m:
            b.skip_reason = m.group("reason") or ""
    return blocks


def _pair_with_expected(blocks: list[Block]) -> list[tuple[Block, Block | None]]:
    """Pair each runnable block with the nearest following `output`/`text`
    block, if any (and as long as it is its immediate successor modulo
    whitespace)."""
    pairs: list[tuple[Block, Block | None]] = []
    for i, b in enumerate(blocks):
        if b.lang not in RUNNABLE_LANGS:
            continue
        expected: Block | None = None
        if i + 1 < len(blocks) and blocks[i + 1].lang in OUTPUT_LANGS:
            expected = blocks[i + 1]
        pairs.append((b, expected))
    return pairs


def _run_block(block: Block, timeout_s: float = 30.0) -> tuple[int, str, str]:
    """Run a bash/sh block. Return (exit_code, stdout, stderr). Uses `-e`
    so the first failing command stops the block; `-u` is NOT set because
    many docs examples reference shell vars that happen to be unset.

    Each block runs in a dedicated tempdir under /tmp/ to insulate blocks
    from each other. Blocks are NOT given cross-block state.
    """
    with tempfile.TemporaryDirectory(prefix="makakoo-docs-verify-") as tmp:
        proc = subprocess.run(
            ["/bin/sh", "-e", "-c", block.body],
            capture_output=True,
            text=True,
            cwd=tmp,
            timeout=timeout_s,
        )
        return proc.returncode, proc.stdout, proc.stderr


def _expected_match(expected_body: str, actual_stdout: str) -> bool:
    """Loose match: every non-empty line in `expected_body` must appear in
    `actual_stdout`, in order. Extra lines in actual are fine (version skew
    tolerance)."""
    needles = [ln.strip() for ln in expected_body.splitlines() if ln.strip()]
    if not needles:
        return True
    cursor = 0
    for ln in actual_stdout.splitlines():
        stripped = ln.strip()
        if cursor < len(needles) and needles[cursor] in stripped:
            cursor += 1
            if cursor == len(needles):
                return True
    return cursor == len(needles)


def verify_file(path: Path, timeout_s: float = 30.0) -> list[Outcome]:
    outcomes: list[Outcome] = []
    blocks = _find_blocks(path)
    for block, expected in _pair_with_expected(blocks):
        if block.skip_reason is not None:
            if not block.skip_reason:
                outcomes.append(
                    Outcome(block=block, status="fail",
                            detail="skip marker is missing a non-empty reason"
                                   "=\"...\" attribute")
                )
                continue
            outcomes.append(
                Outcome(block=block, status="skip",
                        detail=f"skipped: {block.skip_reason}")
            )
            continue

        try:
            rc, stdout, stderr = _run_block(block, timeout_s=timeout_s)
        except subprocess.TimeoutExpired:
            outcomes.append(
                Outcome(block=block, status="fail",
                        detail=f"timeout after {timeout_s}s")
            )
            continue
        except Exception as e:  # noqa: BLE001
            outcomes.append(
                Outcome(block=block, status="fail",
                        detail=f"runner error: {e}")
            )
            continue

        if rc != 0:
            outcomes.append(
                Outcome(block=block, status="fail", stdout=stdout, stderr=stderr,
                        exit_code=rc,
                        detail=f"block exited {rc} (stderr tail: "
                               f"{stderr.strip()[-200:]!r})")
            )
            continue

        if expected is not None:
            if not _expected_match(expected.body, stdout):
                outcomes.append(
                    Outcome(block=block, status="fail", stdout=stdout,
                            stderr=stderr, expected=expected.body,
                            detail="expected output did not match")
                )
                continue

        outcomes.append(Outcome(block=block, status="pass", stdout=stdout))
    return outcomes


def _relpath(path: Path) -> Path | str:
    """Return `path` relative to REPO_ROOT, falling back to the absolute
    path if `path` lives outside the repo (as happens in the self-test)."""
    try:
        return path.relative_to(REPO_ROOT)
    except ValueError:
        return path


def _print_report(all_outcomes: dict[Path, list[Outcome]]) -> int:
    fails = 0
    skips = 0
    passes = 0
    for path, outcomes in all_outcomes.items():
        for o in outcomes:
            if o.status == "pass":
                passes += 1
            elif o.status == "skip":
                skips += 1
                print(
                    f"SKIP  {_relpath(path)}  block #{o.block.idx}  "
                    f"({o.detail})"
                )
            else:
                fails += 1
                print(
                    f"FAIL  {_relpath(path)}  block #{o.block.idx}  "
                    f"{o.detail}",
                    file=sys.stderr,
                )
    total = passes + skips + fails
    print()
    print(f"Total: {total}    Pass: {passes}    Skip: {skips}    Fail: {fails}")
    return 1 if fails else 0


def _self_test() -> int:
    good = "```sh\necho hello\n```\n\n```output\nhello\n```\n"
    bad_bash = "```sh\nexit 3\n```\n"
    skip_no_reason = "```sh\nrm -rf /\n```\n<!-- verify: skip -->\n"
    skip_ok = "```sh\nrm -rf /\n```\n<!-- verify: skip reason=\"demo only\" -->\n"
    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        (d / "good.md").write_text(good)
        (d / "bad.md").write_text(bad_bash)
        (d / "skip_bad.md").write_text(skip_no_reason)
        (d / "skip_ok.md").write_text(skip_ok)

        r1 = verify_file(d / "good.md")
        assert r1[0].status == "pass", r1
        r2 = verify_file(d / "bad.md")
        assert r2[0].status == "fail", r2
        r3 = verify_file(d / "skip_bad.md")
        assert r3[0].status == "fail", r3
        assert "missing a non-empty reason" in r3[0].detail
        r4 = verify_file(d / "skip_ok.md")
        assert r4[0].status == "skip", r4
    print("self-test OK")
    return 0


def _load_manifest(path: Path) -> list[Path]:
    """Parse the tiny TOML-ish manifest. Uses stdlib — no external dep.

    Supported shape:
        files = [
            "docs/walkthroughs/01-fresh-install-mac.md",
            "docs/walkthroughs/02-first-brain-entry.md",
        ]
    """
    try:
        import tomllib  # type: ignore[import-not-found]
    except ModuleNotFoundError:
        try:
            import tomli as tomllib  # type: ignore[import-not-found]
        except ModuleNotFoundError:
            print(
                "error: Python 3.11+ or `pip install tomli` required to parse "
                f"manifest at {path}",
                file=sys.stderr,
            )
            sys.exit(2)
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    files = data.get("files", [])
    return [REPO_ROOT / p for p in files]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("paths", nargs="*", help="Markdown files to verify.")
    ap.add_argument("--manifest", type=Path,
                    help="Path to a TOML manifest listing files to verify.")
    ap.add_argument("--self-test", action="store_true",
                    help="Run the built-in sanity check.")
    ap.add_argument("--timeout", type=float, default=30.0,
                    help="Per-block timeout in seconds (default: 30).")
    ap.add_argument("--json", action="store_true",
                    help="Emit a machine-readable JSON report to stdout.")
    args = ap.parse_args()

    if args.self_test:
        return _self_test()

    paths: list[Path] = [Path(p).resolve() for p in args.paths]
    if args.manifest:
        paths.extend(_load_manifest(args.manifest.resolve()))

    if not paths:
        ap.error("provide at least one path, or --manifest, or --self-test")

    all_outcomes: dict[Path, list[Outcome]] = {}
    for p in paths:
        if not p.exists():
            print(f"warn: {p} does not exist — skipping", file=sys.stderr)
            continue
        all_outcomes[p] = verify_file(p, timeout_s=args.timeout)

    if args.json:
        payload = {
            str(_relpath(path)): [
                {
                    "idx": o.block.idx,
                    "status": o.status,
                    "detail": o.detail,
                    "exit_code": o.exit_code,
                }
                for o in outs
            ]
            for path, outs in all_outcomes.items()
        }
        print(json.dumps(payload, indent=2))
        any_fail = any(o.status == "fail" for outs in all_outcomes.values() for o in outs)
        return 1 if any_fail else 0

    return _print_report(all_outcomes)


if __name__ == "__main__":
    sys.exit(main())
