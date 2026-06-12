#!/usr/bin/env python3
"""Makakoo OS persona registry capture tool.

Portable, dependency-free. Writes:
- $MAKAKOO_HOME/config/persona_registry.json
- $MAKAKOO_HOME/config/persona_context.md

Never renames the primary persona unless explicitly requested with set-primary
and --yes-really.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

SCHEMA_VERSION = 1
MAX_EVENTS = 100


def now_iso() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def home() -> Path:
    for key in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(key)
        if val:
            return Path(val).expanduser().resolve()
    return (Path.home() / "MAKAKOO").resolve()


def slug(value: str) -> str:
    value = value.strip().lower()
    value = re.sub(r"[^a-z0-9]+", "-", value)
    return value.strip("-") or "unknown"


def clean_name(value: str) -> str:
    value = value.strip().strip(".!,;:()[]{}\"'")
    value = re.sub(r"\s+", " ", value)
    # Stop at conjunctions likely starting a new clause.
    value = re.split(r"\s+(?:and|but|also|then|because)\s+", value, maxsplit=1, flags=re.I)[0]
    return value.strip().strip(".!,;:()[]{}\"'")


def config_dir() -> Path:
    d = home() / "config"
    d.mkdir(parents=True, exist_ok=True)
    return d


def persona_path() -> Path:
    return config_dir() / "persona.json"


def registry_path() -> Path:
    return config_dir() / "persona_registry.json"


def context_path() -> Path:
    return config_dir() / "persona_context.md"


def load_json(path: Path, default: Dict[str, Any]) -> Dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else default
    except (OSError, json.JSONDecodeError):
        return default


def atomic_write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(path)


def write_json(path: Path, data: Dict[str, Any]) -> None:
    atomic_write(path, json.dumps(data, indent=2, ensure_ascii=False, sort_keys=True) + "\n")


def load_primary_persona() -> Dict[str, Any]:
    data = load_json(persona_path(), {})
    name = str(data.get("name") or "Harvey")
    user = str(data.get("user") or "Sebastian")
    pronouns = str(data.get("pronouns") or "he/him")
    return {
        "id": slug(name),
        "name": name,
        "source": "config/persona.json",
        "pronouns": pronouns,
        "role": "primary Makakoo OS assistant persona",
        "rename_requires_explicit_primary_intent": True,
        "user_from_persona_json": user,
    }


def default_registry() -> Dict[str, Any]:
    primary = load_primary_persona()
    user_name = str(primary.get("user_from_persona_json") or "Sebastian")
    ts = now_iso()
    reg: Dict[str, Any] = {
        "version": SCHEMA_VERSION,
        "updated_at": ts,
        "primary_persona": {k: v for k, v in primary.items() if k != "user_from_persona_json"},
        "user": {"id": slug(user_name), "name": user_name},
        "companions": {},
        "channel_bindings": {"default-cli": primary["id"]},
        "rules": {
            "primary_persona_file": "config/persona.json",
            "companion_registry_file": "config/persona_registry.json",
            "primary_rename_requires_explicit": True,
            "identity_answer_rule": "Use channel binding when known; otherwise answer as primary persona. Mention companions only when relevant.",
        },
        "events": [],
    }
    ensure_builtin_companions(reg)
    return reg


def load_registry() -> Dict[str, Any]:
    reg = load_json(registry_path(), {})
    if not reg:
        return default_registry()
    reg.setdefault("version", SCHEMA_VERSION)
    reg.setdefault("updated_at", now_iso())
    reg.setdefault("primary_persona", {k: v for k, v in load_primary_persona().items() if k != "user_from_persona_json"})
    primary_user = load_primary_persona().get("user_from_persona_json") or "Sebastian"
    reg.setdefault("user", {"id": slug(str(primary_user)), "name": str(primary_user)})
    reg.setdefault("companions", {})
    reg.setdefault("channel_bindings", {"default-cli": reg.get("primary_persona", {}).get("id", "harvey")})
    reg.setdefault("rules", {})
    reg.setdefault("events", [])
    # Keep primary mirror fresh without clobbering registry-specific rules.
    reg["primary_persona"].update({k: v for k, v in load_primary_persona().items() if k != "user_from_persona_json"})
    ensure_builtin_companions(reg)
    return reg


def add_event(reg: Dict[str, Any], kind: str, payload: Dict[str, Any]) -> None:
    events = reg.setdefault("events", [])
    events.append({"at": now_iso(), "kind": kind, "payload": payload})
    if len(events) > MAX_EVENTS:
        del events[:-MAX_EVENTS]
    reg["updated_at"] = now_iso()


def ensure_builtin_companions(reg: Dict[str, Any]) -> None:
    companions = reg.setdefault("companions", {})
    ts = now_iso()
    if "olibia" not in companions:
        companions["olibia"] = {
            "id": "olibia",
            "name": "Olibia",
            "kind": "mascot",
            "roles": ["guardian owl mascot; protective, encouraging, terse"],
            "channels": ["harveychat", "mascot"],
            "relationships": [{"kind": "companion-of", "target": "harvey", "label": "Olibia supports Harvey and Sebastian"}],
            "notes": ["Use sparingly; owl wisdom is rare and valuable."],
            "created_at": ts,
            "updated_at": ts,
        }
    reg.setdefault("channel_bindings", {}).setdefault("harveychat", "olibia")


def upsert_user(reg: Dict[str, Any], name: str, full_name: Optional[str] = None, note: Optional[str] = None) -> None:
    name = clean_name(name)
    user = reg.setdefault("user", {})
    user.update({"id": slug(name), "name": name, "updated_at": now_iso()})
    if full_name:
        user["full_name"] = full_name.strip()
    if note:
        notes = user.setdefault("notes", [])
        if note not in notes:
            notes.append(note)
    add_event(reg, "set-user", {"name": name, "full_name": full_name, "note": note})


def parse_relationship(raw: str) -> Dict[str, str]:
    # Format: kind:target:label. Missing label gets generated.
    parts = raw.split(":", 2)
    if len(parts) == 1:
        return {"kind": "related-to", "target": slug(parts[0]), "label": raw}
    if len(parts) == 2:
        kind, target = parts
        return {"kind": kind.strip() or "related-to", "target": slug(target), "label": f"{kind.strip()} {target.strip()}"}
    kind, target, label = parts
    return {"kind": kind.strip() or "related-to", "target": slug(target), "label": label.strip()}


def upsert_companion(
    reg: Dict[str, Any],
    name: str,
    kind: str = "channel-companion",
    roles: Optional[List[str]] = None,
    channels: Optional[List[str]] = None,
    relationships: Optional[List[str]] = None,
    note: Optional[str] = None,
) -> str:
    name = clean_name(name)
    comp_id = slug(name)
    companions = reg.setdefault("companions", {})
    ts = now_iso()
    comp = companions.setdefault(
        comp_id,
        {
            "id": comp_id,
            "name": name,
            "kind": kind,
            "roles": [],
            "channels": [],
            "relationships": [],
            "notes": [],
            "created_at": ts,
        },
    )
    comp["name"] = name
    comp["kind"] = kind or comp.get("kind") or "channel-companion"
    for role in roles or []:
        role = role.strip()
        if role and role not in comp.setdefault("roles", []):
            comp["roles"].append(role)
    for channel in channels or []:
        channel = slug(channel)
        if channel and channel not in comp.setdefault("channels", []):
            comp["channels"].append(channel)
        if channel:
            reg.setdefault("channel_bindings", {})[channel] = comp_id
    for rel_raw in relationships or []:
        rel = parse_relationship(rel_raw)
        rels = comp.setdefault("relationships", [])
        if rel not in rels:
            rels.append(rel)
    if note:
        notes = comp.setdefault("notes", [])
        if note not in notes:
            notes.append(note)
    comp["updated_at"] = ts
    add_event(reg, "upsert-companion", {"id": comp_id, "name": name, "kind": kind, "channels": channels or []})
    return comp_id


def bind_channel(reg: Dict[str, Any], channel: str, persona: str, note: Optional[str] = None) -> None:
    channel_id = slug(channel)
    persona_id = slug(persona)
    reg.setdefault("channel_bindings", {})[channel_id] = persona_id
    if persona_id in reg.setdefault("companions", {}):
        comp = reg["companions"][persona_id]
        if channel_id not in comp.setdefault("channels", []):
            comp["channels"].append(channel_id)
        comp["updated_at"] = now_iso()
    add_event(reg, "bind-channel", {"channel": channel_id, "persona": persona_id, "note": note})


def save(reg: Dict[str, Any]) -> None:
    reg["updated_at"] = now_iso()
    write_json(registry_path(), reg)
    render_context(reg)


def render_context(reg: Dict[str, Any]) -> None:
    primary = reg.get("primary_persona", {})
    user = reg.get("user", {})
    lines: List[str] = []
    lines.append("<!-- makakoo:persona-context generated by persona-capture; edit via skill-system-persona-capture -->")
    lines.append("## Dynamic persona registry")
    lines.append("")
    lines.append("Read this when the user asks about names/identity, sets names, or a CLI/model exposes raw harness identity.")
    lines.append("")
    lines.append(f"- Primary persona: {primary.get('name', 'Harvey')} (`config/persona.json`, primary only).")
    user_bits = [str(user.get("name") or "Sebastian")]
    if user.get("full_name") and user.get("full_name") != user.get("name"):
        user_bits.append(str(user["full_name"]))
    lines.append(f"- User: {' / '.join(user_bits)}.")
    lines.append("- Primary rename rule: do not rename the primary persona unless Sebastian explicitly says primary/Harvey should be renamed.")
    lines.append("")
    companions = reg.get("companions", {}) or {}
    if companions:
        lines.append("### Companions / channel identities")
        for comp_id in sorted(companions):
            comp = companions[comp_id]
            roles = "; ".join(comp.get("roles", [])[:2]) or comp.get("kind", "companion")
            channels = ", ".join(comp.get("channels", [])[:6]) or "none"
            rel_labels = [r.get("label", "") for r in comp.get("relationships", []) if isinstance(r, dict)]
            rel = f" Relationships: {'; '.join(rel_labels[:2])}." if rel_labels else ""
            lines.append(f"- {comp.get('name', comp_id)} ({comp.get('kind', 'companion')}): {roles}. Channels: {channels}.{rel}")
        lines.append("")
    bindings = reg.get("channel_bindings", {}) or {}
    if bindings:
        lines.append("### Channel bindings")
        for channel in sorted(bindings):
            persona_id = bindings[channel]
            if persona_id == primary.get("id"):
                display = primary.get("name") or persona_id
            else:
                display = companions.get(persona_id, {}).get("name") or persona_id
            lines.append(f"- `{channel}` -> {display}")
        lines.append("")
    lines.append("### Agent rule")
    lines.append("- If active channel matches a binding, answer identity as that bound persona.")
    lines.append("- Otherwise answer as the primary persona. On Sebastian's install: Harvey.")
    lines.append("- For new name setup, run `makakoo skill skill-system-persona-capture capture --text <verbatim> --channel <channel>`; legacy fallback: `harvey run persona-capture ...`. Do not write ad-hoc WHO.md files.")
    lines.append("")
    atomic_write(context_path(), "\n".join(lines).rstrip() + "\n")


def regex_first(patterns: List[str], text: str) -> Optional[str]:
    for pat in patterns:
        m = re.search(pat, text, flags=re.I)
        if m:
            return clean_name(m.group(1))
    return None


def capture(reg: Dict[str, Any], text: str, channel: Optional[str], source: Optional[str], primary: bool = False) -> List[str]:
    changes: List[str] = []
    raw = text.strip()
    user_name = regex_first([
        r"\bmy name is\s+([A-Z][A-Za-z0-9 ._'-]{1,80})",
        r"\bcall me\s+([A-Z][A-Za-z0-9 ._'-]{1,80})",
    ], raw)
    if user_name:
        upsert_user(reg, user_name, full_name=user_name if " " in user_name else None, note=f"captured from {source or channel or 'conversation'}")
        changes.append(f"user={user_name}")

    assistant_name = regex_first([
        r"\byour name is\s+([A-Z][A-Za-z0-9 ._'-]{1,80})",
        r"\bcall (?:yourself|this agent|this bot|this cli)\s+([A-Z][A-Za-z0-9 ._'-]{1,80})",
        r"\bthis (?:agent|bot|cli|companion) is\s+([A-Z][A-Za-z0-9 ._'-]{1,80})",
    ], raw)
    if assistant_name:
        if primary:
            raise SystemExit("Refusing implicit primary rename from capture. Use set-primary --yes-really for primary persona changes.")
        channels = [channel] if channel else []
        roles = [f"channel companion captured from {source or channel or 'conversation'}"]
        rels: List[str] = []
        brother = regex_first([r"\b(?:your|her|his) brother\s+([A-Z][A-Za-z0-9._'-]{1,80})"], raw)
        if brother:
            rels.append(f"sibling:{brother}:{brother} is {assistant_name}'s brother")
        comp_id = upsert_companion(reg, assistant_name, kind="channel-companion", roles=roles, channels=channels, relationships=rels, note=raw)
        changes.append(f"companion={assistant_name}")
        if channel:
            bind_channel(reg, channel, comp_id, note="captured from name setup")
            changes.append(f"binding={slug(channel)}->{comp_id}")

    # Relationship-only follow-up, e.g. "Harvey is Donna's brother".
    rel_match = re.search(r"\b([A-Z][A-Za-z0-9 ._'-]{1,80})\s+is\s+([A-Z][A-Za-z0-9 ._'-]{1,80})'s\s+(brother|sister|sibling)\b", raw, flags=re.I)
    if rel_match:
        target_name = clean_name(rel_match.group(1))
        owner_name = clean_name(rel_match.group(2))
        rel_kind = rel_match.group(3).lower()
        comp_id = slug(owner_name)
        if comp_id in reg.get("companions", {}):
            upsert_companion(reg, owner_name, relationships=[f"{rel_kind}:{target_name}:{target_name} is {owner_name}'s {rel_kind}"])
            changes.append(f"relationship={owner_name}->{target_name}")

    if not changes:
        add_event(reg, "capture-noop", {"text": raw, "channel": channel, "source": source})
    return changes


def cmd_show(args: argparse.Namespace) -> int:
    reg = load_registry()
    if args.json:
        print(json.dumps(reg, indent=2, ensure_ascii=False, sort_keys=True))
        return 0
    primary = reg.get("primary_persona", {}).get("name", "Harvey")
    user = reg.get("user", {}).get("name", "Sebastian")
    print(f"Primary: {primary}")
    print(f"User: {user}")
    print("Companions:")
    for comp in sorted((reg.get("companions") or {}).values(), key=lambda c: c.get("name", "")):
        print(f"  - {comp.get('name')} [{comp.get('kind')}] channels={','.join(comp.get('channels', [])) or '-'}")
    print("Channel bindings:")
    for k, v in sorted((reg.get("channel_bindings") or {}).items()):
        print(f"  - {k} -> {v}")
    print(f"Registry: {registry_path()}")
    print(f"Context:  {context_path()}")
    return 0


def cmd_render(args: argparse.Namespace) -> int:
    reg = load_registry()
    save(reg)
    print(f"Rendered {context_path()}")
    return 0


def cmd_set_user(args: argparse.Namespace) -> int:
    reg = load_registry()
    upsert_user(reg, args.name, args.full_name, args.note)
    save(reg)
    print(f"Saved user name: {clean_name(args.name)}")
    print(f"Rendered {context_path()}")
    return 0


def cmd_add_companion(args: argparse.Namespace) -> int:
    reg = load_registry()
    comp_id = upsert_companion(reg, args.name, args.kind, args.role or [], args.channel or [], args.relationship or [], args.note)
    save(reg)
    print(f"Saved companion: {clean_name(args.name)} ({comp_id})")
    print(f"Rendered {context_path()}")
    return 0


def cmd_bind_channel(args: argparse.Namespace) -> int:
    reg = load_registry()
    bind_channel(reg, args.channel, args.persona, args.note)
    save(reg)
    print(f"Bound {slug(args.channel)} -> {slug(args.persona)}")
    print(f"Rendered {context_path()}")
    return 0


def cmd_capture(args: argparse.Namespace) -> int:
    reg = load_registry()
    changes = capture(reg, args.text, args.channel, args.source, args.primary)
    save(reg)
    if changes:
        print("Captured: " + ", ".join(changes))
    else:
        print("No persona/name pattern captured; event recorded only.")
    print(f"Registry: {registry_path()}")
    print(f"Context:  {context_path()}")
    return 0


def cmd_set_primary(args: argparse.Namespace) -> int:
    if args.yes_really != "yes-really":
        raise SystemExit("Refusing primary rename without --yes-really yes-really")
    path = persona_path()
    data = load_json(path, {})
    data.update({"version": data.get("version", 1), "name": clean_name(args.name), "home": str(home())})
    if args.user:
        data["user"] = clean_name(args.user)
    write_json(path, data)
    reg = load_registry()
    reg["primary_persona"] = {k: v for k, v in load_primary_persona().items() if k != "user_from_persona_json"}
    add_event(reg, "set-primary", {"name": clean_name(args.name)})
    save(reg)
    print(f"Primary persona renamed to {clean_name(args.name)}")
    print(f"Updated {path}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Makakoo OS persona registry capture")
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("show", help="show registry summary")
    s.add_argument("--json", action="store_true")
    s.set_defaults(func=cmd_show)

    s = sub.add_parser("render", help="regenerate persona_context.md")
    s.set_defaults(func=cmd_render)

    s = sub.add_parser("set-user", help="set preferred user name")
    s.add_argument("--name", required=True)
    s.add_argument("--full-name")
    s.add_argument("--note")
    s.set_defaults(func=cmd_set_user)

    s = sub.add_parser("add-companion", help="add/update companion/channel identity")
    s.add_argument("--name", required=True)
    s.add_argument("--kind", default="channel-companion")
    s.add_argument("--role", action="append")
    s.add_argument("--channel", action="append")
    s.add_argument("--relationship", action="append", help="kind:target:label")
    s.add_argument("--note")
    s.set_defaults(func=cmd_add_companion)

    s = sub.add_parser("bind-channel", help="bind host/channel to persona id/name")
    s.add_argument("--channel", required=True)
    s.add_argument("--persona", required=True)
    s.add_argument("--note")
    s.set_defaults(func=cmd_bind_channel)

    s = sub.add_parser("capture", help="parse one user sentence and persist name setup")
    s.add_argument("--text", required=True)
    s.add_argument("--channel")
    s.add_argument("--source")
    s.add_argument("--primary", action="store_true", help="reserved; capture refuses primary rename even when set")
    s.set_defaults(func=cmd_capture)

    s = sub.add_parser("set-primary", help="explicitly rename primary persona.json")
    s.add_argument("--name", required=True)
    s.add_argument("--user")
    s.add_argument("--yes-really", required=True)
    s.set_defaults(func=cmd_set_primary)
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
