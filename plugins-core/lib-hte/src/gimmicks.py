"""
Mascot Gimmicks — Random ASCII personality moments during Harvey usage.

Surfaces Olibia, nursery mascots, and buddy sprites during normal operations.
Context-aware: memory features → Olibia, SANCHO → dream mascots, superbrain → wisdom.

Design rules (negotiated with lope team):
  - 10% default rate (HARVEY_GIMMICK_RATE env override)
  - Static art only (no animations — zero latency impact)
  - Read-only: never writes nursery.json or buddy.json
  - Max 1 gimmick per process invocation (_SHOWN flag)
  - Respects IS_TTY + SUPPORTS_COLOR from rendering_standards
  - Side-by-side render: 3-line art left, one-liner on middle line right
  - Art lines normalized to uniform width before compositing

Usage:
    from core.terminal.gimmicks import maybe_gimmick
    maybe_gimmick("search")                    # 10% chance
    maybe_gimmick("memory")                    # 10% chance, Olibia-specific
    maybe_gimmick("celebrate", force=True)     # Always show
"""

import json
import os
import random
import sys
from pathlib import Path
from typing import Dict, List, Optional, Tuple

# ── Module state ──────────────────────────────────────────────────
_SHOWN = False  # Max 1 gimmick per process invocation

# ── Configuration ─────────────────────────────────────────────────
_RATE = float(os.environ.get("HARVEY_GIMMICK_RATE", "0.10"))
_OFF = os.environ.get("HARVEY_GIMMICK_OFF", "").strip() in ("1", "true", "yes")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

# ── Rarity colors (ANSI) ─────────────────────────────────────────
RARITY_COLORS = {
    "Common": "\033[37m",      # white
    "Uncommon": "\033[32m",    # green
    "Rare": "\033[34m",        # blue
    "Epic": "\033[35m",        # purple
    "Legendary": "\033[33m",   # gold
}
RESET = "\033[0m"
DIM = "\033[2m"

# ── Olibia's ASCII art ───────────────────────────────────────────
OLIBIA_ART = [
    "  {o,o}  ",
    "  |)__)  ",
    '  -"-"-  ',
]

# ── Contextual one-liner pools ────────────────────────────────────
GIMMICK_LINES: Dict[str, List[str]] = {
    "search": [
        "{name} sniffs the knowledge graph...",
        "{name} digs through the Brain...",
        "shh... {name} is thinking...",
        "{name} found something!",
        "{name} peers into the archives...",
        "the Brain hums. {name} listens.",
        "{name} follows the wikilinks...",
        "{name} checks the indexes...",
    ],
    "memory": [
        "🦉 {name} watches over your memories",
        "🦉 {name} remembers so you don't have to",
        "🦉 another memory secured. {name} approves.",
        "🦉 {name} guards the promoted memories",
        "🦉 the owl sees all. {name} nods.",
        "🦉 {name}: memories are safe",
    ],
    "sancho": [
        "{name} woke up for this task",
        "{name} yawns... but gets to work",
        "{name} runs maintenance quietly",
        "{name} checks the schedule...",
        "{name} stretches and ticks",
        "{name} handles the boring stuff",
    ],
    "dream": [
        "{name} dreams of better clusters...",
        "zzz... {name} consolidates in sleep",
        "{name} mumbles about knowledge graphs...",
        "{name} turns over, dreaming of embeddings",
    ],
    "celebrate": [
        "{name} does a little dance!",
        "{name} is SO proud right now",
        "another win. {name} marks the occasion.",
        "{name} squeaks with delight!",
        "{name} beams quietly",
        "shipped. {name} noticed.",
    ],
    "error": [
        "🦉 {name} frowns at this...",
        "🦉 {name} tilts head. something's off.",
        "🦉 {name} stands watch. we'll fix this.",
    ],
    "boot": [
        "{name} stretches and yawns",
        "{name} is here. ready.",
        "{name} wakes up with Harvey",
        "{name} blinks awake",
    ],
}

# ── Context → mascot source mapping ──────────────────────────────
# "olibia" = always Olibia, "nursery" = random nursery mascot, "buddy" = personal buddy
CONTEXT_SOURCE = {
    "search": "buddy",
    "memory": "olibia",
    "sancho": "nursery",
    "dream": "nursery",
    "celebrate": "nursery",
    "error": "olibia",
    "boot": "buddy",
}


def render_gimmick(context: str, force: bool = False) -> Optional[str]:
    """
    Maybe produce a mascot gimmick as a plain-text string (no ANSI, no stderr).

    For MCP / non-TTY consumers that embed the art in response text.
    Returns the rendered string if the gimmick fires, else None.

    Same gate chain as maybe_gimmick minus IS_TTY (MCP is never a TTY).
    Uses a cooldown timer instead of _SHOWN (MCP servers are long-lived —
    a single-fire flag would kill gimmicks after the first tool call).
    """
    if _OFF:
        return None
    if not force and not _mcp_cooldown_ok():
        return None
    if not force and random.random() > _RATE:
        return None

    mascot = _pick_mascot(context)
    if not mascot:
        return None

    lines = GIMMICK_LINES.get(context, GIMMICK_LINES["search"])
    line = random.choice(lines).format(name=mascot["name"])

    # Plain-text render (no ANSI color for MCP consumers)
    output = _render(mascot["art"], line, "", False)
    _mcp_cooldown_mark()
    return output


