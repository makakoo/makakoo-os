"""Terminal animation engine — spinners, typewriter effects, loading waves, etc."""

import sys
import time
from typing import List, Callable, Optional
from . import rendering_standards as rs
from .widgets import Spinner


class AnimationContext:
    """Tracks rendered line count for in-place cursor animation."""

    def __init__(self):
        self.line_count = 0

    def update(self, new_count: int):
        """Update line count and return cursor movement."""
        diff = new_count - self.line_count
        self.line_count = new_count
        return diff


class SpinnerAnimation:
    """Simple spinner animation."""

    def __init__(self, message: str = "Loading...", duration: float = 2.0, fps: int = 8):
        self.message = message
        self.duration = duration
        self.fps = fps
        self.steps = int(duration * fps)

    def animate(self) -> None:
        """Run spinner animation."""
        if not rs.SUPPORTS_ANIMATION:
            print(f"{self.message} [animated output not supported]")
            time.sleep(self.duration)
            return

        start_time = time.time()
        frame = 0

        while time.time() - start_time < self.duration:
            spinner = Spinner(frame, self.message)
            print(f"\r{spinner.render()}", end="", flush=True)
            frame += 1
            time.sleep(1.0 / self.fps)

        # Clear the line
        print("\r" + " " * 80, end="\r")


class TypewriterEffect:
    """Character-by-character text reveal."""

    def __init__(self, text: str, speed: float = 0.02):
        self.text = text
        self.speed = speed

    def animate(self) -> None:
        """Print text character by character."""
        if not rs.SUPPORTS_ANIMATION:
            print(self.text)
            return

        for char in self.text:
            print(char, end="", flush=True)
            time.sleep(self.speed)
        print()  # newline at end


class FadeIn:
    """Fade in text using ANSI dim/bold."""

    def __init__(self, text: str, duration: float = 1.0, fps: int = 10):
        self.text = text
        self.duration = duration
        self.fps = fps

    def animate(self) -> None:
        """Fade in text."""
        if not rs.SUPPORTS_ANIMATION:
            print(self.text)
            return

        steps = int(self.duration * self.fps)
        for step in range(steps):
            progress = step / max(steps - 1, 1)
            if progress < 0.5:
                output = f"{rs.COLORS.get('dim', '')}{self.text}{rs.COLORS.get('reset', '')}"
            else:
                output = f"{rs.COLORS.get('bold', '')}{self.text}{rs.COLORS.get('reset', '')}"
            print(f"\r{output}", end="", flush=True)
            time.sleep(1.0 / self.fps)
        print()


class ProgressAnimation:
    """Animated progress bar with status updates."""

    def __init__(self, total: int, message: str = "Progress"):
        self.total = total
        self.message = message
        self.current = 0

    def step(self, label: str = "", increment: int = 1):
        """Move progress forward."""
        self.current = min(self.current + increment, self.total)
        percent = self.current / max(self.total, 1)
        filled = int(30 * percent)
        bar = rs.PROGRESS_FULL * filled + rs.PROGRESS_EMPTY * (30 - filled)
        status_text = f"{self.message} [{bar}] {int(percent * 100)}%"
        if label:
            status_text += f" {label}"
        print(f"\r{status_text}", end="", flush=True)

    def done(self, message: str = "Complete"):
        """Mark animation as done."""
        print(f"\r{message}" + " " * 60)


class LoadingWave:
    """Animated wave effect for indeterminate loading."""

    def __init__(self, duration: float = 3.0, width: int = 20):
        self.duration = duration
        self.width = width

    def animate(self) -> None:
        """Show animated wave."""
        if not rs.SUPPORTS_ANIMATION:
            print("Processing...")
            time.sleep(self.duration)
            return

        chars = "≋≈∼ ~∼≈≋" if rs.SUPPORTS_UNICODE else "~-~-~-~-~"
        start_time = time.time()
        frame = 0

        while time.time() - start_time < self.duration:
            pos = frame % len(chars)
            wave = "".join([chars[pos] for _ in range(self.width)])
            print(f"\r{wave}", end="", flush=True)
            frame += 1
            time.sleep(0.1)

        print("\r" + " " * self.width)


class PulseAnimation:
    """Pulsing text effect."""

    def __init__(self, text: str, duration: float = 2.0, fps: int = 4):
        self.text = text
        self.duration = duration
        self.fps = fps

    def animate(self) -> None:
        """Pulse text."""
        if not rs.SUPPORTS_ANIMATION:
            print(self.text)
            time.sleep(self.duration)
            return

        steps = int(self.duration * self.fps)
        for step in range(steps):
            if step % 2 == 0:
                output = f"{rs.COLORS.get('bold', '')}{self.text}{rs.COLORS.get('reset', '')}"
            else:
                output = f"{rs.COLORS.get('dim', '')}{self.text}{rs.COLORS.get('reset', '')}"
            print(f"\r{output}", end="", flush=True)
            time.sleep(1.0 / self.fps)
        print()


class SequentialAnimation:
    """Run multiple animations in sequence."""

    def __init__(self, animations: List[Callable[[], None]]):
        self.animations = animations

    def animate(self) -> None:
        """Run all animations."""
        for anim in self.animations:
            anim()


class ConcurrentAnimation:
    """Run multiple animations concurrently (simple version — single-threaded)."""

    def __init__(self, animations: List[Callable[[], None]], duration: float = 2.0):
        self.animations = animations
        self.duration = duration

    def animate(self) -> None:
        """Run all animations (sequentially, for simplicity)."""
        for anim in self.animations:
            anim()


# ── Demo ────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    print("\n=== Harvey Terminal Animation Demo ===\n")

    print("Spinner Animation:")
    spinner = SpinnerAnimation("Processing", duration=2.0)
    spinner.animate()

    print("\nTypewriter Effect:")
    typewriter = TypewriterEffect("Hello from Harvey OS!", speed=0.05)
    typewriter.animate()

    print("\nProgress Animation:")
    progress = ProgressAnimation(10, "Installing")
    for i in range(11):
        progress.step(f"Step {i}", increment=1)
        time.sleep(0.2)
    progress.done("Installation complete!")

    print("\nLoading Wave:")
    wave = LoadingWave(duration=2.0)
    wave.animate()

    print("\nPulse Animation:")
    pulse = PulseAnimation("Watch me pulse!", duration=2.0)
    pulse.animate()
