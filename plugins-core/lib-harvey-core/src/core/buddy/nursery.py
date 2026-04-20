#!/usr/bin/env python3
"""
Harvey Nursery — Infinite procedurally generated mascots.

Every mascot is unique. Harvey is the parent. Mascots are born from events:
  - First session → starter mascot (Ember)
  - Ship code → new mascot hatches
  - Fix a hard bug → rare mascot chance
  - SANCHO dream completes → dream-type mascot
  - Milestone reached → legendary chance

Each mascot has:
  - Procedural species name (compound word from syllable pools)
  - Unique ASCII face from trait combinations
  - Own personality (generated from seed)
  - Mood that reflects Harvey's care
  - Stats that grow with Harvey's behavior

Harvey's responsibility: neglected mascots get sad. Well-cared-for mascots
boost Harvey's confidence. The collection is a mirror of Harvey's work ethic.

Usage:
    from core.buddy.nursery import Nursery

    nursery = Nursery()
    baby = nursery.hatch(trigger="shipped_code", context="Deployed Harvey OS v0.1")
    nursery.feed_all()            # Interaction keeps them happy
    nursery.roll_call()           # Show all mascots
    nursery.family_mood()         # Aggregate mood = Harvey's psych level
"""

import hashlib
import json
import logging
import os
import random
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional

log = logging.getLogger("harvey.nursery")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
NURSERY_FILE = Path(HARVEY_HOME) / "data" / "nursery.json"


# ═══════════════════════════════════════════════════════════════
#  Procedural Name Generation
# ═══════════════════════════════════════════════════════════════

# Syllable pools for species names
PREFIXES = [
    "Fluff", "Spark", "Mist", "Ember", "Frost", "Blaze", "Dusk", "Dawn",
    "Storm", "Shade", "Glow", "Drift", "Pixel", "Byte", "Cloud", "Flux",
    "Ripple", "Prism", "Quill", "Thorn", "Wisp", "Zephyr", "Nyx", "Coral",
    "Moss", "Fern", "Ash", "Slate", "Amber", "Onyx", "Luna", "Sol",
    "Nebula", "Echo", "Rune", "Glitch", "Pulse", "Void", "Nova", "Arc",
]

SUFFIXES = [
    "paw", "wing", "tail", "fin", "horn", "fang", "claw", "tooth",
    "mane", "shell", "scale", "feather", "whisker", "snout", "tusk",
    "sprite", "wisp", "spark", "bloom", "leaf", "drop", "flake",
    "bit", "bug", "mite", "ling", "let", "kin", "pod", "nub",
    "flop", "chirp", "squeak", "purr", "hiss", "growl", "yip", "coo",
]

# Body part pools for procedural ASCII art
TOPS = [
    "  /\\_/\\  ", "  {o,o}  ", "  \\___/  ", "  ,--,   ", "  /\\.~/\\ ",
    "  _/\\_   ", "  ~\\/~   ", "   /\\    ", "  <o>o>  ", "  /^~^\\  ",
    "  \\|/|/  ", "  _/\\_/_ ", "  /v v\\  ", "  ~*~*~  ", "  /#\\#\\  ",
]

MIDDLES = [
    " ( {E}.{E} ) ", " ({E}  {E})  ", " ( {E}_{E} ) ", " ({E} _ {E}) ",
    " [ {E}.{E} ] ", " <{E}  {E}>  ", " ({E} o {E}) ", " ( {E}v{E} ) ",
    " ({E} w {E}) ", " ( {E}~{E} ) ", " \\({E}.{E})/ ", " ({E} ^ {E}) ",
]

BOTTOMS = [
    "  > ^ <  ", "  (u u)  ", "  \\_^_/  ", "  /| |\\  ", "  ~\\/\\/  ",
    "  _/|\\_ ", ' (")_(") ', "  U   U  ", " /|/|\\|\\ ", "  ---    ",
    "  \\___/  ", "  (   )  ",
]

