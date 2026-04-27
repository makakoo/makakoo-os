#!/usr/bin/env python3
"""
Harvey Buddy — Deterministic ASCII companion with personality.

Inspired by claurst's Tamagotchi system. Each Harvey instance gets a unique
companion derived from the user's identity. Bones (species, rarity, stats)
are always recomputed from a deterministic hash — tamper-proof. Soul (name,
personality) is AI-generated and persisted.

Gives Harvey a face in the terminal.

Usage:
    from core.buddy.buddy import Buddy
    buddy = Buddy()
    print(buddy.greet())
    print(buddy.ascii_art())
    print(buddy.status_line())
"""

import hashlib
import json
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional

from core.buddy.accessories import EYE_NAMES, HAT_NAMES, apply_eyes, apply_hat
from core.buddy.interaction import greet_with_bubble, pet_animation

log = logging.getLogger("harvey.buddy")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
BUDDY_STATE = Path(HARVEY_HOME) / "data" / "buddy.json"


# ═══════════════════════════════════════════════════════════════
#  Mulberry32 PRNG — deterministic from seed
# ═══════════════════════════════════════════════════════════════

class Mulberry32:
    """Deterministic PRNG. Same seed = same sequence. Always."""

    def __init__(self, seed: int):
        self.state = seed & 0xFFFFFFFF

    def next_u32(self) -> int:
        self.state = (self.state + 0x6D2B79F5) & 0xFFFFFFFF
        t = (self.state ^ (self.state >> 15)) * (1 | self.state)
        t = (t + ((t ^ (t >> 7)) * (61 | t))) ^ t
        return (t ^ (t >> 14)) & 0xFFFFFFFF

    def next_float(self) -> float:
        return self.next_u32() / 0xFFFFFFFF

    def next_int(self, lo: int, hi: int) -> int:
        return lo + (self.next_u32() % (hi - lo + 1))


# ═══════════════════════════════════════════════════════════════
#  Species & Rarity
# ═══════════════════════════════════════════════════════════════

SPECIES = [
    ("fox", "🦊"), ("owl", "🦉"), ("cat", "🐱"), ("wolf", "🐺"),
    ("raven", "🐦‍⬛"), ("snake", "🐍"), ("bear", "🐻"), ("eagle", "🦅"),
    ("octopus", "🐙"), ("dragon", "🐉"), ("phoenix", "🔥"), ("shark", "🦈"),
    ("panther", "🐈‍⬛"), ("hawk", "🦅"), ("spider", "🕷️"), ("bat", "🦇"),
    ("scorpion", "🦂"), ("kraken", "🦑"),
]

RARITIES = [
    ("Common", 60), ("Uncommon", 25), ("Rare", 10), ("Epic", 4), ("Legendary", 1),
]

RARITY_COLORS = {
    "Common": "\033[37m",      # white
    "Uncommon": "\033[32m",    # green
    "Rare": "\033[34m",        # blue
    "Epic": "\033[35m",        # purple
    "Legendary": "\033[33m",   # gold
}

STATS = ["DEBUGGING", "PATIENCE", "CHAOS", "WISDOM", "SNARK"]

