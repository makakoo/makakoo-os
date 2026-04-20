"""
Coordinator Scratchpad — Shared filesystem workspace for worker agents.

Each task gets an isolated directory under data/scratch/{task_id}.
Workers read/write intermediate artifacts here so the next stage
always has structured input instead of raw LLM dumps.
"""

import os
import time
from pathlib import Path
from typing import Dict, List, Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))


class Scratchpad:
    """Shared filesystem workspace scoped to a single task."""

    def __init__(self, task_id: str):
        self.task_id = task_id
        self.base_dir = Path(HARVEY_HOME) / "data" / "scratch" / task_id
        self.base_dir.mkdir(parents=True, exist_ok=True)

    def write(self, filename: str, content: str) -> Path:
        """Write content to a file in the scratchpad. Returns the file path."""
        path = self.base_dir / filename
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        return path

    def read(self, filename: str) -> Optional[str]:
        """Read a file from the scratchpad. Returns None if missing."""
        path = self.base_dir / filename
        if not path.exists():
            return None
        return path.read_text(encoding="utf-8")

    def append(self, filename: str, content: str) -> Path:
        """Append content to a file (creates if missing). Returns the file path."""
        path = self.base_dir / filename
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a", encoding="utf-8") as f:
            f.write(content)
        return path

    def list_files(self) -> List[str]:
        """List all files in the scratchpad (relative paths)."""
        if not self.base_dir.exists():
            return []
        return sorted(
            str(p.relative_to(self.base_dir))
            for p in self.base_dir.rglob("*")
            if p.is_file()
        )

    def manifest(self) -> Dict[str, dict]:
        """Return file manifest with sizes and timestamps."""
        result = {}
        for rel_path in self.list_files():
            full = self.base_dir / rel_path
            stat = full.stat()
            result[rel_path] = {
                "size_bytes": stat.st_size,
                "modified": time.ctime(stat.st_mtime),
                "modified_ts": stat.st_mtime,
            }
        return result

    def cleanup(self) -> None:
        """Remove the entire scratchpad directory."""
        import shutil
        if self.base_dir.exists():
            shutil.rmtree(self.base_dir)

    def __repr__(self) -> str:
        return f"Scratchpad(task_id={self.task_id!r}, dir={self.base_dir})"
