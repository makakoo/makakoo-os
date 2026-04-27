"""Invoice Markdown → PDF renderer (Phase 1, pi A1).

Pipeline: invoice ``.md`` → minimal HTML via ``markdown`` lib →
WeasyPrint → ``.pdf`` on disk. No LaTeX, no kernel-capability grant;
runs entirely in-process. Output quality is characterised by the
tests in ``tests/test_pdf_render.py`` — we assert valid PDF bytes
plus that invoice_number / net / IBAN / payment-terms text is
present when parsed back via pypdf.

The stylesheet is intentionally minimal — the Markdown content is
the source of truth for every invoice layout decision. Anything
fancier lives in the per-country invoice templates.
"""
from __future__ import annotations

from pathlib import Path
from typing import Union

from .errors import FreelanceError


_DEFAULT_CSS = """
@page { size: A4; margin: 2cm 2.2cm; }
body { font-family: -apple-system, "Helvetica Neue", Arial, sans-serif; font-size: 10.5pt; color: #111; line-height: 1.45; }
h1 { font-size: 18pt; margin: 0 0 0.6em 0; }
h2 { font-size: 13pt; margin-top: 1.2em; }
h3 { font-size: 11pt; }
table { border-collapse: collapse; width: 100%; margin: 0.8em 0; }
th, td { border: 1px solid #ccc; padding: 4pt 6pt; text-align: left; vertical-align: top; }
th { background: #f3f3f3; }
code { font-family: "SFMono-Regular", Menlo, monospace; font-size: 9.5pt; }
hr { border: 0; border-top: 1px solid #bbb; margin: 1.2em 0; }
"""


def render_markdown_to_pdf(
    md_path: Union[str, Path],
    pdf_path: Union[str, Path],
) -> Path:
    """Render ``md_path`` to ``pdf_path``.

    Raises :class:`FreelanceError` on any renderer failure so the
    calling command can surface a clean error to the user instead of
    a stack trace.
    """
    md_p = Path(md_path)
    pdf_p = Path(pdf_path)
    if not md_p.is_file():
        raise FreelanceError(f"markdown source not found: {md_p}")

    try:
        import markdown as md_lib
        from weasyprint import HTML, CSS
    except ImportError as e:  # pragma: no cover - env bootstrap only
        raise FreelanceError(
            f"PDF render dependency missing: {e}. "
            f"Install with `pip install weasyprint markdown`."
        ) from e

    md_text = md_p.read_text(encoding="utf-8")
    html_body = md_lib.markdown(
        md_text,
        extensions=["tables", "fenced_code", "sane_lists"],
    )
    html_doc = (
        "<!doctype html><html><head><meta charset='utf-8'>"
        f"<title>{md_p.stem}</title></head>"
        f"<body>{html_body}</body></html>"
    )
    try:
        pdf_p.parent.mkdir(parents=True, exist_ok=True)
        HTML(string=html_doc, base_url=str(md_p.parent)).write_pdf(
            target=str(pdf_p),
            stylesheets=[CSS(string=_DEFAULT_CSS)],
        )
    except Exception as e:  # weasyprint raises a variety of types
        raise FreelanceError(f"PDF render failed: {e}") from e
    return pdf_p