# ASCII art per species (3-line compact)
ASCII_ART = {
    "fox":     ["  /\\_/\\  ", " ( o.o ) ", "  > ^ <  "],
    "owl":     ["  {o,o}  ", "  |)__)  ", '  -"-"-  '],
    "cat":     ["  /\\_/\\  ", " ( ^.^ ) ", '  (")_(") '],
    "wolf":    ["  /\\___/\\ ", " ( o _ o )", "  U     U "],
    "raven":   ["  \\___/  ", "  (o>o)  ", "  //^\\\\  "],
    "snake":   ["  _____  ", " /~~~~~\\ ", " \\_____/ "],
    "bear":    [" (\")(\")", " (o  o) ", "  (oo)  "],
    "eagle":   ["  \\v v/  ", "  (o>o)  ", "  _/|\\_ "],
    "octopus": ["  ,--,   ", " (o  o)  ", " /|/|\\|\\ "],
    "dragon":  ["  /\\_/\\~ ", " ( @.@ ) ", "  ~\\/\\/  "],
    "phoenix": ["  \\|/|/  ", "  (O.O)  ", "  /|\\|\\  "],
    "shark":   ["  ___/\\  ", " (o   >) ", "  ---\\/  "],
    "panther": ["  /\\_/\\  ", " ( -.- ) ", "  ( u u) "],
    "hawk":    ["  \\\\v//  ", "  (O>O)  ", "  _/|\\_  "],
    "spider":  [" /\\ /\\  ", " \\(o.o)/ ", " /|/ \\|\\ "],
    "bat":     [" /\\\\_/\\\\ ", "  (o.o)  ", "  \\/ \\/  "],
    "scorpion":["  __/~   ", " (o  o)  ", "  \\___/\\ "],
    "kraken":  [" \\(o.o)/ ", "  /| |\\  ", " /|/|\\|\\ "],
}


# ═══════════════════════════════════════════════════════════════
#  Buddy
# ═══════════════════════════════════════════════════════════════

@dataclass
class BuddyBones:
    """Deterministic, tamper-proof attributes. Always recomputed."""
    species: str
    species_emoji: str
    rarity: str
    is_shiny: bool
    stats: Dict[str, int]
    seed: int
    eye_style: str = "default"
    hat: str = "none"


@dataclass
class BuddySoul:
    """AI-generated, persisted attributes."""
    name: str = ""
    personality: str = ""
    created_at: float = 0.0
    interactions: int = 0
    mood: str = "neutral"
    last_seen: float = 0.0


