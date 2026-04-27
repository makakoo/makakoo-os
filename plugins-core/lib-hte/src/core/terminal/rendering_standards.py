"""Cross-CLI rendering standards — source of truth for terminal capability detection.

All CLI wizards use these constants for safe, degraded rendering:
- Color tier detection (truecolor → 256 → plain)
- Unicode support (box chars with ASCII fallback)
- TTY detection (never animate in pipes)
- Terminal width/height safe defaults
"""

import os
import sys
import locale
import shutil

# ── Terminal Capability Detection ───────────────────────────────────────────

TERM_WIDTH = min(shutil.get_terminal_size(fallback=(80, 24)).columns, 120)
TERM_HEIGHT = shutil.get_terminal_size(fallback=(80, 24)).lines

IS_TTY = sys.stdout.isatty()
IS_STDERR_TTY = sys.stderr.isatty()

# NO_COLOR env var disables color globally (https://no-color.org/)
NO_COLOR = os.environ.get("NO_COLOR") is not None
DUMB_TERM = os.environ.get("TERM") == "dumb"

SUPPORTS_COLOR = IS_TTY and not NO_COLOR and not DUMB_TERM
SUPPORTS_ANIMATION = IS_TTY and SUPPORTS_COLOR  # never animate in piped/redirected output

# Detect Unicode support — UTF-8 locale needed for box chars
try:
    locale_encoding = locale.getpreferredencoding()
    SUPPORTS_UNICODE = locale_encoding.upper() in ("UTF-8", "UTF8", "UTF_8")
except Exception:
    SUPPORTS_UNICODE = True  # assume UTF-8 on modern systems

# ── Color Tier Detection ───────────────────────────────────────────────────

# COLORTERM env var signals truecolor (24-bit) support
COLORTERM = os.environ.get("COLORTERM", "")
if COLORTERM in ("truecolor", "24bit"):
    COLOR_TIER = "truecolor"
elif SUPPORTS_COLOR:
    COLOR_TIER = "256"
else:
    COLOR_TIER = "plain"

# ── Safe Box-Drawing Characters ────────────────────────────────────────────

# Full set: use if Unicode supported
BOXES_UNICODE = {
    "single": {
        "tl": "┌", "tr": "┐", "bl": "└", "br": "┘",
        "h": "─", "v": "│",
        "ml": "├", "mr": "┤", "mm": "┼",
        "mt": "┬", "mb": "┴",
    },
    "bold": {
        "tl": "╔", "tr": "╗", "bl": "╚", "br": "╝",
        "h": "═", "v": "║",
        "ml": "╠", "mr": "╣", "mm": "╬",
        "mt": "╦", "mb": "╩",
    },
    "double": {
        "tl": "╒", "tr": "╕", "bl": "╘", "br": "╙",
        "h": "═", "v": "│",
        "ml": "╞", "mr": "╡", "mm": "╪",
        "mt": "╤", "mb": "╧",
    },
}

# ASCII fallback: use if no Unicode
BOXES_ASCII = {
    "single": {
        "tl": "+", "tr": "+", "bl": "+", "br": "+",
        "h": "-", "v": "|",
        "ml": "+", "mr": "+", "mm": "+",
        "mt": "+", "mb": "+",
    },
    "bold": {
        "tl": "+", "tr": "+", "bl": "+", "br": "+",
        "h": "=", "v": "|",
        "ml": "+", "mr": "+", "mm": "+",
        "mt": "+", "mb": "+",
    },
    "double": {
        "tl": "+", "tr": "+", "bl": "+", "br": "+",
        "h": "=", "v": "|",
        "ml": "+", "mr": "+", "mm": "+",
        "mt": "+", "mb": "+",
    },
}

# Select based on Unicode support
BOX = BOXES_UNICODE if SUPPORTS_UNICODE else BOXES_ASCII

# ── ANSI Color Codes ───────────────────────────────────────────────────────

if COLOR_TIER == "truecolor":
    # Truecolor (24-bit): \033[38;2;R;G;Bm foreground, \033[48;2;R;G;Bm background
    def rgb(r, g, b):
        return f"\033[38;2;{r};{g};{b}m"

    def bg_rgb(r, g, b):
        return f"\033[48;2;{r};{g};{b}m"

    # Common colors as truecolor
    COLORS = {
        "reset": "\033[0m",
        "bold": "\033[1m",
        "dim": "\033[2m",
        "italic": "\033[3m",
        "underline": "\033[4m",
        "blink": "\033[5m",
        "reverse": "\033[7m",
        "hidden": "\033[8m",
        "strikethrough": "\033[9m",

        # Foreground colors
        "black": "\033[38;2;0;0;0m",
        "red": "\033[38;2;220;20;60m",  # crimson
        "green": "\033[38;2;34;139;34m",  # forest green
        "yellow": "\033[38;2;255;215;0m",  # gold
        "blue": "\033[38;2;30;144;255m",  # dodger blue
        "magenta": "\033[38;2;186;85;211m",  # medium orchid
        "cyan": "\033[38;2;64;224;208m",  # turquoise
        "white": "\033[38;2;255;255;255m",
        "gray": "\033[38;2;128;128;128m",

        # Status colors
        "success": "\033[38;2;34;139;34m",  # forest green
        "error": "\033[38;2;220;20;60m",  # crimson
        "warning": "\033[38;2;255;215;0m",  # gold
        "info": "\033[38;2;30;144;255m",  # dodger blue
    }

