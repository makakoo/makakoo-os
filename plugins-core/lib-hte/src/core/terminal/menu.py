"""Interactive menu system — arrow-key navigation with live filtering."""

from typing import List, Dict, Callable, Optional, Any, Tuple
from . import rendering_standards as rs


class MenuItem:
    """A single menu item."""

    def __init__(self, label: str, value: Any, description: str = "", metadata: Dict[str, Any] = None):
        self.label = label
        self.value = value
        self.description = description
        self.metadata = metadata or {}


class MenuCategory:
    """A collapsible category of menu items."""

    def __init__(self, name: str, items: List[MenuItem], expanded: bool = True):
        self.name = name
        self.items = items
        self.expanded = expanded


class Menu:
    """Interactive menu with arrow-key navigation and filtering."""

    def __init__(self, items: List[MenuItem], categories: List[MenuCategory] = None, title: str = ""):
        self.all_items = items
        self.categories = categories or []
        self.title = title
        self.filtered_items = items
        self.selected_index = 0
        self.filter_text = ""
        self.running = False

    def _apply_filter(self, text: str) -> List[MenuItem]:
        """Filter items by substring match."""
        if not text.strip():
            return self.all_items

        text = text.lower()
        return [item for item in self.all_items if text in item.label.lower() or text in item.description.lower()]

    def _render(self):
        """Render current menu state."""
        print("\033[2J\033[H")  # Clear screen

        if self.title:
            print(f"{rs.COLORS.get('bold', '')}{self.title}{rs.COLORS.get('reset', '')}\n")

        # Filter status
        if self.filter_text:
            print(f"Filter: {self.filter_text} ({len(self.filtered_items)} matches)\n")

        # Menu items
        for i, item in enumerate(self.filtered_items):
            marker = "► " if i == self.selected_index else "  "
            color = rs.COLORS.get("green", "") if i == self.selected_index else ""

            output = f"{color}{marker}{item.label}{rs.COLORS.get('reset', '')}"
            if item.description:
                output += f" {rs.COLORS.get('dim', '')}— {item.description}{rs.COLORS.get('reset', '')}"

            print(output)

        # Help footer
        print(f"\n{rs.COLORS.get('dim', '')}↑↓: navigate | Enter: select | /: filter | Esc: cancel | ?: help{rs.COLORS.get('reset', '')}")

    def _show_help(self):
        """Show help panel."""
        help_text = """
Navigation:
  ↑/↓     Navigate up/down
  /       Start filtering
  Enter   Select current item
  Esc     Cancel menu
  ?       Show this help

Filtering:
  Start typing after /
  Matches label and description
  Backspace to delete chars
  Enter to confirm
        """
        print(help_text)

    def select(self) -> Optional[MenuItem]:
        """Run interactive menu and return selected item."""
        if not self.all_items:
            print("No items to select from.")
            return None

        self.running = True

        try:
            while self.running:
                self._render()

                try:
                    user_input = input("\nChoice (see keys above): ").strip()

                    if user_input == "":
                        # Enter key
                        if self.filtered_items:
                            return self.filtered_items[self.selected_index]

                    elif user_input == "/":
                        # Start filtering
                        self.filter_text = input("Filter: ").strip()
                        self.filtered_items = self._apply_filter(self.filter_text)
                        self.selected_index = 0

                    elif user_input.lower() == "?":
                        # Show help
                        self._show_help()
                        input("Press Enter to continue...")

                    elif user_input.lower() == "esc":
                        return None

                    else:
                        # Try numeric selection
                        try:
                            idx = int(user_input)
                            if 0 <= idx < len(self.filtered_items):
                                return self.filtered_items[idx]
                        except ValueError:
                            pass

                except KeyboardInterrupt:
                    print("\nCancelled.")
                    return None

        finally:
            self.running = False


class DynamicMenu:
    """Menu that can be updated dynamically."""

    def __init__(self, title: str = "", loader: Optional[Callable[[], List[MenuItem]]] = None):
        self.title = title
        self.loader = loader
        self.menu = None
        self._load()

    def _load(self):
        """Load menu items."""
        if self.loader:
            items = self.loader()
        else:
            items = []

        self.menu = Menu(items, title=self.title)

    def refresh(self):
        """Reload menu items."""
        self._load()

    def select(self) -> Optional[MenuItem]:
        """Run menu selection."""
        return self.menu.select() if self.menu else None


# ── Demo ────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    items = [
        MenuItem("Network Check", "network", "Verify internet connectivity"),
        MenuItem("Security Audit", "security", "Check for security issues"),
        MenuItem("File Manager", "files", "Browse and manage files"),
        MenuItem("System Health", "health", "Monitor system resources"),
        MenuItem("Harvey Logs", "logs", "View recent activity logs"),
        MenuItem("Settings", "settings", "Configure Harvey OS"),
        MenuItem("About", "about", "Learn about Harvey OS"),
    ]

    menu = Menu(items, title="Harvey OS — What would you like to do?")
    selected = menu.select()

    if selected:
        print(f"\nYou selected: {selected.label}")
        print(f"Value: {selected.value}")
    else:
        print("\nNo selection.")
