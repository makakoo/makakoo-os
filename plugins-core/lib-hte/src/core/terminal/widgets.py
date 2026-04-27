"""UI Widgets — reusable components for wizard UI, dashboards, and interactive tools."""

from dataclasses import dataclass
from typing import List, Dict, Optional, Union
from . import rendering_standards as rs
from .renderer import Renderer


class Widget:
    """Base widget class."""

    def __init__(self, width=None):
        self.width = width or rs.TERM_WIDTH
        self.renderer = Renderer()

    def render(self) -> str:
        """Return rendered string."""
        raise NotImplementedError


@dataclass
class Column:
    """Table column definition."""

    name: str
    width: int
    align: str = "left"  # left, center, right


class ProgressBar(Widget):
    """Simple progress bar widget."""

    def __init__(self, current: int, total: int, width: int = 30, label: str = "", parent_width=None):
        super().__init__(parent_width)
        self.current = current
        self.total = total
        self.bar_width = width
        self.label = label

    def render(self) -> str:
        return self.renderer.progress_bar(self.current, self.total, self.bar_width, self.label)


class Panel(Widget):
    """Boxed content panel with optional title."""

    def __init__(self, content: Union[str, List[str]], title: str = "", parent_width=None):
        super().__init__(parent_width)
        self.content = content if isinstance(content, list) else [content]
        self.title = title

    def render(self) -> str:
        return self.renderer.box_draw(self.title, self.content, self.width)


class StatusBadge(Widget):
    """Colored status badge [LEVEL]."""

    def __init__(self, level: str, parent_width=None):
        super().__init__(parent_width)
        self.level = level

    def render(self) -> str:
        return self.renderer.status_badge(self.level)


class Spinner(Widget):
    """Animated spinner frame."""

    def __init__(self, step: int, message: str = "", parent_width=None):
        super().__init__(parent_width)
        self.step = step
        self.message = message

    def render(self) -> str:
        return self.renderer.spinner(self.step, self.message)


class SpeechBubble(Widget):
    """ASCII speech bubble."""

    def __init__(self, text: str, parent_width=None):
        super().__init__(parent_width)
        self.text = text

    def render(self) -> str:
        lines = self.text.split("\n")
        max_len = max(len(line) for line in lines) if lines else 0

        # Build bubble
        result = []
        result.append(" " + rs.BOX["single"]["tl"] + rs.BOX["single"]["h"] * (max_len + 2) + rs.BOX["single"]["tr"])

        for line in lines:
            padding = max_len - len(line)
            result.append(f" {rs.BOX['single']['v']} {line}{' ' * padding} {rs.BOX['single']['v']}")

        result.append(" " + rs.BOX["single"]["bl"] + rs.BOX["single"]["h"] * (max_len + 2) + rs.BOX["single"]["br"])

        return "\n".join(result)


class Table(Widget):
    """Formatted table with borders."""

    def __init__(self, columns: List[Column], rows: List[Dict[str, str]], parent_width=None):
        super().__init__(parent_width)
        self.columns = columns
        self.rows = rows

    def render(self) -> str:
        lines = []

        # Header
        box = rs.BOX["bold"]
        header_parts = []
        for col in self.columns:
            header_parts.append(col.name.ljust(col.width))
        header_line = box["v"] + box["v"].join(header_parts) + box["v"]
        lines.append(header_line)

        # Separator
        sep_parts = []
        for col in self.columns:
            sep_parts.append(box["h"] * col.width)
        sep_line = box["ml"] + box["h"].join(sep_parts) + box["mr"]
        lines.append(sep_line)

        # Rows
        for row in self.rows:
            row_parts = []
            for col in self.columns:
                value = str(row.get(col.name, ""))
                if col.align == "center":
                    value = value.center(col.width)
                elif col.align == "right":
                    value = value.rjust(col.width)
                else:
                    value = value.ljust(col.width)
                row_parts.append(value)
            row_line = box["v"] + box["v"].join(row_parts) + box["v"]
            lines.append(row_line)

        # Bottom border
        bottom_parts = []
        for col in self.columns:
            bottom_parts.append(box["h"] * col.width)
        bottom_line = box["bl"] + box["h"].join(bottom_parts) + box["br"]
        lines.append(bottom_line)

        return "\n".join(lines)


