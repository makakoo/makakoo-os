"""pytest fixtures + sys.path bootstrap.

Python doesn't allow hyphens in module names, but the plugin lives at
`plugins-core/agent-harveychat/`. We make the python/ folder
addressable as `plugins_core.agent_harveychat.python` by inserting
two parent paths into sys.path and re-aliasing the hyphenated dirs
under their underscore-equivalent module names.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

_HERE = Path(__file__).resolve().parent  # plugins-core/agent-harveychat/python/
_PLUGIN_ROOT = _HERE.parent  # plugins-core/agent-harveychat/
_REPO_ROOT = _PLUGIN_ROOT.parent.parent  # makakoo-os/

# Ensure the repo root is on sys.path so `import plugins_core` works.
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))


def _alias_package(real_dir: Path, alias_name: str) -> ModuleType:
    """Create a virtual package whose __path__ points at `real_dir`."""
    if alias_name in sys.modules:
        return sys.modules[alias_name]
    spec = importlib.util.spec_from_file_location(
        alias_name,
        real_dir / "__init__.py" if (real_dir / "__init__.py").exists() else None,
        submodule_search_locations=[str(real_dir)],
    )
    if spec is None or spec.loader is None:
        # No __init__.py — synthesize a package on the fly.
        mod = ModuleType(alias_name)
        mod.__path__ = [str(real_dir)]  # type: ignore[attr-defined]
        sys.modules[alias_name] = mod
        return mod
    module = importlib.util.module_from_spec(spec)
    sys.modules[alias_name] = module
    spec.loader.exec_module(module)
    return module


# Build the alias chain: plugins_core → plugins_core.agent_harveychat
# → plugins_core.agent_harveychat.python.
_alias_package(_REPO_ROOT / "plugins-core", "plugins_core")
_alias_package(_PLUGIN_ROOT, "plugins_core.agent_harveychat")
_alias_package(_HERE, "plugins_core.agent_harveychat.python")

# Also expose the python/ directory directly on sys.path so flat
# `from bridge import ...` works the same way in tests as in
# production (the supervisor cd's into python/ before launching).
# Then bind the flat module names to the SAME module object as the
# package-aliased names so an `OutboundFrame` imported from
# `bridge` is the same class as one imported from
# `plugins_core.agent_harveychat.python.bridge`. Without this,
# isinstance checks across the two import paths fail.
if str(_HERE) not in sys.path:
    sys.path.insert(0, str(_HERE))

import importlib  # noqa: E402

for _flat_name in ("bridge", "tool_dispatcher", "file_enforcement", "brain_sync"):
    _full = f"plugins_core.agent_harveychat.python.{_flat_name}"
    _mod = importlib.import_module(_full)
    sys.modules[_flat_name] = _mod
