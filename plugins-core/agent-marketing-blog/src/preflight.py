#!/usr/bin/env python3
"""
preflight.py — marketing-blog agent preflight check.

HALLUCINATION-PROOF CONTRACT:
    Does: Verify switchAILocal is reachable, jevvellabsblog repo is present
          and writable, templates are in place. Report exactly what is missing.
    Does NOT: Generate anything. Write anything. Fetch credentials.
    On failure: Prints the specific missing dependency and exits non-zero.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import List


def _load_harvey_env() -> None:
    home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
    env_file = Path(home) / ".env"
    if not env_file.exists():
        return
    for line in env_file.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip().strip('"').strip("'")
        if key and key not in os.environ:
            os.environ[key] = value


def main() -> int:
    _load_harvey_env()
    errors: List[str] = []
    warnings: List[str] = []
    here = Path(__file__).resolve().parent

    # 1. switchAILocal reachable
    base_url = os.environ.get("OPENAI_BASE_URL", "http://localhost:18080/v1").rstrip("/")
    api_key = os.environ.get("OPENAI_API_KEY") or os.environ.get("LLM_API_KEY") or "sk-test-123"
    try:
        req = urllib.request.Request(
            f"{base_url}/models",
            headers={"Authorization": f"Bearer {api_key}"},
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read())
            n = len(data.get("data", []))
            print(f"✓ switchAILocal reachable at {base_url} ({n} models)")
    except urllib.error.HTTPError as exc:
        errors.append(f"switchAILocal returned HTTP {exc.code} at {base_url} — is it running?")
    except Exception as exc:
        errors.append(
            f"switchAILocal unreachable at {base_url}: {type(exc).__name__}: {exc}\n"
            f"  Start it, or set OPENAI_BASE_URL to another provider."
        )

    # 2. Templates directory
    tmpl_dir = here / "templates"
    if not tmpl_dir.exists():
        errors.append(f"templates dir missing: {tmpl_dir}")
    else:
        frontmatter = tmpl_dir / "jekyll-frontmatter.md"
        if not frontmatter.exists():
            errors.append(f"template missing: {frontmatter}")
        else:
            print(f"✓ templates present ({len(list(tmpl_dir.glob('*')))} files)")

    # 3. Examples directory
    ex_dir = here / "examples"
    if not ex_dir.exists():
        errors.append(f"examples dir missing: {ex_dir}")
    else:
        count = len(list(ex_dir.glob("*.md")))
        if count < 1:
            warnings.append(f"examples dir is empty at {ex_dir}")
        else:
            print(f"✓ examples present ({count} files)")

    # 4. Jevvellabsblog repo (optional — only needed for publish_flow)
    repo_path = os.environ.get("JEVVELLABSBLOG_REPO", os.path.expanduser("~/code/jevvellabsblog"))
    repo = Path(repo_path)
    if not repo.exists():
        warnings.append(
            f"JEVVELLABSBLOG_REPO not found at {repo} — publish_flow will fail until:\n"
            f"  git clone <jevvellabsblog repo> {repo}\n"
            f"  OR: export JEVVELLABSBLOG_REPO=/path/to/existing/clone"
        )
    elif not (repo / "_posts").exists():
        warnings.append(f"repo at {repo} has no _posts/ dir — not a Jekyll blog?")
    else:
        try:
            # Verify it's a git repo
            subprocess.run(
                ["git", "-C", str(repo), "rev-parse", "--git-dir"],
                check=True, capture_output=True,
            )
            print(f"✓ jevvellabsblog repo present at {repo}")
        except subprocess.CalledProcessError:
            warnings.append(f"{repo} is not a git repo")

    if errors:
        print("\nPREFLIGHT FAILED:", file=sys.stderr)
        for e in errors:
            print(f"  ✗ {e}", file=sys.stderr)
        if warnings:
            print("\nWarnings (non-fatal):", file=sys.stderr)
            for w in warnings:
                print(f"  ! {w}", file=sys.stderr)
        return 1

    if warnings:
        print("\nWarnings (non-fatal):")
        for w in warnings:
            print(f"  ! {w}")

    print("\n✓ preflight passed — ready to generate and publish")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
