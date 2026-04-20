"""
Canonical HARVEY_HOME resolver.

The `~` tilde bug has bitten Harvey three times. The root cause is simple:
`os.environ.get("HARVEY_HOME", "~/MAKAKOO")` returns `"~/MAKAKOO"` literally if
the env var is unset or set to a tilde-prefixed value — and any code that
joins paths onto that string ends up creating ghost directories named `~`
in whatever cwd the process started in.

Rule: never read HARVEY_HOME directly. Always go through `harvey_home()`.

Every caller that needs HARVEY_HOME should use:

    from core.paths import harvey_home
    HARVEY_HOME = harvey_home()

This function:
  1. Reads the env var, defaulting to ~/MAKAKOO
  2. Always expands user (~/MAKAKOO → /Users/foo/HARVEY)
  3. Always resolves to absolute
  4. Returns a plain string (for os.path.join compatibility)

Modules that can't import from `core` (e.g. the logseq-brain skill scripts
that run standalone via sys.path insertion) can inline the same logic —
there's a copy-pastable reference in `_inline_fallback()` below.
"""

from __future__ import annotations

import os


def harvey_home() -> str:
    """Return absolute HARVEY_HOME path. Never returns a tilde-prefixed string."""
    raw = os.environ.get("HARVEY_HOME", "~/MAKAKOO")
    return os.path.abspath(os.path.expanduser(raw))


def brain_dir() -> str:
    return os.path.join(harvey_home(), "data", "Brain")


def data_dir() -> str:
    return os.path.join(harvey_home(), "data")


def _inline_fallback() -> str:
    """Reference implementation for modules that can't import core.paths.

    Keep this byte-compatible with harvey_home() above. If you change one,
    change the other. The tilde bug comes back if you forget to expanduser.
    """
    raw = os.environ.get("HARVEY_HOME", "~/MAKAKOO")
    return os.path.abspath(os.path.expanduser(raw))
