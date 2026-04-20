"""
Harvey v4 Skill Dispatcher — unified `harvey skill run/list/info` across every
infected CLI host.

Canonical skill tree lives at ``~/MAKAKOO/skills-shared/`` with three subtrees:

    skills-shared/
      gstack/        # Real gstack skill packages (browse, ship, qa, cso, ...)
      harvey/        # Harvey native skills, categorized (dev, meta, ai-ml, ...)
      user/          # Individual Claude skills + shared agents skills

Resolution order for ``harvey skill run <name>``:

    1. gstack/<name>/
    2. harvey/<category>/<name>/   (scanned across categories)
    3. user/<name>/
    4. error (not found)

Entrypoint detection (per resolved skill dir):

    1. SKILL.md frontmatter ``entrypoint:`` field  (explicit opt-in)
    2. dist/<name>                                 (gstack pattern, preferred)
    3. bin/<name>                                  (generic)
    4. run.sh                                      (shell wrapper)
    5. <name>.py with shebang                      (python single-file)
    6. docs-only fallback                          (print SKILL.md, exit 0)

Arguments after <name> are forwarded verbatim to the resolved executable;
exit code and stdout/stderr propagate.

Designed to be callable three ways:

    - ``harvey skill run browse ...`` (when the ``harvey`` CLI binary exists)
    - ``python3 -m core.orchestration.skill_dispatcher run browse ...``
    - ``SkillDispatcher().run("browse", [...])`` programmatically
"""
from __future__ import annotations

import argparse
import json
import os
import shlex
import stat
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable, List, Optional

CANONICAL = Path(os.environ.get("HARVEY_HOME", str(Path.home() / "MAKAKOO"))) / "skills-shared"
_SUBTREES = ("gstack", "harvey", "user")
_ENTRYPOINT_PATTERNS = ("dist/{name}", "bin/{name}", "run.sh")


@dataclass
class ResolvedSkill:
    name: str
    source: str  # "gstack" | "harvey" | "user"
    path: Path
    entrypoint: Optional[Path]
    category: Optional[str] = None
    docs_only: bool = False


@dataclass
class SkillInfo:
    name: str
    source: str
    path: str
    entrypoint: Optional[str]
    category: Optional[str]
    docs_only: bool
    description: str = ""


@dataclass
class DispatcherError(Exception):
    message: str
    exit_code: int = 1

    def __str__(self) -> str:  # noqa: D401
        return self.message


# ----------------------------------------------------------------------
# Core resolution
# ----------------------------------------------------------------------


def _is_executable(path: Path) -> bool:
    if not path.is_file():
        return False
    return bool(path.stat().st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH))


def _has_python_shebang(path: Path) -> bool:
    if not path.is_file():
        return False
    try:
        with open(path, "rb") as f:
            head = f.read(64)
    except OSError:
        return False
    return head.startswith(b"#!") and b"python" in head.split(b"\n", 1)[0]


def _parse_frontmatter_entrypoint(skill_md: Path) -> Optional[str]:
    """
    Read SKILL.md and extract an ``entrypoint:`` key from the YAML-ish frontmatter
    block delimited by ``---`` lines at the top of the file. Pure text parsing,
    no PyYAML dep.
    """
    try:
        text = skill_md.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    if not text.startswith("---"):
        return None
    end = text.find("\n---", 3)
    if end == -1:
        return None
    block = text[3:end]
    for line in block.splitlines():
        line = line.strip()
        if line.startswith("entrypoint:"):
            _, _, value = line.partition(":")
            return value.strip().strip('"').strip("'")
    return None


def _parse_frontmatter_description(skill_md: Path) -> str:
    try:
        text = skill_md.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    if not text.startswith("---"):
        return ""
    end = text.find("\n---", 3)
    if end == -1:
        return ""
    for line in text[3:end].splitlines():
        line = line.strip()
        if line.startswith("description:"):
            _, _, value = line.partition(":")
            return value.strip().strip('"').strip("'")
    return ""


def _detect_entrypoint(skill_dir: Path, name: str) -> Optional[Path]:
    skill_md = skill_dir / "SKILL.md"
    if skill_md.exists():
        frontmatter_ep = _parse_frontmatter_entrypoint(skill_md)
        if frontmatter_ep:
            candidate = (skill_dir / frontmatter_ep).resolve()
            if candidate.exists():
                return candidate
    for pat in _ENTRYPOINT_PATTERNS:
        candidate = skill_dir / pat.format(name=name)
        if candidate.exists() and _is_executable(candidate):
            return candidate
        if candidate.exists() and candidate.is_file():
            return candidate
    py_main = skill_dir / f"{name}.py"
    if py_main.exists() and _has_python_shebang(py_main):
        return py_main
    return None


