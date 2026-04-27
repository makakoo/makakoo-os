"""
Animated sprite frames for Harvey Buddy species.

Each species has 3 frames for idle animation — subtle variations like
ear twitches, eye blinks, tail wags. SpriteAnimator handles timing.
"""

import time
from typing import List

SPRITE_FRAMES = {
    "fox": [
        ["  /\\_/\\  ", " ( o.o ) ", "  > ^ <  "],
        ["  /\\_/\\  ", " ( -.o ) ", "  > ^ <  "],
        ["  /\\_/\\  ", " ( o.o ) ", "  > ~ <  "],
    ],
    "owl": [
        ["  {o,o}  ", "  |)__)  ", '  -"-"-  '],
        ["  {-,-}  ", "  |)__)  ", '  -"-"-  '],
        ["  {o,o}  ", "  |)_)   ", '  -"-"-  '],
    ],
    "cat": [
        ["  /\\_/\\  ", " ( ^.^ ) ", '  (")_(") '],
        ["  /\\_/\\  ", " ( -.^ ) ", '  (")_(") '],
        ["  /\\_/\\  ", " ( ^.^ ) ", '  (")~(") '],
    ],
    "wolf": [
        ["  /\\___/\\ ", " ( o _ o )", "  U     U "],
        ["  /\\___/\\ ", " ( o _ o )", "  U     U "],
        ["  /\\___/\\ ", " ( - _ o )", "  U     U "],
    ],
    "raven": [
        ["  \\___/  ", "  (o>o)  ", "  //^\\\\  "],
        ["  \\___/  ", "  (->o)  ", "  //^\\\\  "],
        ["  \\__/   ", "  (o>o)  ", "  //^\\\\  "],
    ],
    "snake": [
        ["  _____  ", " /~~~~~\\ ", " \\_____/ "],
        ["  _____  ", " /~~~~_\\ ", " \\_____/ "],
        ["  _____  ", " /~~~~~\\ ", " \\___~_/ "],
    ],
    "bear": [
        [' (")(\")', " (o  o) ", "  (oo)  "],
        [' (")(\")', " (-  o) ", "  (oo)  "],
        [' (")(\")', " (o  o) ", "  (OO)  "],
    ],
    "eagle": [
        ["  \\v v/  ", "  (o>o)  ", "  _/|\\_ "],
        ["  \\v v/  ", "  (->o)  ", "  _/|\\_ "],
        ["  \\v_v/  ", "  (o>o)  ", "  _/|\\_ "],
    ],
    "octopus": [
        ["  ,--,   ", " (o  o)  ", " /|/|\\|\\ "],
        ["  ,--,   ", " (-  o)  ", " /|/|\\|\\ "],
        ["  ,--,   ", " (o  o)  ", " \\|/|/|\\ "],
    ],
    "dragon": [
        ["  /\\_/\\~ ", " ( @.@ ) ", "  ~\\/\\/  "],
        ["  /\\_/\\~ ", " ( @.@ ) ", "  ~/\\/\\  "],
        ["  /\\_/\\~+", " ( @.@ ) ", "  ~\\/\\/  "],
    ],
    "phoenix": [
        ["  \\|/|/  ", "  (O.O)  ", "  /|\\|\\  "],
        ["  /|\\|\\  ", "  (O.O)  ", "  \\|/|/  "],
        ["  \\|/|/  ", "  (-.O)  ", "  /|\\|\\  "],
    ],
    "shark": [
        ["  ___/\\  ", " (o   >) ", "  ---\\/  "],
        ["  ___/\\  ", " (-   >) ", "  ---\\/  "],
        ["  ___/\\  ", " (o   >) ", "  --~\\/  "],
    ],
    "panther": [
        ["  /\\_/\\  ", " ( -.- ) ", "  ( u u) "],
        ["  /\\_/\\  ", " ( _._ ) ", "  ( u u) "],
        ["  /\\_/\\  ", " ( -.- ) ", "  ( u_u) "],
    ],
    "hawk": [
        ["  \\\\v//  ", "  (O>O)  ", "  _/|\\_  "],
        ["  \\\\v//  ", "  (->O)  ", "  _/|\\_  "],
        ["  \\\\_//  ", "  (O>O)  ", "  _/|\\_  "],
    ],
    "spider": [
        [" /\\ /\\  ", " \\(o.o)/ ", " /|/ \\|\\ "],
        [" /\\ /\\  ", " \\(-.o)/ ", " /|/ \\|\\ "],
        [" /\\ /\\  ", " \\(o.o)/ ", " \\|/ /|\\ "],
    ],
    "bat": [
        [" /\\\\_/\\\\ ", "  (o.o)  ", "  \\/ \\/  "],
        [" /\\\\_/\\\\ ", "  (-.-)  ", "  \\/ \\/  "],
        [" /\\\\ /\\\\ ", "  (o.o)  ", "  \\/ \\/  "],
    ],
    "scorpion": [
        ["  __/~   ", " (o  o)  ", "  \\___/\\ "],
        ["  __/~   ", " (-  o)  ", "  \\___/\\ "],
        ["  __/~~  ", " (o  o)  ", "  \\___/\\ "],
    ],
    "kraken": [
        [" \\(o.o)/ ", "  /| |\\  ", " /|/|\\|\\ "],
        [" \\(-.o)/ ", "  /| |\\  ", " /|/|\\|\\ "],
        [" \\(o.o)/ ", "  /| |\\  ", " \\|/|/|\\ "],
    ],
}

# Animation order — mostly frame 0 with occasional blink (1) and idle motion (2)
IDLE_SEQUENCE = [0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]


class SpriteAnimator:
    """Handles frame timing for sprite animation."""

    def __init__(self, species: str):
        self.species = species
        self.frames = SPRITE_FRAMES.get(species, SPRITE_FRAMES["fox"])
        self._start_time = time.time()

    def current_frame(self) -> List[str]:
        """Return the current frame based on elapsed time (500ms ticks)."""
        elapsed = time.time() - self._start_time
        tick = int(elapsed / 0.5) % len(IDLE_SEQUENCE)
        frame_idx = IDLE_SEQUENCE[tick]
        return self.frames[frame_idx]

    def animate(self, duration: float = 3.0):
        """Print animation loop to terminal for the given duration."""
        end_time = time.time() + duration
        last_tick = -1

        while time.time() < end_time:
            elapsed = time.time() - self._start_time
            tick = int(elapsed / 0.5) % len(IDLE_SEQUENCE)

            if tick != last_tick:
                frame = self.frames[IDLE_SEQUENCE[tick]]
                # Move cursor up 3 lines and redraw (skip on first frame)
                if last_tick != -1:
                    print(f"\033[3A", end="")
                for line in frame:
                    print(line)
                last_tick = tick

            time.sleep(0.05)  # poll rate — not the tick rate
