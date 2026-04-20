"""Terminal renderer — safe output, capability detection, degrades gracefully."""

import sys
from . import rendering_standards as rs


class Renderer:
    """Safe terminal output with capability detection and graceful degradation."""

    def __init__(self):
        self.width = rs.TERM_WIDTH
        self.height = rs.TERM_HEIGHT
        self.supports_color = rs.SUPPORTS_COLOR
        self.supports_unicode = rs.SUPPORTS_UNICODE
        self.supports_animation = rs.SUPPORTS_ANIMATION
        self.color_tier = rs.COLOR_TIER
        self.colors = rs.COLORS
        self.box = rs.BOX
        self.symbols = rs.SYMBOLS

    def print(self, text, color=None, bold=False, dim=False, file=None):
        """Safe print with optional color and formatting."""
        if file is None:
            file = sys.stdout

        output = ""
        if bold:
            output += self.colors.get("bold", "")
        if dim:
            output += self.colors.get("dim", "")
        if color and color in self.colors:
            output += self.colors[color]

        output += text
        output += self.colors.get("reset", "")

        print(output, file=file)

    def box_draw(self, title="", content_lines=None, width=None):
        """Draw a box with optional title and content."""
        if width is None:
            width = self.width

        box = self.box["bold"]
        top = box["tl"] + box["h"] * (width - 2) + box["tr"]
        bottom = box["bl"] + box["h"] * (width - 2) + box["br"]

        lines = [top]

        if title:
            title_line = f" {title} "
            padding = width - len(title_line) - 2
            left_pad = padding // 2
            right_pad = padding - left_pad
            lines.append(box["v"] + " " * left_pad + title_line + " " * right_pad + box["v"])
            if content_lines:
                lines.append(box["ml"] + box["h"] * (width - 2) + box["mr"])

        if content_lines:
            for line in content_lines:
                # Truncate line if too long
                if len(line) > width - 4:
                    line = line[: width - 4] + "..."
                padding = width - len(line) - 4
                lines.append(box["v"] + " " + line + " " * padding + " " + box["v"])

        lines.append(bottom)
        return "\n".join(lines)

    def progress_bar(self, current, total, width=30, label=""):
        """Render a progress bar."""
        if total == 0:
            percent = 1.0
        else:
            percent = current / total

        filled = int(width * percent)
        bar = rs.PROGRESS_FULL * filled + rs.PROGRESS_EMPTY * (width - filled)

        # Add percentage and label
        pct_str = f"{int(percent * 100)}%"
        result = f"[{bar}] {pct_str}"

        if label:
            result += f" {label}"

        return result

    def spinner(self, step, message=""):
        """Get spinner frame for animation."""
        frame = rs.SPINNER_FRAMES[step % len(rs.SPINNER_FRAMES)]
        if message:
            return f"{frame} {message}"
        return frame

    def header(self, title, subtitle=""):
        """Render a header with ASCII art."""
        lines = []
        line_len = max(len(title), len(subtitle)) + 4

        # Top border
        lines.append(rs.BOX["bold"]["tl"] + rs.BOX["bold"]["h"] * line_len + rs.BOX["bold"]["tr"])

        # Title
        title_line = f"  {title}  "
        if len(title_line) < line_len + 2:
            padding = line_len + 2 - len(title_line)
            title_line += " " * padding
        lines.append(rs.BOX["bold"]["v"] + title_line + rs.BOX["bold"]["v"])

        # Subtitle if provided
        if subtitle:
            sub_line = f"  {subtitle}  "
            if len(sub_line) < line_len + 2:
                padding = line_len + 2 - len(sub_line)
                sub_line += " " * padding
            lines.append(rs.BOX["bold"]["v"] + sub_line + rs.BOX["bold"]["v"])

        # Bottom border
        lines.append(rs.BOX["bold"]["bl"] + rs.BOX["bold"]["h"] * line_len + rs.BOX["bold"]["br"])

        return "\n".join(lines)

    def status_badge(self, level):
        """Render a status badge [LEVEL] with color."""
        color = rs.STATUS_COLORS.get(level.lower(), self.colors.get("info", ""))
        badge = f"[{level.upper()}]"
        return f"{color}{badge}{self.colors.get('reset', '')}"

    def demo(self):
        """Print a demo of rendering capabilities."""
        print("\n=== Harvey Terminal Renderer Demo ===\n")

        print(f"Terminal Size: {self.width}x{self.height}")
        print(f"Color Tier: {self.color_tier}")
        print(f"Unicode Support: {self.supports_unicode}")
        print(f"Animation Support: {self.supports_animation}\n")

        print("=== Progress Bars ===")
        for pct in [0, 25, 50, 75, 100]:
            print(self.progress_bar(pct, 100, width=20, label=f"{pct}%"))

        print("\n=== Boxes ===")
        print(self.box_draw("Status", ["Item 1: Ready", "Item 2: Running", "Item 3: Failed"]))

        print("\n=== Status Badges ===")
        for level in ["low", "medium", "high", "forbidden"]:
            print(f"{level.capitalize()}: {self.status_badge(level)}")

        print("\n=== Colored Text ===")
        for color in ["green", "yellow", "red", "blue", "gray"]:
            self.print(f"This is {color}", color=color)

        print("\n=== Header ===")
        print(self.header("Harvey OS", "Universal Terminal Engine"))
