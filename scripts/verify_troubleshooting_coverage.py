#!/usr/bin/env python3
"""Verify every Rust-emitted error/bail!/anyhow! string is indexed in symptoms.md.

Greps the Rust workspace under `makakoo-core/src/`, `makakoo/src/`, and
`makakoo-mcp/src/` for `bail!("…")` and `anyhow!("…")` invocations,
extracts the string literals, and asserts each appears in
`docs/troubleshooting/symptoms.md`.

A string is considered "present" in symptoms.md if any significant word from
the Rust string appears in the document — this is deliberately loose because
the Rust strings are often format templates (`"failed to read {}: {e}"`) and
the doc typically uses a normalized form (`"failed to read <path>: <os-error>"`).

Exits 0 if coverage is complete; 1 with a list of missing strings otherwise.

Usage:
  python3 scripts/verify_troubleshooting_coverage.py
  python3 scripts/verify_troubleshooting_coverage.py --json

Known gaps are listed in the top of symptoms.md under a comment. Add an entry
to `_KNOWN_GAPS` below to silence a warning for a string that's intentionally
not documented (e.g. internal panics that should never reach a user).
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_ROOTS = [REPO_ROOT / s for s in ("makakoo-core/src", "makakoo/src", "makakoo-mcp/src")]
SYMPTOMS_MD = REPO_ROOT / "docs" / "troubleshooting" / "symptoms.md"

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
    }
)

# Strings that are too short to be meaningful error indicators.
_MIN_LEN = 8

_BAIL_RE = re.compile(r'(?:bail!|anyhow!)\(\s*"([^"]{%d,})"' % _MIN_LEN)


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
            for m in _BAIL_RE.finditer(text):
                found.add(m.group(1))
    return found


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

    if not SYMPTOMS_MD.exists():
        print(f"error: {SYMPTOMS_MD} does not exist", file=sys.stderr)
        return 2

    rust_strings = collect_rust_strings()
    symptoms_text = SYMPTOMS_MD.read_text(encoding="utf-8")

    missing = []
    for s in sorted(rust_strings):
        if s in _KNOWN_GAPS:
            continue
        if is_documented(s, symptoms_text):
            continue
        missing.append(s)

    report = {
        "rust_strings_found": len(rust_strings),
        "known_gaps": len(_KNOWN_GAPS),
        "missing_from_symptoms_md": missing,
    }

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"Rust bail!/anyhow! strings scanned: {report['rust_strings_found']}")
        print(f"Explicit known gaps:                {report['known_gaps']}")
        print(f"Missing from symptoms.md:           {len(missing)}")
        if missing:
            print("\nAdd these to docs/troubleshooting/symptoms.md:")
            for s in missing:
                print(f"  - {s!r}")
        else:
            print("\nOK — every Rust error string has a symptoms.md entry.")

    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
