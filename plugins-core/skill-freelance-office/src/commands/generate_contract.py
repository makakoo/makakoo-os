"""freelance-office generate-contract — render a Projektvereinbarung.

Creates ``clients/<client>/projects/<project>/`` + contracts subdir
on demand. Existing v1 is never overwritten — bumps to v2, v3, …
"""
from __future__ import annotations

import json
import shutil
from datetime import date
from pathlib import Path
from typing import Any, Dict, List

from jinja2 import Environment, FileSystemLoader, StrictUndefined

from ..core import brain, client_meta, paths, render, settings
from ..core.errors import FreelanceError, NotInitialisedError


def add_arguments(parser):
    parser.add_argument("--client", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--title", required=True)
    parser.add_argument("--description", default="")
    parser.add_argument("--meilensteine", default="[]", help='JSON list of {"name","description","due_date"}')
    parser.add_argument("--total-days", type=float, required=True)
    parser.add_argument("--rate", type=float, default=None, help="override day rate (default: client.day_rate_agreed)")


def _parse_meilensteine(raw: str) -> List[Dict[str, str]]:
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        raise FreelanceError(f"--meilensteine must be JSON: {e}") from e
    if not isinstance(data, list):
        raise FreelanceError("--meilensteine must be a JSON list")
    out = []
    for i, m in enumerate(data):
        if not isinstance(m, dict):
            raise FreelanceError(f"meilenstein[{i}] must be an object")
        out.append({
            "name": str(m.get("name", "")),
            "description": str(m.get("description", "")),
            "due_date": str(m.get("due_date", "")),
        })
    return out


def _next_version(contracts_dir: Path, project: str) -> int:
    existing = sorted(contracts_dir.glob(f"{project}-v*.md"))
    if not existing:
        return 1
    highest = 0
    for f in existing:
        stem = f.stem
        try:
            n = int(stem.rsplit("-v", 1)[1])
            if n > highest:
                highest = n
        except (IndexError, ValueError):
            continue
    return highest + 1


def _ensure_tracker(project_dir: Path, client_slug: str, project_slug: str, title: str,
                    day_rate: float, agreed_days: float) -> None:
    tracker_path = project_dir / "_project-tracker.md"
    if tracker_path.is_file():
        return
    env = Environment(
        loader=FileSystemLoader(str(Path(__file__).resolve().parent.parent / "templates")),
        trim_blocks=True, lstrip_blocks=True,
        undefined=StrictUndefined, keep_trailing_newline=True,
    )
    ctx = {
        "project_title": title,
        "client_slug": client_slug,
        "project_slug": project_slug,
        "started": str(date.today()),
        "agreed_days": int(agreed_days),
        "day_rate": int(day_rate),
        "total_contract_value": int(day_rate * agreed_days),
    }
    rendered = env.get_template("PROJECT_TRACKER.md.j2").render(**ctx)
    project_dir.mkdir(parents=True, exist_ok=True)
    tracker_path.write_text(rendered, encoding="utf-8")


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    s = settings.load_settings_at(home)

    client_dir = home / "clients" / args.client
    if not client_dir.is_dir():
        raise NotInitialisedError(f"client {args.client!r} missing; run onboard-client")
    meta = client_meta.ClientMeta.load(client_dir / "meta.yaml").flat()
    day_rate = float(args.rate if args.rate is not None else (meta.get("day_rate_agreed") or 0))
    if day_rate <= 0:
        raise FreelanceError(
            "day rate missing — pass --rate or set client.meta.day_rate_agreed via onboard-client"
        )
    milestones = _parse_meilensteine(args.meilensteine)
    total_net = day_rate * float(args.total_days)

    reverse_charge = (
        not s.tax.kleinunternehmer
        and bool(meta.get("b2b", True))
        and str(meta.get("client_country", "DE")).upper() != "DE"
        and bool(str(meta.get("ust_id", "")).strip())
    )
    ctx: Dict[str, Any] = {
        "project_title": args.title,
        "description": args.description,
        "day_rate": day_rate,
        "total_days": int(args.total_days),
        "total_net": round(total_net, 2),
        "meilensteine": milestones,
        "payment_terms_days": int(meta.get("payment_terms_days") or s.finance.payment_terms_days),
        "started": str(date.today()),
        "advance_payment": "50% bei Start" if total_net > 10_000 else "",
        "kleinunternehmer": bool(s.tax.kleinunternehmer),
        "reverse_charge": reverse_charge,
        "from_name": s.identity.name,
        "from_dba": s.identity.dba,
        "from_email": s.identity.email,
        "to_name": str(meta.get("name") or args.client),
        "to_email": str(meta.get("contact_email") or ""),
    }

    project_dir = client_dir / "projects" / args.project
    dry = bool(getattr(args, "dry_run", False))

    rendered = render.render_contract(ctx, office_root=home)

    contracts_dir = project_dir / "contracts"
    version = _next_version(contracts_dir, args.project) if contracts_dir.exists() else 1
    out_path = contracts_dir / f"{args.project}-v{version}.md"

    if dry:
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_write": str(out_path),
            "version": version,
            "day_rate": day_rate,
            "total_days": args.total_days,
            "total_net": round(total_net, 2),
            "meilensteine": len(milestones),
            "preview_bytes": len(rendered),
            "message": f"dry-run: would create contract v{version} at {out_path}",
        }

    _ensure_tracker(project_dir, args.client, args.project, args.title, day_rate, float(args.total_days))
    contracts_dir.mkdir(parents=True, exist_ok=True)
    out_path.write_text(rendered, encoding="utf-8")

    journal_line = (
        f"Generated contract v{version} for [[{args.client}/{args.project}]] — "
        f"{int(args.total_days)} days × €{day_rate:.0f} = €{total_net:.2f} net. [[freelance-office]]"
    )
    warnings = []
    try:
        journaled = brain.append_journal_line(journal_line)
    except FreelanceError as e:
        warnings.append(f"brain journal failed: {e}")
        journaled = None

    return {
        "status": "ok",
        "exit_code": 0,
        "client": args.client,
        "project": args.project,
        "path": str(out_path),
        "version": version,
        "day_rate": day_rate,
        "total_days": args.total_days,
        "total_net": round(total_net, 2),
        "milestones": len(milestones),
        "journal": journaled,
        "warnings": warnings,
        "message": f"contract v{version} at {out_path}. Review before sending.",
    }