class Header(Widget):
    """Header with ASCII art."""

    def __init__(self, title: str, subtitle: str = "", parent_width=None):
        super().__init__(parent_width)
        self.title = title
        self.subtitle = subtitle

    def render(self) -> str:
        return self.renderer.header(self.title, self.subtitle)


class TextBlock(Widget):
    """Simple text block with optional box."""

    def __init__(self, text: str, boxed: bool = False, title: str = "", parent_width=None):
        super().__init__(parent_width)
        self.text = text
        self.boxed = boxed
        self.title = title

    def render(self) -> str:
        if self.boxed:
            return Panel(self.text, self.title, self.width).render()
        return self.text


class Menu(Widget):
    """Menu with selectable items (for arrow-key navigation)."""

    def __init__(self, items: List[str], selected_index: int = 0, parent_width=None):
        super().__init__(parent_width)
        self.items = items
        self.selected_index = selected_index

    def render(self) -> str:
        """Render menu with current selection highlighted."""
        lines = []
        for i, item in enumerate(self.items):
            if i == self.selected_index:
                marker = "► "
                color = rs.COLORS.get("green", "")
                line = f"{color}{marker}{item}{rs.COLORS.get('reset', '')}"
            else:
                line = f"  {item}"
            lines.append(line)
        return "\n".join(lines)


class StatCard(Widget):
    """Card with stat labels and values."""

    def __init__(self, stats: Dict[str, str], title: str = "", parent_width=None):
        super().__init__(parent_width)
        self.stats = stats
        self.title = title

    def render(self) -> str:
        """Render stats in a box with two-column layout."""
        lines = []

        box = rs.BOX["bold"]
        title_section = ""
        if self.title:
            title_line = f" {self.title} "
            content_width = self.width - 4
            title_section = box["ml"] + box["h"] * content_width + box["mr"] + "\n"

        stat_lines = []
        for key, value in self.stats.items():
            stat_lines.append(f"  {key}: {value}")

        return self.renderer.box_draw(self.title, stat_lines, self.width)


class Alert(Widget):
    """Colored alert box."""

    def __init__(self, message: str, level: str = "info", parent_width=None):
        super().__init__(parent_width)
        self.message = message
        self.level = level  # info, warning, error, success

    def render(self) -> str:
        color = rs.STATUS_COLORS.get(self.level, rs.COLORS.get("info", ""))
        icon = {
            "info": rs.SYMBOLS.get("bullet", "*"),
            "warning": rs.SYMBOLS.get("warning_sign", "!"),
            "error": rs.SYMBOLS.get("cross", "X"),
            "success": rs.SYMBOLS.get("checkmark", "✓"),
        }.get(self.level, "*")

        badge = f"{color}[{self.level.upper()}]{rs.COLORS.get('reset', '')}"
        return f"{badge} {icon} {self.message}"


# ── Demo ────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    print("\n=== Harvey Terminal Widgets Demo ===\n")

    # Progress bars
    print("=== Progress Bars ===")
    for pct in [0, 25, 50, 75, 100]:
        pb = ProgressBar(pct, 100, 20, f"{pct}%")
        print(pb.render())

    # Panel
    print("\n=== Panel ===")
    panel = Panel(["Line 1", "Line 2", "Line 3"], "Status")
    print(panel.render())

    # Speech bubble
    print("\n=== Speech Bubble ===")
    bubble = SpeechBubble("Hello there!\nHow are you?")
    print(bubble.render())

    # Status badges
    print("\n=== Status Badges ===")
    for level in ["low", "medium", "high", "forbidden"]:
        badge = StatusBadge(level)
        print(badge.render())

    # Table
    print("\n=== Table ===")
    cols = [Column("Name", 15), Column("Status", 10), Column("Time", 12)]
    rows = [
        {"Name": "switchAILocal", "Status": "running", "Time": "12:34:56"},
        {"Name": "Qdrant", "Status": "running", "Time": "12:34:56"},
        {"Name": "PostgreSQL", "Status": "stopped", "Time": "N/A"},
    ]
    table = Table(cols, rows)
    print(table.render())

    # Alert
    print("\n=== Alerts ===")
    for level in ["info", "success", "warning", "error"]:
        alert = Alert(f"This is a {level} message", level)
        print(alert.render())

    # Header
    print("\n=== Header ===")
    header = Header("Harvey OS", "Universal Terminal Engine")
    print(header.render())
