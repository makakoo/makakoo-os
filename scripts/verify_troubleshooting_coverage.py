#!/usr/bin/env python3
"""Verify every user-facing Rust diagnostic string is indexed in symptoms.md.

Scans the Rust workspace under `makakoo-core/src/`, `makakoo/src/`, and
`makakoo-mcp/src/` for `bail!`, `anyhow!`, `print_error`, `print_warn`, and
`InvalidInput` invocations, extracts their leading string literals, and asserts
each appears in `docs/troubleshooting/symptoms.md`.

Diagnostics that predate the expanded scanner are recorded explicitly in
`scripts/troubleshooting_legacy_baseline.json`. They remain visible debt, but
cannot hide any newly introduced diagnostic.

A string is considered "present" in symptoms.md if any significant word from
the Rust string appears in the document — this is deliberately loose because
the Rust strings are often format templates (`"failed to read {}: {e}"`) and
the doc typically uses a normalized form (`"failed to read <path>: <os-error>"`).

Exits 0 if coverage is complete; 1 with a list of missing strings otherwise.

Usage:
  python3 scripts/verify_troubleshooting_coverage.py
  python3 scripts/verify_troubleshooting_coverage.py --json

Add an entry to `_KNOWN_GAPS` below only for an internal diagnostic that cannot
reach an operator and therefore intentionally has no troubleshooting entry.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_ROOTS = [REPO_ROOT / s for s in ("makakoo-core/src", "makakoo/src", "makakoo-mcp/src")]
SYMPTOMS_MD = REPO_ROOT / "docs" / "troubleshooting" / "symptoms.md"
LEGACY_BASELINE = REPO_ROOT / "scripts" / "troubleshooting_legacy_baseline.json"

# Strings we know about but deliberately don't index — mostly internal
# developer-facing panics or very context-dependent format templates.
_KNOWN_GAPS = frozenset(
    {
        "failed to read {}: {}",
        "failed to write bootstrap cache: {e}",
        "reading {}: {e}",
        "trust file {}: {e}",
        "GET {url}: {e}",
        "http {status}: {text}",
        "mascot: {name}",
        'agent show: serialise: {}',
        'could not parse access_key + secret_key from garage CLI output:\\n---\\n{out}\\n---',
        'could not parse node id from `garage node id -q` output:\\n{node_id_raw}',
        'daemon-reload: {e}',
        'failed to create canonical bootstrap dir {}: {}',
        'failed to write canonical bootstrap {}: {}',
        'launchctl bootstrap: {e}',
        'plist generation: {e}',
        'plist write: {e}',
        'plist: {e}',
        'read audit log: {e}',
        'read current_exe: {e}',
        'resolve bot token: {}',
        'resolve slack app token: {}',
        'resolve slack bot token: {}',
        'serialize audit event: {err}',
        'serialize outcome: {e}',
        'slot load: {e}',
        'supervisor: {e}',
        'tokio runtime: {e}',
        'unit generation: {e}',
    }
)

# Strings that are too short to be meaningful error indicators.
_MIN_LEN = 8

_DIAGNOSTIC_RE = re.compile(
    r'(?:bail!|anyhow!|print_error|print_warn|InvalidInput)\s*\(\s*'
    r'(?:format!\s*\(\s*)?"((?:\\.|[^"\\]){%d,})"' % _MIN_LEN,
    re.DOTALL,
)


def extract_diagnostics(text: str) -> set[str]:
    return {
        match.group(1).replace(r'\"', '"')
        for match in _DIAGNOSTIC_RE.finditer(text)
    }


def scanner_self_check() -> None:
    fixture = '''
        anyhow::bail!("bail diagnostic example");
        anyhow::anyhow!("anyhow diagnostic example");
        output::print_error(format!("print error example: {}", value));
        output::print_warn("print warning example");
        MakakooError::InvalidInput(format!("invalid input example: {}", value));
    '''
    expected = {
        "bail diagnostic example",
        "anyhow diagnostic example",
        "print error example: {}",
        "print warning example",
        "invalid input example: {}",
    }
    actual = extract_diagnostics(fixture)
    if actual != expected:
        raise RuntimeError(f"diagnostic scanner self-check failed: {actual!r}")


def collect_rust_strings() -> set[str]:
    found: set[str] = set()
    for root in RUST_ROOTS:
        if not root.exists():
            continue
        for path in root.rglob("*.rs"):
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            found.update(extract_diagnostics(text))
    return found


def collect_rust_strings_at_revision(revision: str) -> set[str]:
    """Read diagnostics from the frozen pre-v0.3 Git tree.

    This provenance check prevents a contributor from hiding a new diagnostic
    by hand-appending it to the legacy allowlist.
    """
    try:
        paths = subprocess.run(
            ["git", "ls-tree", "-r", "--name-only", revision, "--", *[str(root.relative_to(REPO_ROOT)) for root in RUST_ROOTS]],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError(f"cannot read legacy baseline revision {revision}: {error}") from error

    found: set[str] = set()
    for relative in paths:
        if not relative.endswith(".rs"):
            continue
        try:
            text = subprocess.run(
                ["git", "show", f"{revision}:{relative}"],
                cwd=REPO_ROOT,
                check=True,
                capture_output=True,
                text=True,
            ).stdout
        except (OSError, subprocess.CalledProcessError) as error:
            raise RuntimeError(f"cannot read {relative} at {revision}: {error}") from error
        found.update(extract_diagnostics(text))
    return found


def load_legacy_baseline() -> tuple[str, set[str]]:
    try:
        payload = json.loads(LEGACY_BASELINE.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, TypeError) as error:
        raise RuntimeError(f"cannot read legacy diagnostic baseline: {error}") from error
    if not isinstance(payload, dict):
        raise RuntimeError("legacy diagnostic baseline must be an object with provenance")
    revision = payload.get("source_commit")
    diagnostics = payload.get("diagnostics")
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise RuntimeError("legacy baseline source_commit must be a full Git SHA")
    if not isinstance(diagnostics, list) or not all(isinstance(item, str) for item in diagnostics):
        raise RuntimeError("legacy baseline diagnostics must be a string array")
    if len(diagnostics) != len(set(diagnostics)):
        raise RuntimeError("legacy baseline diagnostics contain duplicates")
    source_strings = collect_rust_strings_at_revision(revision)
    unproven = set(diagnostics) - source_strings
    if unproven:
        preview = ", ".join(repr(item) for item in sorted(unproven)[:5])
        raise RuntimeError(
            f"legacy baseline contains {len(unproven)} diagnostic(s) absent from "
            f"source commit {revision}: {preview}"
        )
    return revision, set(diagnostics)


_WORD_RE = re.compile(r"[A-Za-z][A-Za-z0-9_-]{2,}")


def _key_words(s: str) -> set[str]:
    """Return the set of meaningful words from a Rust format string, excluding
    format placeholders and tiny stopwords."""
    # Strip format placeholders like {}, {e}, {foo}.
    stripped = re.sub(r"\{[^}]*\}", " ", s)
    words = {w.lower() for w in _WORD_RE.findall(stripped)}
    # Drop very generic English + common-in-CLI-errors filler words.
    words -= {
        "the", "and", "for", "with", "not", "from", "that", "this", "into",
        "run", "use", "pass", "set", "has", "have", "are", "was", "one",
        "contains", "containing", "accepts", "accepted", "remote", "install",
        "knows", "name", "you", "your", "them", "some", "any", "all",
    }
    return words


def is_documented(rust_str: str, symptoms_text: str) -> bool:
    """True if the key words from `rust_str` all appear somewhere in
    symptoms.md. We normalize to lowercase for the match."""
    keys = _key_words(rust_str)
    if not keys:
        return True  # nothing distinctive to match — skip
    st = symptoms_text.lower()
    return all(k in st for k in keys)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    scanner_self_check()

    if not SYMPTOMS_MD.exists():
        print(f"error: {SYMPTOMS_MD} does not exist", file=sys.stderr)
        return 2

    rust_strings = collect_rust_strings()
    symptoms_text = SYMPTOMS_MD.read_text(encoding="utf-8")
    try:
        baseline_revision, legacy_baseline = load_legacy_baseline()
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    missing = []
    for s in sorted(rust_strings):
        if s in _KNOWN_GAPS or s in legacy_baseline:
            continue
        if is_documented(s, symptoms_text):
            continue
        missing.append(s)

    report = {
        "rust_strings_found": len(rust_strings),
        "known_gaps": len(_KNOWN_GAPS),
        "legacy_baseline": len(legacy_baseline),
        "legacy_baseline_source_commit": baseline_revision,
        "stale_legacy_baseline": sorted(legacy_baseline - rust_strings),
        "missing_from_symptoms_md": missing,
    }

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"Rust user-facing diagnostics scanned: {report['rust_strings_found']}")
        print(f"Internal exclusions:                {report['known_gaps']}")
        print(f"Pre-v0.3 legacy baseline:           {report['legacy_baseline']}")
        print(f"Missing from symptoms.md:           {len(missing)}")
        if missing:
            print("\nAdd these to docs/troubleshooting/symptoms.md:")
            for s in missing:
                print(f"  - {s!r}")
        else:
            print("\nOK — every non-baselined Rust diagnostic has a symptoms.md entry.")

    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