# ── MCP cooldown (replaces _SHOWN for long-lived processes) ──
_MCP_LAST_SHOWN: float = 0.0
_MCP_COOLDOWN_SEC: float = 300.0  # 5 minutes between gimmicks


def _mcp_cooldown_ok() -> bool:
    """True if enough time passed since last MCP gimmick."""
    import time
    return (time.time() - _MCP_LAST_SHOWN) >= _MCP_COOLDOWN_SEC


def _mcp_cooldown_mark() -> None:
    """Record that a gimmick just fired via MCP."""
    global _MCP_LAST_SHOWN
    import time
    _MCP_LAST_SHOWN = time.time()


def maybe_gimmick(context: str, force: bool = False) -> bool:
    """
    Maybe show a mascot gimmick. Returns True if shown.

    Args:
        context: Feature context — search, memory, sancho, dream, celebrate, error, boot
        force: If True, always show (ignores rate + _SHOWN flag)
    """
    global _SHOWN

    # Hard gates
    if _OFF:
        return False
    if not force and _SHOWN:
        return False

    # TTY check — use rendering_standards if available, else basic check
    try:
        from core.terminal.rendering_standards import IS_TTY, SUPPORTS_COLOR
        if not IS_TTY:
            return False
        use_color = SUPPORTS_COLOR
    except ImportError:
        if not sys.stdout.isatty():
            return False
        use_color = True

    # Rate check
    if not force and random.random() > _RATE:
        return False

    # Pick mascot
    mascot = _pick_mascot(context)
    if not mascot:
        return False

    # Pick one-liner
    lines = GIMMICK_LINES.get(context, GIMMICK_LINES["search"])
    line = random.choice(lines).format(name=mascot["name"])

    # Render and print
    output = _render(mascot["art"], line, mascot.get("color", ""), use_color)
    print(output, file=sys.stderr)

    _SHOWN = True
    return True


def _pick_mascot(context: str) -> Optional[dict]:
    """
    Pick a context-appropriate mascot with LEGO-composed art.
    Species comes from mascot source, eyes/accessories from context recipe.
    """
    source = CONTEXT_SOURCE.get(context, "nursery")

    if source == "olibia":
        art = _compose_art("owl", context)
        return {"name": "Olibia", "art": art, "color": RARITY_COLORS.get("Epic", "")}
    elif source == "nursery":
        mascot = _random_nursery_mascot_lego(context)
        if mascot:
            return mascot
    elif source == "buddy":
        buddy = _get_buddy_art_lego(context)
        if buddy:
            return buddy

    # Fallback: Olibia with LEGO composition
    art = _compose_art("owl", context)
    return {"name": "Olibia", "art": art, "color": RARITY_COLORS.get("Epic", "")}


def _compose_art(species: str, context: str) -> List[str]:
    """Compose LEGO art for a species + context. Falls back to static art."""
    try:
        from core.terminal.lego_art import compose
        return compose(species, context)
    except ImportError:
        # Fallback to static Olibia
        return list(OLIBIA_ART)


def _random_nursery_mascot_lego(context: str) -> Optional[dict]:
    """Load a random nursery mascot, compose LEGO art from context."""
    try:
        nursery_path = Path(HARVEY_HOME) / "data" / "nursery.json"
        if not nursery_path.exists():
            return None
        data = json.loads(nursery_path.read_text(encoding="utf-8"))
        mascots = data.get("mascots", [])
        if not mascots:
            return None
        m = random.choice(mascots)
        name = m.get("name", "???")
        species_name = m.get("species_name", "fox")

        # Compose LEGO art
        try:
            from core.terminal.lego_art import compose_for_mascot
            art = compose_for_mascot(name, species_name, context)
        except ImportError:
            art = m.get("art", ["  ???  ", " (?.?) ", "  ???  "])

        return {
            "name": name,
            "art": art,
            "color": RARITY_COLORS.get(m.get("rarity", "Common"), ""),
        }
    except Exception:
        return None


def _get_buddy_art_lego(context: str) -> Optional[dict]:
    """Load personal buddy, compose LEGO art from context."""
    try:
        buddy_path = Path(HARVEY_HOME) / "data" / "buddy.json"
        if not buddy_path.exists():
            return None
        data = json.loads(buddy_path.read_text(encoding="utf-8"))
        name = data.get("name", "Buddy")
        species = data.get("species", "fox")
        rarity = data.get("rarity", "Common")

        # Compose LEGO art
        art = _compose_art(species, context)

        return {
            "name": name,
            "art": art,
            "color": RARITY_COLORS.get(rarity, ""),
        }
    except Exception:
        return None


def _render(art_lines: List[str], one_liner: str, color: str, use_color: bool) -> str:
    """
    Render 3-line ASCII art with one-liner on the middle line (right side).

    Art lines are normalized to uniform width before compositing.
    """
    # Normalize art line widths (lope fix: pad procedural nursery art)
    max_width = max((len(line) for line in art_lines), default=10)
    padded = [line.ljust(max_width) for line in art_lines]

    gap = "  "
    lines = []
    for i, art_line in enumerate(padded):
        if use_color and color:
            art_str = f"{color}{art_line}{RESET}"
        else:
            art_str = art_line

        if i == 1:
            # Middle line: art + one-liner
            if use_color:
                lines.append(f"{art_str}{gap}{DIM}{one_liner}{RESET}")
            else:
                lines.append(f"{art_str}{gap}{one_liner}")
        else:
            lines.append(art_str)

    return "\n" + "\n".join(lines) + "\n"
