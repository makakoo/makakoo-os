"""File Manager — Interactive file browser and operations."""

import os
import sys
import shutil
import json
from pathlib import Path
from typing import Optional, List

# Add parent to path for HTE import
sys.path.insert(0, str(Path(__file__).parent.parent.parent.parent.parent))

from core.terminal import Header, Menu, MenuItem, Panel, StatusBadge, Alert


class FileManager:
    """Interactive file browser with arrow-key navigation."""

    def __init__(self, start_path: str = "~"):
        self.current_path = Path(start_path).expanduser().resolve()
        self.history = [self.current_path]

    def _get_entries(self) -> List[Path]:
        """Get directory entries sorted."""
        try:
            entries = sorted(self.current_path.iterdir(), key=lambda p: (not p.is_dir(), p.name.lower()))
            return entries
        except PermissionError:
            print(f"\nPermission denied: {self.current_path}\n")
            return []

    def _format_size(self, bytes_size: int) -> str:
        """Format file size for display."""
        for unit in ("B", "KB", "MB", "GB"):
            if bytes_size < 1024:
                return f"{bytes_size:.1f}{unit}"
            bytes_size /= 1024
        return f"{bytes_size:.1f}TB"

    def _show_file_preview(self, path: Path) -> None:
        """Show preview of file contents."""
        if not path.is_file():
            return

        try:
            # Try to read as text
            if path.suffix in (".json", ".yml", ".yaml", ".txt", ".md"):
                with open(path, "r") as f:
                    lines = f.readlines()[:20]  # First 20 lines
                    print(f"\n{Panel(''.join(lines), path.name, 80).render()}\n")
            else:
                # Binary file
                size = self._format_size(path.stat().st_size)
                print(f"\n{Alert(f'Binary file ({size})', 'info').render()}\n")
        except Exception as e:
            print(f"\n{Alert(f'Cannot preview: {e}', 'error').render()}\n")

    def _delete_file(self, path: Path) -> bool:
        """Delete file with confirmation."""
        if not path.exists():
            print(f"\n{Alert('File not found', 'error').render()}\n")
            return False

        # Show what will be deleted
        size = self._format_size(path.stat().st_size) if path.is_file() else "folder"
        msg = f"Delete {path.name} ({size})? This cannot be undone."

        confirm = input(f"\n⚠️  {msg} [y/N]: ").strip().lower()
        if confirm in ("y", "yes"):
            try:
                if path.is_file():
                    path.unlink()
                else:
                    shutil.rmtree(path)
                print(f"\n{Alert('Deleted successfully', 'success').render()}\n")
                return True
            except Exception as e:
                print(f"\n{Alert(f'Failed to delete: {e}', 'error').render()}\n")
                return False

        return False

    def _copy_file(self, path: Path) -> bool:
        """Copy file to destination."""
        if not path.exists():
            return False

        dest = input(f"Copy to (relative to {self.current_path}): ").strip()
        if not dest:
            return False

        dest_path = self.current_path / dest

        try:
            if path.is_file():
                shutil.copy2(path, dest_path)
            else:
                shutil.copytree(path, dest_path)
            print(f"\n{Alert('Copied successfully', 'success').render()}\n")
            return True
        except Exception as e:
            print(f"\n{Alert(f'Failed to copy: {e}', 'error').render()}\n")
            return False

    def browse(self) -> None:
        """Interactive file browser."""
        print("\n" + Header("File Manager", str(self.current_path)).render() + "\n")

        while True:
            entries = self._get_entries()

            if not entries:
                print(f"Empty directory or permission denied.\n")
                break

            # Build menu items
            items = []
            items.append(MenuItem("..", None, "Go to parent directory"))

            for entry in entries:
                size_str = ""
                if entry.is_file():
                    size_str = self._format_size(entry.stat().st_size)
                    icon = "📄"
                else:
                    icon = "📁"

                description = f"{icon} {size_str}" if size_str else icon
                items.append(MenuItem(entry.name, entry, description))

            menu = Menu(items, title=f"Navigate {self.current_path}")
            selected = menu.select()

            if not selected:
                break

            # Handle selection
            if selected.value is None:
                # Go up
                if self.current_path.parent != self.current_path:
                    self.current_path = self.current_path.parent
                continue

            path = selected.value

            if path.is_dir():
                try:
                    self.current_path = path
                except PermissionError:
                    print(f"\n{Alert('Permission denied', 'error').render()}\n")

            else:
                # File — show options
                print(f"\nFile: {path.name}\n")
                action = input("Action (v=view, d=delete, c=copy, or Enter to close): ").strip().lower()

                if action == "v":
                    self._show_file_preview(path)
                elif action == "d":
                    if self._delete_file(path):
                        break  # Refresh directory
                elif action == "c":
                    self._copy_file(path)


def file_manager_main():
    """Run interactive file manager."""
    print("\n" + Header("File Manager", "Browse and manage files").render() + "\n")

    start_path = input("Start path (default: home): ").strip() or "~"

    manager = FileManager(start_path)
    manager.browse()

    print("\nFile manager closed.\n")


if __name__ == "__main__":
    file_manager_main()
