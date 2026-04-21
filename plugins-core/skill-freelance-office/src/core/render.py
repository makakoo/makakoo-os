"""Jinja2 renderer for invoices + contracts.

Template lookup order for every render call:

1. The user's office templates: ``<office_root>/templates/``.
2. The plugin-bundled fallbacks: ``src/templates/`` (shipped with the
   plugin; present on every machine with the plugin installed).

Per-country template name is picked by the caller (``generate_invoice``
reads ``regime.INVOICE_TEMPLATE`` from the office's tax module). This
module accepts an explicit ``template_name`` so v0.2 can render
``INVOICE_AR.md.j2`` or ``INVOICE_ES.md.j2`` without a second rendering
function per country.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, Optional

from jinja2 import Environment, FileSystemLoader, StrictUndefined

from . import paths
from .errors import FreelanceError


def _plugin_templates_dir() -> Path:
    return Path(__file__).resolve().parent.parent / "templates"


def _env(template_dirs: Optional[list] = None, office_root: Optional[Path] = None) -> Environment:
    if template_dirs is None:
        dirs = []
        if office_root is not None:
            dirs.append(str(paths.templates_dir_for(office_root)))
        else:
            dirs.append(str(paths.templates_dir()))
        dirs.append(str(_plugin_templates_dir()))
        template_dirs = dirs
    return Environment(
        loader=FileSystemLoader(template_dirs),
        trim_blocks=True,
        lstrip_blocks=True,
        undefined=StrictUndefined,
        keep_trailing_newline=True,
    )


def render_invoice(context: Dict[str, Any], template_name: str = "INVOICE.md.j2",
                   office_root: Optional[Path] = None) -> str:
    env = _env(office_root=office_root)
    try:
        tmpl = env.get_template(template_name)
    except Exception:
        # Fall back to the base DE template if the country-specific one
        # is missing — this keeps the plugin usable on upgrades where
        # a new country module appears before its template does.
        try:
            tmpl = env.get_template("INVOICE.md.j2")
        except Exception as e:
            raise FreelanceError(
                f"invoice template not found (tried {template_name!r} and INVOICE.md.j2): {e}"
            ) from e
    return tmpl.render(**context)


def render_contract(context: Dict[str, Any], office_root: Optional[Path] = None) -> str:
    env = _env(office_root=office_root)
    try:
        tmpl = env.get_template("PROJECT_VEREINBARUNG.md.j2")
    except Exception:
        try:
            tmpl = env.get_template("PROJECT_VEREINBARUNG.md")
        except Exception as e:
            raise FreelanceError(f"Projektvereinbarung template not found: {e}") from e
    return tmpl.render(**context)
