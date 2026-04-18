"""
LEGO ASCII Art — Composable character builder for Harvey mascot gimmicks.

Instead of static pre-built sprites, assembles characters from parts:
  HEAD (species) + EYES (context) + BODY (species) + FEET (species)
  + ACCESSORY (context) + FLOAT (context)

Every display is unique because species, eyes, accessory, and float
are chosen independently by context.

Usage:
    from core.terminal.lego_art import compose
    art_lines = compose(species="fox", context="search")
    # Returns 3 lines of assembled ASCII art

    art_lines = compose(species="owl", context="dream")
    # Olibia with dreamy eyes and zzz floating
"""

import random
from typing import Dict, List, Optional, Tuple


# ═══════════════════════════════════════════════════════════════
#  PARTS CATALOG
# ═══════════════════════════════════════════════════════════════

# ── Heads (top line, species-driven) ──────────────────────────

HEADS: Dict[str, str] = {
    "fox":       "  /\\_/\\  ",
    "cat":       "  /\\_/\\  ",
    "wolf":      " /\\___/\\ ",
    "panther":   "  /\\_/\\  ",
    "owl":       "  {___}  ",
    "raven":     "  \\___/  ",
    "eagle":     "  \\v v/  ",
    "hawk":      "  \\\\v//  ",
    "bat":       " /\\\\_/\\\\ ",
    "bear":      " (\")(\")",
    "octopus":   "  ,--,   ",
    "dragon":    "  /\\_/\\~ ",
    "phoenix":   "  \\|/|/  ",
    "shark":     "  ___/\\  ",
    "spider":    " /\\ /\\  ",
    "snake":     "  _____  ",
    "scorpion":  "  __/~   ",
    "kraken":    "  \\   /  ",
}

# ── Body wraps (middle line frame, species-driven) ────────────
# {E} placeholder gets replaced with eye expression

BODIES: Dict[str, str] = {
    "fox":       " ( {E} ) ",
    "cat":       " ( {E} ) ",
    "wolf":      " ( {E} )",
    "panther":   " ( {E} ) ",
    "owl":       "  |){E}| ",
    "raven":     "  ({E})  ",
    "eagle":     "  ({E})  ",
    "hawk":      "  ({E})  ",
    "bat":       "  ({E})  ",
    "bear":      " ({E} ) ",
    "octopus":   " ({E} ) ",
    "dragon":    " ( {E} ) ",
    "phoenix":   "  ({E})  ",
    "shark":     " ({E}  >)",
    "spider":    " \\({E})/ ",
    "snake":     " /~{E}~\\ ",
    "scorpion":  " ({E} ) ",
    "kraken":    " \\({E})/ ",
}

# ── Feet (bottom line, species-driven) ────────────────────────

FEET: Dict[str, str] = {
    "fox":       "  > ^ <  ",
    "cat":       '  (")_(") ',
    "wolf":      "  U     U ",
    "panther":   "  ( u u) ",
    "owl":       '  -"-"-  ',
    "raven":     "  //^\\\\  ",
    "eagle":     "  _/|\\_  ",
    "hawk":      "  _/|\\_  ",
    "bat":       "  \\/ \\/  ",
    "bear":      "  (oo)   ",
    "octopus":   " /|/|\\|\\ ",
    "dragon":    "  ~\\/\\/  ",
    "phoenix":   "  /|\\|\\  ",
    "shark":     "  ---\\/  ",
    "spider":    " /|/ \\|\\ ",
    "snake":     " \\_____/ ",
    "scorpion":  "  \\___/\\ ",
    "kraken":    " /|/|\\|\\ ",
}

# ── Eyes (expression, context-driven) ─────────────────────────
# Each is a 3-char string that replaces {E} in the body wrap

EYES: Dict[str, List[str]] = {
    "neutral":   ["o.o", "o_o", "·.·"],
    "happy":     ["^.^", "^_^", "^·^"],
    "thinking":  ["o.o", "·.·", "°.°"],
    "sleepy":    ["-.-", "-.~", "~.~"],
    "excited":   ["*.* ", "!.!", "°o°"],
    "worried":   ["o.O", "O.o", "°.O"],
    "proud":     ["^.~", "^.o", "~.^"],
    "searching": [">.>", ">.o", "o.<"],
    "dreamy":    ["~.~", "-.~", "~.-"],
    "alert":     ["O.O", "O_O", "0.0"],
    "mischief":  [">.^", "^.<", ">.~"],
    "love":      ["♥.♥", "❤.❤", "♡.♡"],
}

# ── Accessories (right side of body line, context-driven) ─────