class Buddy:
    """Harvey's deterministic companion."""

    def __init__(self, user_id: str = None):
        self.user_id = user_id or self._get_user_id()
        self.bones = self._compute_bones()
        self.soul = self._load_soul()

    # ── Bones (always recomputed) ─────────────────────────────

    def _get_user_id(self) -> str:
        """Derive user identity from system."""
        import getpass
        hostname = os.uname().nodename
        user = getpass.getuser()
        home = os.path.expanduser("~")
        return f"{hostname}:{user}:{home}"

    def _compute_bones(self) -> BuddyBones:
        """Deterministic bones from user ID. Can never be tampered."""
        seed_str = self.user_id + "harvey-buddy-2026"
        seed_bytes = hashlib.sha256(seed_str.encode()).digest()
        seed = int.from_bytes(seed_bytes[:4], "little")
        rng = Mulberry32(seed)

        # Species
        species_idx = rng.next_int(0, len(SPECIES) - 1)
        species_name, species_emoji = SPECIES[species_idx]

        # Rarity (weighted)
        roll = rng.next_float() * 100
        cumulative = 0
        rarity = "Common"
        for r_name, r_weight in RARITIES:
            cumulative += r_weight
            if roll < cumulative:
                rarity = r_name
                break

        # Shiny (1% chance, independent)
        is_shiny = rng.next_float() < 0.01

        # Stats
        stats = {}
        peak = rng.next_int(0, len(STATS) - 1)
        dump = (peak + rng.next_int(1, len(STATS) - 1)) % len(STATS)
        rarity_bonus = {"Common": 0, "Uncommon": 5, "Rare": 10, "Epic": 20, "Legendary": 35}

        for i, stat in enumerate(STATS):
            base = rng.next_int(5, 18)
            if i == peak:
                base += 50
            elif i == dump:
                base = max(1, base - 8)
            base += rarity_bonus.get(rarity, 0)
            stats[stat] = min(99, base)

        # Eyes and hat (deterministic from rng)
        eye_idx = rng.next_int(0, len(EYE_NAMES) - 1)
        hat_idx = rng.next_int(0, len(HAT_NAMES) - 1)

        return BuddyBones(
            species=species_name,
            species_emoji=species_emoji,
            rarity=rarity,
            is_shiny=is_shiny,
            stats=stats,
            seed=seed,
            eye_style=EYE_NAMES[eye_idx],
            hat=HAT_NAMES[hat_idx],
        )

    # ── Soul (persisted) ──────────────────────────────────────

    def _load_soul(self) -> BuddySoul:
        """Load persisted soul or create default."""
        if BUDDY_STATE.exists():
            try:
                data = json.loads(BUDDY_STATE.read_text())
                soul = BuddySoul(**data.get("soul", {}))
                return soul
            except Exception:
                pass

        # Default soul — can be overwritten by LLM later
        soul = BuddySoul(
            name=self._default_name(),
            personality=self._default_personality(),
            created_at=time.time(),
        )
        self._save_soul(soul)
        return soul

    def _save_soul(self, soul: BuddySoul = None):
        """Persist soul to disk."""
        if soul is None:
            soul = self.soul
        BUDDY_STATE.parent.mkdir(parents=True, exist_ok=True)
        data = {
            "soul": {
                "name": soul.name,
                "personality": soul.personality,
                "created_at": soul.created_at,
                "interactions": soul.interactions,
                "mood": soul.mood,
                "last_seen": soul.last_seen,
            },
            # bones are NOT saved — always recomputed
        }
        BUDDY_STATE.write_text(json.dumps(data, indent=2))

    def _default_name(self) -> str:
        """Generate a default name from species + seed."""
        names = {
            "fox": "Kitsune", "owl": "Athena", "cat": "Miso",
            "wolf": "Fenrir", "raven": "Munin", "snake": "Ouroboros",
            "bear": "Kodiak", "eagle": "Aquila", "octopus": "Inky",
            "dragon": "Ember", "phoenix": "Astra", "shark": "Mako",
            "panther": "Shadow", "hawk": "Talon", "spider": "Arachne",
            "bat": "Echo", "scorpion": "Venom", "kraken": "Leviathan",
        }
        return names.get(self.bones.species, "Buddy")

    def _default_personality(self) -> str:
        """Default personality trait based on rarity."""
        traits = {
            "Common": "friendly and reliable",
            "Uncommon": "curious and adventurous",
            "Rare": "wise and thoughtful",
            "Epic": "fierce and determined",
            "Legendary": "ancient and all-knowing",
        }
        return traits.get(self.bones.rarity, "mysterious")

    # ── Interaction ───────────────────────────────────────────

    def interact(self):
        """Record an interaction."""
        self.soul.interactions += 1
        self.soul.last_seen = time.time()
        self._update_mood()
        self._save_soul()

    def _update_mood(self):
        """Update mood based on interaction frequency."""
        if self.soul.last_seen == 0:
            self.soul.mood = "excited"
            return

        hours_since = (time.time() - self.soul.last_seen) / 3600
        if hours_since < 1:
            self.soul.mood = "happy"
        elif hours_since < 8:
            self.soul.mood = "content"
        elif hours_since < 24:
            self.soul.mood = "neutral"
        elif hours_since < 72:
            self.soul.mood = "lonely"
        else:
            self.soul.mood = "forgotten"

    def pet(self) -> str:
        """Pet the buddy — plays heart animation, updates mood."""
        mood = pet_animation(self.soul.name)
        self.soul.interactions += 1
        self.soul.mood = mood
        self.soul.last_seen = time.time()
        self._save_soul()
        return f"{self.soul.name} feels {mood}!"

    def speak(self) -> str:
        """Get a mood-appropriate greeting in a speech bubble."""
        return greet_with_bubble(self)

    # ── Display ───────────────────────────────────────────────

    def ascii_art(self) -> str:
        """Get ASCII art for this buddy, with eyes and hat applied."""
        art = list(ASCII_ART.get(self.bones.species, ["  ???  ", " (?.?) ", "  ???  "]))
        art = apply_eyes(art, self.bones.eye_style)
        art = apply_hat(art, self.bones.hat)
        color = RARITY_COLORS.get(self.bones.rarity, "")
        reset = "\033[0m"
        shiny = " ✨" if self.bones.is_shiny else ""
        lines = [f"{color}{line}{reset}" for line in art]
        lines.append(f"{color}{self.soul.name}{shiny}{reset}")
        return "\n".join(lines)

    def greet(self) -> str:
        """Context-aware greeting."""
        hour = int(time.strftime("%H"))
        b = self.bones
        s = self.soul

        time_greeting = (
            "Good morning" if 5 <= hour < 12
            else "Good afternoon" if 12 <= hour < 17
            else "Good evening" if 17 <= hour < 22
            else "Late night session"
        )

        mood_flavor = {
            "happy": f"{s.name} is glad to see you again!",
            "excited": f"{s.name} just woke up for the first time!",
            "content": f"{s.name} is here and ready.",
            "neutral": f"{s.name} blinks at you expectantly.",
            "lonely": f"{s.name} perks up — it's been a while!",
            "forgotten": f"{s.name} stares at you... do I know you?",
        }

        color = RARITY_COLORS.get(b.rarity, "")
        reset = "\033[0m"

        return (
            f"{time_greeting}. {mood_flavor.get(s.mood, '')}\n"
            f"{color}[{b.rarity}{'✨' if b.is_shiny else ''} {b.species_emoji} "
            f"{s.name} the {b.species.title()}]{reset}"
        )

    def status_line(self) -> str:
        """Compact one-line status for terminal header."""
        b = self.bones
        s = self.soul
        shiny = "✨" if b.is_shiny else ""
        top_stat = max(b.stats, key=b.stats.get)
        return (
            f"{b.species_emoji}{shiny} {s.name} "
            f"[{b.rarity}] "
            f"{top_stat}:{b.stats[top_stat]} "
            f"mood:{s.mood}"
        )

    def stat_card(self) -> str:
        """Full stat card display with Harvey-specific stats."""
        b = self.bones
        s = self.soul
        color = RARITY_COLORS.get(b.rarity, "")
        reset = "\033[0m"

        lines = [
            f"{color}╔════════════════════════════════════╗{reset}",
            f"{color}║ {b.species_emoji} {s.name} the {b.species.title():<20} ║{reset}",
            f"{color}║ {b.rarity:>12}{'✨ Shiny' if b.is_shiny else '':>8}{'':>14}║{reset}",
            f"{color}╠════════════════════════════════════╣{reset}",
        ]
        for stat, val in b.stats.items():
            bar = "█" * (val // 5) + "░" * (20 - val // 5)
            lines.append(f"{color}║ {stat:<10}: {val:>2} {bar} ║{reset}")
        lines.append(f"{color}║ Eyes: {b.eye_style:<28} ║{reset}")
        lines.append(f"{color}║ Hat:  {b.hat:<28} ║{reset}")
        lines.append(f"{color}║ Personality: {s.personality:<21} ║{reset}")
        lines.append(f"{color}║ Mood: {s.mood:<28} ║{reset}")
        lines.append(f"{color}║ Sessions: {s.interactions:<24} ║{reset}")
        lines.append(f"{color}╚════════════════════════════════════╝{reset}")
        return "\n".join(lines)


# ═══════════════════════════════════════════════════════════════
#  CLI
# ═══════════════════════════════════════════════════════════════

if __name__ == "__main__":
    import sys

    buddy = Buddy()
    buddy.interact()

    cmd = sys.argv[1] if len(sys.argv) > 1 else "greet"

    if cmd == "greet":
        print(buddy.ascii_art())
        print()
        print(buddy.greet())
    elif cmd == "stats":
        print(buddy.stat_card())
    elif cmd == "status":
        print(buddy.status_line())
    elif cmd == "pet":
        print(buddy.ascii_art())
        print(buddy.pet())
    elif cmd == "speak":
        print(buddy.ascii_art())
        print()
        print(buddy.speak())
    else:
        print(f"Unknown: {cmd}. Try: greet, stats, status, pet, speak")