class SkillDispatcher:
    def __init__(self, canonical: Path = CANONICAL) -> None:
        self.canonical = canonical

    # ---- listing ----

    def _iter_gstack(self) -> Iterable[ResolvedSkill]:
        root = self.canonical / "gstack"
        if not root.exists():
            return
        for child in sorted(root.iterdir()):
            if not child.is_dir():
                continue
            skill_md = child / "SKILL.md"
            if not skill_md.exists():
                continue
            ep = _detect_entrypoint(child, child.name)
            yield ResolvedSkill(
                name=child.name,
                source="gstack",
                path=child,
                entrypoint=ep,
                docs_only=ep is None,
            )

    def _iter_harvey(self) -> Iterable[ResolvedSkill]:
        root = self.canonical / "harvey"
        if not root.exists():
            return
        for category in sorted(root.iterdir()):
            if not category.is_dir():
                continue
            for child in sorted(category.iterdir()):
                if not child.is_dir():
                    continue
                skill_md = child / "SKILL.md"
                if not skill_md.exists():
                    continue
                ep = _detect_entrypoint(child, child.name)
                yield ResolvedSkill(
                    name=child.name,
                    source="harvey",
                    path=child,
                    entrypoint=ep,
                    category=category.name,
                    docs_only=ep is None,
                )

    def _iter_user(self) -> Iterable[ResolvedSkill]:
        root = self.canonical / "user"
        if not root.exists():
            return
        for child in sorted(root.iterdir()):
            if not child.is_dir():
                continue
            skill_md = child / "SKILL.md"
            if not skill_md.exists():
                continue
            ep = _detect_entrypoint(child, child.name)
            yield ResolvedSkill(
                name=child.name,
                source="user",
                path=child,
                entrypoint=ep,
                docs_only=ep is None,
            )

    def iter_all(self) -> Iterable[ResolvedSkill]:
        yield from self._iter_gstack()
        yield from self._iter_harvey()
        yield from self._iter_user()

    # ---- resolution ----

    def resolve(self, name: str, source: Optional[str] = None) -> ResolvedSkill:
        """
        Resolve a skill by name, optionally restricted to a single source.
        Precedence (no source filter): gstack > harvey > user.
        """
        if source and source not in _SUBTREES:
            raise DispatcherError(
                f"invalid --source '{source}' (must be one of: {', '.join(_SUBTREES)})"
            )

        search_order: List[str] = [source] if source else list(_SUBTREES)

        # gstack / user: flat lookup
        # harvey: category scan
        for s in search_order:
            if s == "gstack":
                candidate = self.canonical / "gstack" / name
                skill_md = candidate / "SKILL.md"
                if skill_md.exists():
                    ep = _detect_entrypoint(candidate, name)
                    return ResolvedSkill(
                        name=name,
                        source="gstack",
                        path=candidate,
                        entrypoint=ep,
                        docs_only=ep is None,
                    )
            elif s == "harvey":
                root = self.canonical / "harvey"
                if root.exists():
                    for category in sorted(root.iterdir()):
                        if not category.is_dir():
                            continue
                        candidate = category / name
                        skill_md = candidate / "SKILL.md"
                        if skill_md.exists():
                            ep = _detect_entrypoint(candidate, name)
                            return ResolvedSkill(
                                name=name,
                                source="harvey",
                                path=candidate,
                                entrypoint=ep,
                                category=category.name,
                                docs_only=ep is None,
                            )
            elif s == "user":
                candidate = self.canonical / "user" / name
                skill_md = candidate / "SKILL.md"
                if skill_md.exists():
                    ep = _detect_entrypoint(candidate, name)
                    return ResolvedSkill(
                        name=name,
                        source="user",
                        path=candidate,
                        entrypoint=ep,
                        docs_only=ep is None,
                    )

        raise DispatcherError(
            f"skill not found: '{name}' (searched {', '.join(search_order)}) — try: harvey skill list",
            exit_code=2,
        )

    # ---- actions ----

    def run(self, name: str, args: Optional[List[str]] = None, source: Optional[str] = None) -> int:
        args = list(args or [])
        skill = self.resolve(name, source=source)
        if skill.docs_only:
            # Print the SKILL.md and exit 0 — docs-only skill
            skill_md = skill.path / "SKILL.md"
            try:
                sys.stdout.write(skill_md.read_text(encoding="utf-8"))
            except OSError as exc:
                raise DispatcherError(f"unable to read {skill_md}: {exc}") from exc
            return 0
        assert skill.entrypoint is not None
        cmd: List[str]
        if _has_python_shebang(skill.entrypoint):
            cmd = [sys.executable, str(skill.entrypoint), *args]
        elif _is_executable(skill.entrypoint):
            cmd = [str(skill.entrypoint), *args]
        elif skill.entrypoint.suffix == ".sh":
            cmd = ["/bin/sh", str(skill.entrypoint), *args]
        else:
            cmd = [str(skill.entrypoint), *args]
        result = subprocess.run(cmd, env=os.environ.copy())
        return result.returncode

    def list(
        self,
        source_filter: Optional[str] = None,
        category_filter: Optional[str] = None,
    ) -> List[SkillInfo]:
        infos: List[SkillInfo] = []
        for s in self.iter_all():
            if source_filter and s.source != source_filter:
                continue
            if category_filter and s.category != category_filter:
                continue
            skill_md = s.path / "SKILL.md"
            desc = _parse_frontmatter_description(skill_md) if skill_md.exists() else ""
            infos.append(
                SkillInfo(
                    name=s.name,
                    source=s.source,
                    path=str(s.path),
                    entrypoint=str(s.entrypoint) if s.entrypoint else None,
                    category=s.category,
                    docs_only=s.docs_only,
                    description=desc,
                )
            )
        return infos

    def info(self, name: str, source: Optional[str] = None) -> str:
        skill = self.resolve(name, source=source)
        lines = [
            f"Name:        {skill.name}",
            f"Source:      {skill.source}",
            f"Path:        {skill.path}",
            f"Category:    {skill.category or '-'}",
            f"Entrypoint:  {skill.entrypoint or '(docs-only)'}",
            f"Docs-only:   {skill.docs_only}",
            "",
            "--- SKILL.md ---",
        ]
        skill_md = skill.path / "SKILL.md"
        if skill_md.exists():
            lines.append(skill_md.read_text(encoding="utf-8", errors="replace"))
        else:
            lines.append("(no SKILL.md)")
        return "\n".join(lines)