ACCESSORIES: Dict[str, List[str]] = {
    "magnifying_glass": ["~Q", "oQ", "-Q"],
    "book":             ["[=]", "[#]", "[-]"],
    "shield":           ["|]", "[>", "|>"],
    "wrench":           ["}>", "d-", "|>"],
    "zzz":              ["z z", "Z z", "z Z"],
    "sparkle":          ["* *", "✦ ✦", "✧ ✧"],
    "heart":            ["<3", "♥", "♡"],
    "exclaim":          [" !", "!!", " !"],
    "question":         [" ?", "??", " ?"],
    "coffee":           ["c[_]", "c(_)", "[_]>"],
    "scroll":           ["~=", "~≡", "=~"],
    "flag":             ["|>", "|►", "|▸"],
    "none":             ["", "", ""],
}

# ── Floats (above head or after top line, mood/context accent) ─

FLOATS: Dict[str, List[str]] = {
    "thought":    ["...", " . ", "···"],
    "zzz":        ["z Z", "Z z", "zZz"],
    "sparkle":    ["✦", "✧", "*"],
    "exclaim":    ["!", "!!", "⚡"],
    "heart":      ["♥", "❤", "♡"],
    "music":      ["♪", "♫", "♬"],
    "sweat":      ["'", ";;", "\""],
    "none":       ["", "", ""],
}


# ═══════════════════════════════════════════════════════════════
#  CONTEXT RECIPES — what parts to use for each feature
# ═══════════════════════════════════════════════════════════════

RECIPES: Dict[str, dict] = {
    "search": {
        "eyes": "searching",
        "accessory": "magnifying_glass",
        "float": "thought",
    },
    "memory": {
        "eyes": "proud",
        "accessory": "book",
        "float": "sparkle",
    },
    "sancho": {
        "eyes": "sleepy",
        "accessory": "wrench",
        "float": "none",
    },
    "dream": {
        "eyes": "dreamy",
        "accessory": "none",
        "float": "zzz",
    },
    "celebrate": {
        "eyes": "excited",
        "accessory": "sparkle",
        "float": "music",
    },
    "error": {
        "eyes": "worried",
        "accessory": "exclaim",
        "float": "sweat",
    },
    "boot": {
        "eyes": "neutral",
        "accessory": "coffee",
        "float": "none",
    },
    "promote": {
        "eyes": "proud",
        "accessory": "flag",
        "float": "sparkle",
    },
    "consolidate": {
        "eyes": "thinking",
        "accessory": "scroll",
        "float": "thought",
    },
}

# All known species
ALL_SPECIES = list(HEADS.keys())


# ═══════════════════════════════════════════════════════════════
#  COMPOSITOR
# ═══════════════════════════════════════════════════════════════

def compose(species: str = "fox", context: str = "search") -> List[str]:
    """
    Assemble a 3-line ASCII character from parts based on species + context.

    Args:
        species: Animal species (fox, owl, cat, etc.)
        context: Feature context (search, memory, sancho, dream, celebrate, etc.)

    Returns:
        3 lines of composed ASCII art, normalized to uniform width.
    """
    # Fallback if unknown species
    if species not in HEADS:
        species = random.choice(ALL_SPECIES)

    recipe = RECIPES.get(context, RECIPES["search"])

    # Pick parts
    head = HEADS[species]
    body_template = BODIES.get(species, BODIES["fox"])
    feet = FEET.get(species, FEET["fox"])

    # Pick eyes (random from pool for variety)
    eye_pool = EYES.get(recipe["eyes"], EYES["neutral"])
    eyes = random.choice(eye_pool)

    # Assemble body with eyes
    body = body_template.replace("{E}", eyes)

    # Pick accessory
    acc_pool = ACCESSORIES.get(recipe["accessory"], ACCESSORIES["none"])
    accessory = random.choice(acc_pool)

    # Pick float
    float_pool = FLOATS.get(recipe["float"], FLOATS["none"])
    float_char = random.choice(float_pool)

    # ── Compose the 3 lines ──────────────────────────────────

    # Line 0: head + float (if any)
    if float_char:
        line0 = head + " " + float_char
    else:
        line0 = head

    # Line 1: body + accessory
    if accessory:
        line1 = body + accessory
    else:
        line1 = body

    # Line 2: feet (clean)
    line2 = feet

    # Normalize widths
    lines = [line0, line1, line2]
    max_w = max(len(l) for l in lines)
    lines = [l.ljust(max_w) for l in lines]

    return lines


def compose_random(context: str = "search") -> Tuple[List[str], str]:
    """
    Compose with a random species. Returns (art_lines, species_name).
    """
    species = random.choice(ALL_SPECIES)
    return compose(species, context), species


def compose_for_mascot(mascot_name: str, mascot_species: str, context: str) -> List[str]:
    """
    Compose for a known mascot (from nursery or buddy).

    Maps mascot species_name to closest base species, then composes.
    Nursery species are procedural names (Misttooth, Lunanub) — we
    extract the base species from the sprite if available, or pick random.
    """
    # Try direct species match
    species_lower = mascot_species.lower()
    for base in ALL_SPECIES:
        if base in species_lower:
            return compose(base, context)

    # Fallback: hash mascot name to pick a stable species
    species_idx = sum(ord(c) for c in mascot_name) % len(ALL_SPECIES)
    return compose(ALL_SPECIES[species_idx], context)