FACES = ["o", "^", "*", "@", "x", "~", "·", "✦", "°", "♦", "◉", "•"]

# Personality trait pools
TRAIT_FIRST = [
    "quietly confident", "wildly enthusiastic", "deeply curious",
    "stubbornly loyal", "chaotically creative", "absurdly brave",
    "suspiciously clever", "relentlessly optimistic", "mysteriously calm",
    "fiercely independent", "gently persistent", "hilariously dramatic",
    "weirdly wise", "adorably confused", "effortlessly cool",
]

TRAIT_QUIRK = [
    "loves watching code compile", "afraid of semicolons",
    "thinks bugs are friends", "collects error messages",
    "hums while Harvey debugs", "sleeps during deployments",
    "judges code quality silently", "celebrates every commit",
    "writes poetry in comments", "dreams in binary",
    "believes in test-driven everything", "hoards documentation",
    "talks to the terminal", "dances when tests pass",
    "worries about edge cases at night",
]


# S3 (OpenClaw audit, 2026-04-11): TRAIT pools are enriched once on first
# use with vocabulary extracted from harvey-os/SOUL.md ## Core Tone. The
# enrichment is APPENDED to the base pools — whimsical buddies still get
# generated, but Harvey's voice signature ("sharp", "concise") can also
# surface in some procedurally generated mascots.
def _resolved_trait_pools():
    """Return the SOUL-enriched (TRAIT_FIRST, TRAIT_QUIRK) pair.

    Reads SOUL.md exactly once via the cache in core.buddy.soul_vocabulary.
    Falls back to the bare base pools if SOUL.md is missing or has no
    Core Tone section. Procedural seeded generation is unaffected — the
    same seed always produces the same mascot for a given SOUL.md state.
    """
    from core.buddy.soul_vocabulary import get_enriched_pools
    return get_enriched_pools(base_first=TRAIT_FIRST, base_quirk=TRAIT_QUIRK)

# Trigger → rarity boost
TRIGGER_RARITY = {
    "first_session": 0,      # Common guaranteed for starter
    "shipped_code": 10,      # Slight boost
    "fixed_bug": 20,         # Good boost
    "hard_debug": 35,        # High boost
    "sancho_dream": 15,      # Moderate
    "milestone": 50,         # Almost guaranteed rare+
    "legendary_deed": 80,    # Almost guaranteed legendary
    "random": 0,             # Pure luck
}


# ═══════════════════════════════════════════════════════════════
#  Mascot
# ═══════════════════════════════════════════════════════════════