# ----------------------------------------------------------------------
# CLI entrypoint
# ----------------------------------------------------------------------


def _cmd_run(args: argparse.Namespace) -> int:
    dispatcher = SkillDispatcher()
    try:
        return dispatcher.run(args.name, args.passthrough, source=args.source)
    except DispatcherError as exc:
        sys.stderr.write(f"error: {exc}\n")
        return exc.exit_code


def _cmd_list(args: argparse.Namespace) -> int:
    dispatcher = SkillDispatcher()
    infos = dispatcher.list(source_filter=args.source, category_filter=args.category)
    if args.json:
        payload = [
            {
                "name": i.name,
                "source": i.source,
                "path": i.path,
                "entrypoint": i.entrypoint,
                "category": i.category,
                "docs_only": i.docs_only,
                "description": i.description,
            }
            for i in infos
        ]
        print(json.dumps(payload, indent=2))
        return 0
    by_source = {"gstack": [], "harvey": [], "user": []}
    for i in infos:
        by_source.setdefault(i.source, []).append(i)
    for src in ("gstack", "harvey", "user"):
        items = by_source.get(src, [])
        if not items:
            continue
        print(f"=== {src} ({len(items)}) ===")
        for i in items:
            tag = "" if not i.docs_only else " [docs]"
            cat = f" [{i.category}]" if i.category else ""
            desc = f" — {i.description}" if i.description else ""
            print(f"  {i.name}{cat}{tag}{desc}")
    print(f"\nTotal: {len(infos)} skills")
    return 0


def _cmd_info(args: argparse.Namespace) -> int:
    dispatcher = SkillDispatcher()
    try:
        print(dispatcher.info(args.name, source=args.source))
        return 0
    except DispatcherError as exc:
        sys.stderr.write(f"error: {exc}\n")
        return exc.exit_code


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="harvey skill",
        description="Harvey v4 unified skill dispatcher — run any skill from any CLI.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_run = sub.add_parser("run", help="Run a skill by name")
    p_run.add_argument("name", help="Skill name")
    p_run.add_argument("--source", choices=_SUBTREES, help="Force source subtree")
    p_run.add_argument("passthrough", nargs=argparse.REMAINDER, help="Args passed to skill")
    p_run.set_defaults(func=_cmd_run)

    p_list = sub.add_parser("list", help="List available skills")
    p_list.add_argument("--source", choices=_SUBTREES, help="Filter by source")
    p_list.add_argument("--category", help="Filter by harvey category")
    p_list.add_argument("--json", action="store_true", help="Emit JSON instead of text")
    p_list.set_defaults(func=_cmd_list)

    p_info = sub.add_parser("info", help="Show skill details + SKILL.md")
    p_info.add_argument("name")
    p_info.add_argument("--source", choices=_SUBTREES, help="Force source subtree")
    p_info.set_defaults(func=_cmd_info)

    return parser


def main(argv: Optional[List[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