elif COLOR_TIER == "256":
    # 256-color palette
    def rgb(r, g, b):
        # Approximate RGB to 256-color index
        return f"\033[38;5;{_rgb_to_256(r, g, b)}m"

    def bg_rgb(r, g, b):
        return f"\033[48;5;{_rgb_to_256(r, g, b)}m"

    def _rgb_to_256(r, g, b):
        # Simple approximation: convert RGB to 216-color cube (6x6x6)
        ri = round(r / 255 * 5)
        gi = round(g / 255 * 5)
        bi = round(b / 255 * 5)
        return 16 + 36 * ri + 6 * gi + bi

    COLORS = {
        "reset": "\033[0m",
        "bold": "\033[1m",
        "dim": "\033[2m",
        "italic": "\033[3m",
        "underline": "\033[4m",
        "blink": "\033[5m",
        "reverse": "\033[7m",
        "hidden": "\033[8m",
        "strikethrough": "\033[9m",

        # 256-color palette indices
        "black": "\033[38;5;16m",
        "red": "\033[38;5;9m",
        "green": "\033[38;5;10m",
        "yellow": "\033[38;5;11m",
        "blue": "\033[38;5;12m",
        "magenta": "\033[38;5;13m",
        "cyan": "\033[38;5;14m",
        "white": "\033[38;5;15m",
        "gray": "\033[38;5;8m",

        # Status colors
        "success": "\033[38;5;10m",
        "error": "\033[38;5;9m",
        "warning": "\033[38;5;11m",
        "info": "\033[38;5;12m",
    }

else:
    # Plain text — no colors at all
    def rgb(r, g, b):
        return ""

    def bg_rgb(r, g, b):
        return ""

    COLORS = {
        "reset": "",
        "bold": "",
        "dim": "",
        "italic": "",
        "underline": "",
        "blink": "",
        "reverse": "",
        "hidden": "",
        "strikethrough": "",

        # No colors
        "black": "",
        "red": "",
        "green": "",
        "yellow": "",
        "blue": "",
        "magenta": "",
        "cyan": "",
        "white": "",
        "gray": "",

        # Status colors
        "success": "",
        "error": "",
        "warning": "",
        "info": "",
    }

# ── Status to Color Mapping ────────────────────────────────────────────────

STATUS_COLORS = {
    "pending": COLORS["yellow"],
    "ready": COLORS["info"],
    "running": COLORS["blue"],
    "blocked": COLORS["gray"],
    "complete": COLORS["success"],
    "failed": COLORS["error"],
    "success": COLORS["success"],
    "error": COLORS["error"],
    "warning": COLORS["warning"],
    "info": COLORS["info"],
    "low": COLORS["success"],
    "medium": COLORS["warning"],
    "high": COLORS["error"],
    "forbidden": COLORS["red"],
}

# ── Symbols ────────────────────────────────────────────────────────────────

# Unicode symbols with ASCII fallback
SYMBOLS = {
    "checkmark": "✓" if SUPPORTS_UNICODE else "✓",
    "cross": "✗" if SUPPORTS_UNICODE else "X",
    "dot": "●" if SUPPORTS_UNICODE else "*",
    "bullet": "•" if SUPPORTS_UNICODE else "-",
    "arrow_right": "→" if SUPPORTS_UNICODE else "=>",
    "arrow_left": "←" if SUPPORTS_UNICODE else "<=",
    "arrow_up": "↑" if SUPPORTS_UNICODE else "^",
    "arrow_down": "↓" if SUPPORTS_UNICODE else "v",
    "hourglass": "⏳" if SUPPORTS_UNICODE else "[*]",
    "gear": "⚙" if SUPPORTS_UNICODE else "[*]",
    "warning_sign": "⚠" if SUPPORTS_UNICODE else "[!]",
    "prohibition": "🚫" if SUPPORTS_UNICODE else "[X]",
}

# Spinner frames: unicode with ASCII fallback
SPINNER_FRAMES = (
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] if SUPPORTS_UNICODE
    else ["|", "/", "-", "\\"]
)

# Progress bar chars
PROGRESS_FULL = "▓" if SUPPORTS_UNICODE else "#"
PROGRESS_EMPTY = "░" if SUPPORTS_UNICODE else "-"

# ── Safe Cursor Movement ───────────────────────────────────────────────────

def cursor_up(n):
    """Move cursor up n lines (for in-place animation)."""
    if IS_TTY:
        return f"\033[{n}A"
    return ""

def cursor_home():
    """Move cursor to home position."""
    if IS_TTY:
        return "\033[H"
    return ""

def clear_line():
    """Clear current line."""
    if IS_TTY:
        return "\033[2K"
    return ""

def clear_screen():
    """Clear entire screen."""
    if IS_TTY:
        return "\033[2J"
    return ""

# ── Detection Summary ──────────────────────────────────────────────────────

DEBUG_INFO = f"""
Terminal Capability Detection:
  TERM_WIDTH: {TERM_WIDTH}
  TERM_HEIGHT: {TERM_HEIGHT}
  IS_TTY: {IS_TTY}
  SUPPORTS_COLOR: {SUPPORTS_COLOR}
  SUPPORTS_UNICODE: {SUPPORTS_UNICODE}
  SUPPORTS_ANIMATION: {SUPPORTS_ANIMATION}
  COLOR_TIER: {COLOR_TIER}
  NO_COLOR: {NO_COLOR}
  DUMB_TERM: {DUMB_TERM}
"""
