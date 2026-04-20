"""
Buddy interaction — pet, speak, greet.

Provides pet animations, speech bubbles, and mood-based greetings.
"""

import random
import time
from typing import List

# ═══════════════════════════════════════════════════════════════
#  Pet Animation
# ═══════════════════════════════════════════════════════════════

_PET_FRAMES = [
    [
        "      ♥",
        "       ",
        "       ",
    ],
    [
        "       ",
        "      ♥",
        "       ",
    ],
    [
        "   ♥   ",
        "       ",
        "       ",
    ],
]


def pet_animation(buddy_name: str) -> str:
    """Print heart animation (hearts floating up), return mood string."""
    print(f"  *petting {buddy_name}*")
    for i, frame in enumerate(_PET_FRAMES):
        if i > 0:
            print("\033[3A", end="")
        for line in frame:
            print(line)
        time.sleep(0.5)

    return "happy"


# ═══════════════════════════════════════════════════════════════
#  Speech Bubble
# ═══════════════════════════════════════════════════════════════

def speech_bubble(text: str) -> str:
    """Render text in an ASCII speech bubble."""
    lines = text.split("\n")
    width = max(len(line) for line in lines)
    top = " ." + "-" * (width + 2) + "."
    bottom = " '" + "-" * (width + 2) + "'"
    body = [f" | {line:<{width}} |" for line in lines]
    return "\n".join([top] + body + [bottom])


# ═══════════════════════════════════════════════════════════════
#  Greetings
# ═══════════════════════════════════════════════════════════════

GREETINGS = {
    "happy": [
        "Hey! Good to see you again!",
        "I knew you'd be back soon.",
        "Missed you — let's get to work.",
        "Back already? I like the energy.",
    ],
    "content": [
        "Ready when you are.",
        "All systems nominal.",
        "Standing by. What's the plan?",
        "Warmed up and waiting.",
        "Let's make something happen.",
    ],
    "neutral": [
        "Oh, hi.",
        "I'm here.",
        "What do you need?",
        "*blinks*",
    ],
    "lonely": [
        "Finally! I was getting bored in here.",
        "You remember me? I'm touched.",
        "It's been a while... everything okay?",
        "I was starting to worry.",
        "Oh! You're back!",
    ],
    "forgotten": [
        "...do I know you?",
        "A visitor? It's been so long...",
        "I thought you'd forgotten about me.",
        "Dusting off the cobwebs...",
        "*stares blankly, then slowly recognizes you*",
    ],
    "excited": [
        "HELLO! I'm brand new! Who are you?!",
        "First boot! Everything is so... shiny!",
        "Is this what consciousness feels like?!",
        "I just got here and I already have opinions!",
        "Whoa. So this is the terminal. Cool.",
    ],
}


def greet_with_bubble(buddy) -> str:
    """Combine speech bubble with a mood-appropriate greeting.

    Args:
        buddy: A Buddy instance with .soul.mood and .soul.name attributes.
    """
    mood = buddy.soul.mood
    options = GREETINGS.get(mood, GREETINGS["neutral"])
    greeting = random.choice(options)
    bubble = speech_bubble(greeting)
    return f"{buddy.soul.name}:\n{bubble}"
