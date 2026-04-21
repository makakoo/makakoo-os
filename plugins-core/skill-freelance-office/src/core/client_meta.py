"""Parse + update a client ``meta.yaml`` file.

Despite the ``.yaml`` extension, the file is markdown with fenced
``yaml`` code blocks. We preserve the markdown structure and only
touch individual key:value lines inside those fences.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

from .errors import FreelanceError

_FENCE_RE = re.compile(r"```yaml\n(.*?)```", re.DOTALL)


@dataclass
class ClientMeta:
    path: Path
    text: str
    blocks: List[Dict[str, Any]] = field(default_factory=list)

    @classmethod
    def load(cls, path: Path) -> "ClientMeta":
        if not path.is_file():
            raise FreelanceError(f"client meta not found: {path}")
        text = path.read_text(encoding="utf-8")
        blocks: List[Dict[str, Any]] = []
        for m in _FENCE_RE.finditer(text):
            try:
                parsed = yaml.safe_load(m.group(1)) or {}
                if isinstance(parsed, dict):
                    blocks.append(parsed)
            except yaml.YAMLError:
                continue
        return cls(path=path, text=text, blocks=blocks)

    def flat(self) -> Dict[str, Any]:
        """Merged view of every YAML block — last write wins (matches
        the on-page reading order)."""
        out: Dict[str, Any] = {}
        for b in self.blocks:
            out.update(b)
        return out

    def set_value(self, key: str, value: Any) -> None:
        """Substitute ``<key>: <value>`` in the first fenced block that
        already declares this key. If no block has it, append a new key
        to the first block."""
        rendered = _render_scalar(value)
        pat = re.compile(rf"^(\s*){re.escape(key)}:\s*[^\n#]*(#.*)?$", re.MULTILINE)
        fences = list(_FENCE_RE.finditer(self.text))
        for m in fences:
            inner = m.group(1)
            if pat.search(inner):
                def _replace(mm):
                    comment = mm.group(2) or ""
                    sep = "  " if comment else ""
                    return f"{mm.group(1)}{key}: {rendered}{sep}{comment}"
                new_inner = pat.sub(_replace, inner, count=1)
                self.text = self.text[: m.start(1)] + new_inner + self.text[m.end(1) :]
                return
        if fences:
            m = fences[0]
            inner = m.group(1)
            new_inner = inner.rstrip("\n") + f"\n{key}: {rendered}\n"
            self.text = self.text[: m.start(1)] + new_inner + self.text[m.end(1) :]
        else:
            self.text = self.text.rstrip("\n") + f"\n\n```yaml\n{key}: {rendered}\n```\n"

    def write(self) -> None:
        self.path.write_text(self.text, encoding="utf-8")


def _render_scalar(value: Any) -> str:
    if value is None:
        return '""'
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int) or isinstance(value, float):
        return str(value)
    s = str(value)
    if s == "":
        return '""'
    if any(c in s for c in (":", "#", " ", ",", "'")) or s.startswith(("@", "-", "?")):
        return '"' + s.replace('"', '\\"') + '"'
    return s


def get(path: Path, key: str, default: Optional[Any] = None) -> Any:
    """Convenience — load, look up ``key`` in the flat view."""
    meta = ClientMeta.load(path)
    return meta.flat().get(key, default)
