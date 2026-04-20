"""
Buddy accessories — eyes and hats.

Applied on top of sprite art to customize each buddy's appearance.
Eyes and hats are selected deterministically from the Mulberry32 rng in BuddyBones.
"""

from typing import List, Tuple

# ═══════════════════════════════════════════════════════════════
#  Eyes
# ═══════════════════════════════════════════════════════════════

EYES = {
    "default":  ("o", "o"),
    "cute":     ("^", "^"),
    "star":     ("*", "*"),
    "sleepy":   ("-", "-"),
    "wink":     ("o", "-"),
    "sparkle":  ("✦", "✦"),
}

EYE_NAMES = list(EYES.keys())

# ═══════════════════════════════════════════════════════════════
#  Hats
# ═══════════════════════════════════════════════════════════════

HATS = {
    "none":     [],
    "crown":    ["   👑    "],
    "top_hat":  ["   ___   ", "  |___|  "],
    "wizard":   ["   /\\   ", "  /  \\  "],
    "halo":     ["   oOo   "],
    "party":    ["    /|   "],
    "antenna":  ["    |    ", "    *    "],
    "pirate":   ["  ~~v~~  "],
}

HAT_NAMES = list(HATS.keys())


# ═══════════════════════════════════════════════════════════════
#  Application Functions
# ═══════════════════════════════════════════════════════════════

def apply_eyes(art_lines: List[str], eye_name: str) -> List[str]:
    """Substitute eye characters in sprite art.

    Replaces the first 'o' with the left eye and the second 'o' with
    the right eye. Only modifies lines that contain parenthesized faces.
    """
    if eye_name not in EYES or eye_name == "default":
        return list(art_lines)

    left, right = EYES[eye_name]
    result = []
    for line in art_lines:
        # Look for face patterns with 'o' eyes — replace first two occurrences
        # Only operate on lines that look like face lines (contain parens or similar)
        if "o" in line and ("(" in line or "{" in line or "\\" in line):
            new_line = line.replace("o", left, 1)
            new_line = new_line.replace("o", right, 1)
            result.append(new_line)
        else:
            result.append(line)
    return result


def apply_hat(art_lines: List[str], hat_name: str) -> List[str]:
    """Prepend hat ASCII art lines above the sprite."""
    if hat_name not in HATS or hat_name == "none":
        return list(art_lines)

    hat_lines = HATS[hat_name]
    return hat_lines + list(art_lines)