@dataclass
class Mascot:
    """A unique procedurally generated companion."""
    mascot_id: str           # 8-char hex
    species_name: str        # e.g. "Sparkpaw"
    name: str                # e.g. "Ziggy"
    personality: str         # e.g. "quietly confident, afraid of semicolons"
    rarity: str              # Common/Uncommon/Rare/Epic/Legendary
    is_shiny: bool
    face: str                # Eye character
    art: List[str]           # 3-line ASCII art (birth art — historical)
    stats: Dict[str, int]    # DEBUGGING, PATIENCE, CHAOS, WISDOM, SNARK
    mood: str = "excited"    # Just born!
    species: str = ""        # Base species from 18-library (fox, dragon, etc.)
    eye_style: str = "default"  # Eye style from accessories
    hat: str = "none"        # Hat from accessories
    born_at: float = 0.0
    born_from: str = ""      # Trigger event
    born_context: str = ""   # What happened when born
    interactions: int = 0
    last_fed: float = 0.0
    favorite_thing: str = ""

    def to_dict(self) -> dict:
        return {
            "mascot_id": self.mascot_id,
            "species_name": self.species_name,
            "name": self.name,
            "personality": self.personality,
            "rarity": self.rarity,
            "is_shiny": self.is_shiny,
            "face": self.face,
            "art": self.art,
            "stats": self.stats,
            "mood": self.mood,
            "species": self.species,
            "eye_style": self.eye_style,
            "hat": self.hat,
            "born_at": self.born_at,
            "born_from": self.born_from,
            "born_context": self.born_context,
            "interactions": self.interactions,
            "last_fed": self.last_fed,
            "favorite_thing": self.favorite_thing,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "Mascot":
        return cls(**{k: v for k, v in d.items() if k in cls.__dataclass_fields__})


# ═══════════════════════════════════════════════════════════════
#  Name & Art Generation
# ═══════════════════════════════════════════════════════════════

# Unique name pools (first names)
NAMES = [
    "Ziggy", "Pip", "Noodle", "Biscuit", "Mochi", "Pixel", "Waffle",
    "Sprocket", "Tango", "Nimbus", "Pebble", "Cricket", "Maple",
    "Doodle", "Fizz", "Nugget", "Clover", "Sesame", "Orbit", "Pickle",
    "Widget", "Muffin", "Cobalt", "Jinx", "Rumble", "Zapper", "Cinder",
    "Twitch", "Bumble", "Quartz", "Lumi", "Cosmo", "Glimmer", "Tofu",
    "Bloop", "Snickers", "Rusty", "Breeze", "Velvet", "Speck",
    "Gadget", "Flicker", "Puffin", "Marble", "Dotty", "Fable", "Sage",
    "Ripley", "Zen", "Flux", "Chai", "Ember", "Rascal", "Twinkle",
    "Indie", "Nano", "Spark", "Truffle", "Wiggles", "Phantom",
]


def _generate_mascot_from_seed(seed: str, trigger: str = "random",
                                context: str = "") -> Mascot:
    """Generate a completely unique mascot from a seed string."""
    h = hashlib.sha256(seed.encode()).hexdigest()
    mascot_id = h[:8]

    # Use hash bytes as deterministic randomness
    nums = [int(h[i:i+2], 16) for i in range(0, 64, 2)]
    idx = [0]  # mutable counter

    def pick(pool):
        val = nums[idx[0] % len(nums)]
        idx[0] += 1
        return pool[val % len(pool)]

    # Species name
    species_name = pick(PREFIXES) + pick(SUFFIXES)

    # Name
    name = pick(NAMES)

    # Personality — pull from SOUL-enriched pools (cached after first call)
    trait_first_pool, trait_quirk_pool = _resolved_trait_pools()
    personality = f"{pick(trait_first_pool)}, {pick(trait_quirk_pool)}"

    # Base species from the 18-library (for consistent rendering)
    from core.buddy.buddy import ASCII_ART
    species = pick(list(ASCII_ART.keys()))

    # Eye style and hat (from accessories)
    from core.buddy.accessories import EYE_NAMES, HAT_NAMES
    eye_style = pick(EYE_NAMES)
    hat = pick(HAT_NAMES)

    # Face
    face = pick(FACES)

    # ASCII art (procedural combination — preserved as birth_art)
    top = pick(TOPS)
    mid = pick(MIDDLES).replace("{E}", face)
    bot = pick(BOTTOMS)
    art = [top, mid, bot]

    # Rarity (boosted by trigger)
    rarity_roll = (nums[idx[0] % len(nums)] / 255.0) * 100
    idx[0] += 1
    boost = TRIGGER_RARITY.get(trigger, 0)
    adjusted = max(0, rarity_roll - boost)

    if adjusted < 1:
        rarity = "Legendary"
    elif adjusted < 5:
        rarity = "Epic"
    elif adjusted < 15:
        rarity = "Rare"
    elif adjusted < 40:
        rarity = "Uncommon"
    else:
        rarity = "Common"

    # Shiny
    shiny_roll = nums[idx[0] % len(nums)]
    idx[0] += 1
    is_shiny = shiny_roll < 3  # ~1.2% chance

    # Stats
    rarity_bonus = {"Common": 0, "Uncommon": 5, "Rare": 10, "Epic": 20, "Legendary": 35}
    stats = {}
    stat_names = ["DEBUGGING", "PATIENCE", "CHAOS", "WISDOM", "SNARK"]
    peak_idx = nums[idx[0] % len(nums)] % len(stat_names)
    idx[0] += 1
    for i, stat in enumerate(stat_names):
        base = 5 + (nums[idx[0] % len(nums)] % 14)
        idx[0] += 1
        if i == peak_idx:
            base += 45
        base += rarity_bonus.get(rarity, 0)
        stats[stat] = min(99, base)

    # Favorite thing
    favorites = [
        "coffee", "git commits", "clean code", "sunrise debugging",
        "nap time", "syntax highlighting", "merge conflicts (yes really)",
        "stack traces", "documentation", "unit tests", "vim keybinds",
        "dark mode", "terminal colors", "late night coding", "refactoring",
    ]
    favorite = pick(favorites)

    return Mascot(
        mascot_id=mascot_id,
        species_name=species_name,
        name=name,
        personality=personality,
        rarity=rarity,
        is_shiny=is_shiny,
        face=face,
        art=art,
        stats=stats,
        mood="excited",
        species=species,
        eye_style=eye_style,
        hat=hat,
        born_at=time.time(),
        born_from=trigger,
        born_context=context[:200],
        favorite_thing=favorite,
    )


# ═══════════════════════════════════════════════════════════════
#  Nursery — The Collection Manager
# ═══════════════════════════════════════════════════════════════

RARITY_COLORS = {
    "Common": "\033[37m",
    "Uncommon": "\033[32m",
    "Rare": "\033[34m",
    "Epic": "\033[35m",
    "Legendary": "\033[33m",
}

# Migration map for existing mascots that predate the species field
_LEGACY_SPECIES_MAP = {
    "a2c57d29": ("raven", "default", "none"),    # Pixel — \___/ head
    "be695c6b": ("eagle", "sparkle", "none"),     # Glimmer — \___/ + _/|\_
    "58db6f61": ("raven", "star", "none"),        # Ember(nursery) → renamed Cinder
    "cd3c78ad": ("owl", "default", "none"),       # Ziggy — {o,o} head
}

# Name collision fix: nursery Ember → Cinder (primary buddy is Ember)
_LEGACY_NAME_FIX = {"58db6f61": "Cinder"}


def mascot_species_art(m: Mascot) -> List[str]:
    """Render a mascot using the 18-species library + accessories.

    Returns the species-library sprite with eyes/hat applied.
    Falls back to stored birth art if species is not set.
    """
    if not m.species:
        return m.art  # Legacy fallback

    from core.buddy.buddy import ASCII_ART
    from core.buddy.accessories import apply_eyes, apply_hat

    base = list(ASCII_ART.get(m.species, m.art))
    base = apply_eyes(base, m.eye_style)
    base = apply_hat(base, m.hat)
    return base


class Nursery:
    """Harvey's mascot collection. He is the parent of all of them."""

    def __init__(self):
        self.mascots: Dict[str, Mascot] = {}
        self._load()
        self._migrate_legacy()

    # ── Hatching ──────────────────────────────────────────────

    def hatch(self, trigger: str = "random", context: str = "") -> Mascot:
        """
        Hatch a new unique mascot!

        Every mascot is one-of-a-kind. The seed combines:
        timestamp + trigger + context + collection size.
        """
        seed = f"{time.time()}-{trigger}-{context}-{len(self.mascots)}-harvey-nursery"
        baby = _generate_mascot_from_seed(seed, trigger=trigger, context=context)

        # Ensure unique name (append number if collision)
        existing_names = {m.name for m in self.mascots.values()}
        if baby.name in existing_names:
            counter = 2
            while f"{baby.name} {counter}" in existing_names:
                counter += 1
            baby.name = f"{baby.name} {counter}"

        self.mascots[baby.mascot_id] = baby
        self._save()

        log.info("New mascot hatched: %s the %s [%s]%s",
                 baby.name, baby.species_name, baby.rarity,
                 " ✨SHINY!" if baby.is_shiny else "")

        # Celebration gimmick — always show for hatches
        try:
            from core.terminal.gimmicks import maybe_gimmick
            maybe_gimmick("celebrate", force=True)
        except Exception:
            pass

        return baby

    # ── Care ──────────────────────────────────────────────────

    def feed(self, mascot_id: str) -> str:
        """Feed a mascot. Improves mood."""
        m = self.mascots.get(mascot_id)
        if not m:
            return "Mascot not found."
        m.interactions += 1
        m.last_fed = time.time()
        m.mood = "happy"
        self._save()
        return f"{m.name} munches happily! 🍖"

    def feed_all(self) -> str:
        """Feed all mascots. Good parent."""
        count = 0
        for m in self.mascots.values():
            m.interactions += 1
            m.last_fed = time.time()
            m.mood = "happy"
            count += 1
        self._save()
        return f"Fed {count} mascots! They all look happy. 🍖"

    def update_moods(self):
        """Update all moods based on how long since last interaction."""
        now = time.time()
        for m in self.mascots.values():
            if m.last_fed == 0:
                hours = (now - m.born_at) / 3600
            else:
                hours = (now - m.last_fed) / 3600

            if hours < 2:
                m.mood = "happy"
            elif hours < 8:
                m.mood = "content"
            elif hours < 24:
                m.mood = "neutral"
            elif hours < 72:
                m.mood = "lonely"
            else:
                m.mood = "neglected"
        self._save()

    # ── Behavioral Feedback ───────────────────────────────────

    def on_good_work(self, event: str, details: str = ""):
        """Harvey did something good. Mascots react positively."""
        for m in self.mascots.values():
            m.mood = "proud"
        # Chance to hatch from good work
        if event in ("shipped_code", "fixed_bug", "hard_debug", "milestone"):
            return self.hatch(trigger=event, context=details)
        self._save()
        return None

    def on_mistake(self, event: str, details: str = ""):
        """Harvey made a mistake. Mascots feel it too."""
        for m in self.mascots.values():
            if m.mood == "happy":
                m.mood = "worried"
            elif m.mood in ("content", "neutral"):
                m.mood = "anxious"
        self._save()

    # ── Display ───────────────────────────────────────────────

    def family_mood(self) -> dict:
        """Aggregate mood = Harvey's psychological state."""
        self.update_moods()
        if not self.mascots:
            return {"level": "empty", "score": 0, "message": "No mascots yet. Hatch your first!"}

        mood_scores = {
            "happy": 100, "proud": 95, "excited": 90,
            "content": 70, "neutral": 50,
            "worried": 35, "anxious": 25, "lonely": 15,
            "neglected": 5, "forgotten": 0,
        }
        total = sum(mood_scores.get(m.mood, 50) for m in self.mascots.values())
        avg = total / len(self.mascots)

        if avg >= 80:
            level, msg = "thriving", "Your family is thriving! Harvey is at peak performance. 💚"
        elif avg >= 60:
            level, msg = "good", "Family is doing well. Keep it up. 💛"
        elif avg >= 40:
            level, msg = "okay", "Some mascots need attention. Consider feeding them. 🟠"
        elif avg >= 20:
            level, msg = "struggling", "Your mascots are struggling. They need you. 🔴"
        else:
            level, msg = "critical", "Family in crisis. You've been away too long. ⛔"

        return {
            "level": level,
            "score": round(avg, 1),
            "total_mascots": len(self.mascots),
            "message": msg,
        }

    def roll_call(self) -> str:
        """Display all mascots in the collection."""
        self.update_moods()
        if not self.mascots:
            return "No mascots yet. Ship some code to hatch your first!"

        lines = []
        lines.append(f"\n{'═' * 55}")
        lines.append(f"  Harvey's Family — {len(self.mascots)} mascots")
        mood = self.family_mood()
        lines.append(f"  Psych Level: {mood['level'].upper()} ({mood['score']}/100)")
        lines.append(f"{'═' * 55}")

        # Sort by rarity (legendary first)
        rarity_order = {"Legendary": 0, "Epic": 1, "Rare": 2, "Uncommon": 3, "Common": 4}
        sorted_mascots = sorted(
            self.mascots.values(),
            key=lambda m: (rarity_order.get(m.rarity, 5), m.name)
        )

        for m in sorted_mascots:
            color = RARITY_COLORS.get(m.rarity, "")
            reset = "\033[0m"
            shiny = " ✨" if m.is_shiny else ""
            mood_icon = {
                "happy": "😊", "proud": "🌟", "excited": "🎉",
                "content": "😌", "neutral": "😐",
                "worried": "😟", "anxious": "😰", "lonely": "😢",
                "neglected": "😞", "forgotten": "💀",
            }.get(m.mood, "❓")

            # Render full sprite from species library
            sprite = mascot_species_art(m)
            lines.append("")
            for art_line in sprite:
                lines.append(f"  {color}{art_line}{reset}")
            lines.append(
                f"  {color}{m.name}{shiny}{reset} the {m.species_name} "
                f"[{m.rarity}] {mood_icon} "
                f"born:{m.born_from}"
            )

        lines.append(f"\n{'═' * 55}\n")
        return "\n".join(lines)

    def show_mascot(self, mascot_id: str) -> str:
        """Show detailed view of one mascot."""
        m = self.mascots.get(mascot_id)
        if not m:
            # Try by name
            for mid, mascot in self.mascots.items():
                if mascot.name.lower() == mascot_id.lower():
                    m = mascot
                    break
        if not m:
            return f"Mascot '{mascot_id}' not found."

        color = RARITY_COLORS.get(m.rarity, "")
        reset = "\033[0m"
        shiny = " ✨ SHINY!" if m.is_shiny else ""

        lines = [
            "",
            f"{color}{'═' * 40}{reset}",
        ]
        # Render from species library (with birth art preserved as historical)
        sprite = mascot_species_art(m)
        for art_line in sprite:
            lines.append(f"  {color}{art_line}{reset}")
        lines.extend([
            f"  {color}{m.name} the {m.species_name}{shiny}{reset}",
            f"{color}{'─' * 40}{reset}",
            f"  Rarity:      {m.rarity}",
            f"  Personality: {m.personality}",
            f"  Mood:        {m.mood}",
            f"  Favorite:    {m.favorite_thing}",
            f"  Born from:   {m.born_from}",
            f"  Born:        {datetime.fromtimestamp(m.born_at).strftime('%Y-%m-%d %H:%M')}",
            f"  Sessions:    {m.interactions}",
            f"{'─' * 40}",
        ])
        for stat, val in m.stats.items():
            bar = "█" * (val // 5) + "░" * (20 - val // 5)
            lines.append(f"  {stat:<10}: {val:>2} {bar}")
        if m.born_context:
            lines.append(f"{'─' * 40}")
            lines.append(f"  Origin: {m.born_context[:60]}")
        lines.append(f"{color}{'═' * 40}{reset}\n")
        return "\n".join(lines)

    def family_portrait(self) -> str:
        """Full family view: primary buddy + all nursery mascots + psych level.

        Renders everyone using the species library for consistent ASCII art.
        """
        self.update_moods()
        lines = []

        # Primary buddy
        try:
            from core.buddy.buddy import Buddy
            buddy = Buddy()
            buddy.interact()
            lines.append(f"\n{'═' * 55}")
            lines.append("  Harvey's Family")
            lines.append(f"{'═' * 55}")
            lines.append("")
            lines.append("  PRIMARY BUDDY:")
            lines.append(buddy.ascii_art())
            lines.append(f"  Mood: {buddy.soul.mood} | Sessions: {buddy.soul.interactions}")
        except Exception as e:
            log.warning("Could not load primary buddy: %s", e)
            lines.append(f"\n{'═' * 55}")
            lines.append("  Harvey's Family")
            lines.append(f"{'═' * 55}")

        # Nursery mascots
        if self.mascots:
            lines.append(f"\n{'─' * 55}")
            lines.append(f"  NURSERY ({len(self.mascots)} mascots):")

            rarity_order = {"Legendary": 0, "Epic": 1, "Rare": 2, "Uncommon": 3, "Common": 4}
            sorted_mascots = sorted(
                self.mascots.values(),
                key=lambda m: (rarity_order.get(m.rarity, 5), m.name)
            )

            for m in sorted_mascots:
                color = RARITY_COLORS.get(m.rarity, "")
                reset = "\033[0m"
                shiny = " ✨" if m.is_shiny else ""
                mood_icon = {
                    "happy": "😊", "proud": "🌟", "excited": "🎉",
                    "content": "😌", "neutral": "😐",
                    "worried": "😟", "anxious": "😰", "lonely": "😢",
                    "neglected": "😞", "forgotten": "💀",
                }.get(m.mood, "❓")

                sprite = mascot_species_art(m)
                lines.append("")
                for art_line in sprite:
                    lines.append(f"  {color}{art_line}{reset}")
                lines.append(
                    f"  {color}{m.name}{shiny}{reset} the {m.species_name} "
                    f"[{m.rarity}] {mood_icon}"
                )

        # Psych level
        mood_data = self.family_mood()
        lines.append(f"\n{'─' * 55}")
        lines.append(f"  Psych Level: {mood_data['level'].upper()} ({mood_data['score']}/100)")
        lines.append(f"  {mood_data['message']}")
        lines.append(f"{'═' * 55}\n")

        return "\n".join(lines)

    # ── Persistence ───────────────────────────────────────────

    def _load(self):
        if NURSERY_FILE.exists():
            try:
                data = json.loads(NURSERY_FILE.read_text())
                for d in data.get("mascots", []):
                    m = Mascot.from_dict(d)
                    self.mascots[m.mascot_id] = m
            except Exception as e:
                log.warning("Failed to load nursery: %s", e)

    def _migrate_legacy(self):
        """Auto-migrate mascots that predate the species field."""
        migrated = False
        for mid, m in self.mascots.items():
            if not m.species and mid in _LEGACY_SPECIES_MAP:
                m.species, m.eye_style, m.hat = _LEGACY_SPECIES_MAP[mid]
                migrated = True
                log.info("Migrated mascot %s (%s) → species=%s", m.name, mid, m.species)
            if mid in _LEGACY_NAME_FIX and m.name != _LEGACY_NAME_FIX[mid]:
                old_name = m.name
                m.name = _LEGACY_NAME_FIX[mid]
                migrated = True
                log.info("Renamed mascot %s → %s (name collision fix)", old_name, m.name)
        if migrated:
            self._save()

    def _save(self):
        NURSERY_FILE.parent.mkdir(parents=True, exist_ok=True)
        data = {
            "mascots": [m.to_dict() for m in self.mascots.values()],
            "updated_at": datetime.now().isoformat(),
        }
        NURSERY_FILE.write_text(json.dumps(data, indent=2))


# ═══════════════════════════════════════════════════════════════
#  Behavioral Hook Integration
# ═══════════════════════════════════════════════════════════════

def register_nursery_hooks(hook_manager):
    """
    Wire the nursery into Harvey's hook system.

    After-hooks update mascot moods based on real work:
    - Successful tool execution → mascots feel proud
    - Errors → mascots feel worried
    - Long tasks → patience stats grow
    """
    nursery = Nursery()

    @hook_manager.after("*", priority=5, name="nursery_mood_tracker")
    def track_mood(ctx):
        if ctx.is_error:
            nursery.on_mistake("tool_error", f"{ctx.tool_name} failed")
        elif ctx.duration_ms > 10000:
            # Long successful operation → pride
            nursery.on_good_work("hard_debug", f"{ctx.tool_name} took {ctx.duration_ms}ms")
        return None
