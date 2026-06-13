#!/usr/bin/env python3
"""Makakoo Brain Network control plane.

Thin, safe wrapper around Octopus signed-MCP primitives:
- activation/deactivation of the `agent-octopus-peer` listener
- persistent peer endpoint registry
- signed remote read calls (`brain_search`) with origin metadata

No remote writes in v1.
"""
from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any, Iterable

try:
    from core.octopus import identity, trust_store  # type: ignore
    from core.octopus.discovery.invite import decode_invite  # type: ignore
except Exception:  # pragma: no cover - reported by doctor/commands
    identity = None  # type: ignore
    trust_store = None  # type: ignore
    decode_invite = None  # type: ignore

VERSION = 1
DEFAULT_PORT = 8765
SAFE_HOST_RE = re.compile(r"^[A-Za-z0-9_.:-]+$")


def now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def home() -> Path:
    for key in ("MAKAKOO_HOME", "HARVEY_HOME"):
        val = os.environ.get(key)
        if val:
            return Path(val).expanduser().resolve()
    return (Path.home() / "MAKAKOO").resolve()


def config_dir() -> Path:
    d = home() / "config" / "brain-network"
    d.mkdir(parents=True, exist_ok=True)
    return d


def state_dir() -> Path:
    d = home() / "state" / "skill-brain-network"
    d.mkdir(parents=True, exist_ok=True)
    return d


def config_path() -> Path:
    return config_dir() / "config.json"


def peers_path() -> Path:
    return config_dir() / "peers.json"


def env_path() -> Path:
    return config_dir() / "octopus-peer.env"


def audit_path() -> Path:
    return state_dir() / "audit.jsonl"


def load_json(path: Path, default: Any) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return default


def atomic_write(path: Path, text: str, mode: int | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(text, encoding="utf-8")
    if mode is not None:
        try:
            os.chmod(tmp, mode)
        except OSError:
            pass
    tmp.replace(path)


def write_json(path: Path, data: Any, mode: int | None = None) -> None:
    atomic_write(path, json.dumps(data, indent=2, sort_keys=True) + "\n", mode=mode)


def audit(event: str, **payload: Any) -> None:
    row = {"at": now_iso(), "event": event, **payload}
    audit_path().parent.mkdir(parents=True, exist_ok=True)
    with audit_path().open("a", encoding="utf-8") as f:
        f.write(json.dumps(row, sort_keys=True) + "\n")


def makakoo_bin() -> str:
    found = shutil.which("makakoo")
    if found:
        return found
    candidate = Path.home() / ".local" / "bin" / "makakoo"
    if candidate.exists():
        return str(candidate)
    return "makakoo"


def run_makakoo(args: list[str], *, capture: bool = False, check: bool = False) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["MAKAKOO_HOME"] = str(home())
    env["PATH"] = f"{Path.home() / '.local' / 'bin'}:{env.get('PATH', '')}"
    proc = subprocess.run(
        [makakoo_bin(), *args],
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.STDOUT if capture else None,
        env=env,
        check=False,
    )
    if check and proc.returncode != 0:
        out = proc.stdout or ""
        raise RuntimeError(f"makakoo {' '.join(args)} failed ({proc.returncode})\n{out}")
    return proc


def load_config() -> dict[str, Any]:
    data = load_json(config_path(), {})
    if not isinstance(data, dict):
        data = {}
    data.setdefault("version", VERSION)
    data.setdefault("enabled", False)
    data.setdefault("bind_mode", "loopback")
    data.setdefault("bind_host", "127.0.0.1")
    data.setdefault("port", DEFAULT_PORT)
    return data


def save_config(data: dict[str, Any]) -> None:
    data["version"] = VERSION
    data["updated_at"] = now_iso()
    write_json(config_path(), data)


def load_peers() -> dict[str, Any]:
    data = load_json(peers_path(), {"version": VERSION, "peers": {}})
    if not isinstance(data, dict):
        data = {"version": VERSION, "peers": {}}
    data.setdefault("version", VERSION)
    data.setdefault("peers", {})
    if not isinstance(data["peers"], dict):
        data["peers"] = {}
    return data


def save_peers(data: dict[str, Any]) -> None:
    data["version"] = VERSION
    data["updated_at"] = now_iso()
    write_json(peers_path(), data)


def shell_quote_env(value: str) -> str:
    return "'" + value.replace("'", "'\\''") + "'"


def write_octopus_env(bind_host: str, port: int) -> None:
    body = "\n".join([
        "# generated by makakoo network activate",
        f"MAKAKOO_MCP_HTTP_BIND={shell_quote_env(bind_host)}",
        f"MAKAKOO_MCP_HTTP_PORT={shell_quote_env(str(port))}",
        "",
    ])
    atomic_write(env_path(), body, mode=0o600)


def tailscale_ip() -> str | None:
    exe = shutil.which("tailscale")
    if not exe:
        return None
    proc = subprocess.run([exe, "ip", "-4"], text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    if proc.returncode != 0:
        return None
    for line in proc.stdout.splitlines():
        val = line.strip()
        if val:
            return val
    return None


def resolve_bind(spec: str, *, yes: bool) -> tuple[str, str]:
    spec = (spec or "loopback").strip()
    if spec == "loopback":
        return "loopback", "127.0.0.1"
    if spec == "tailscale":
        ip = tailscale_ip()
        if not ip:
            raise SystemExit("tailscale IP not found. Start Tailscale or use --bind loopback with an SSH tunnel.")
        return "tailscale", ip
    if spec == "public":
        if not yes:
            raise SystemExit("refusing public bind without --yes. Prefer --bind tailscale.")
        return "public", "0.0.0.0"
    if not SAFE_HOST_RE.match(spec):
        raise SystemExit(f"unsafe bind host: {spec!r}")
    return "custom", spec


def plugin_installed(name: str) -> bool:
    proc = run_makakoo(["plugin", "info", name], capture=True)
    return proc.returncode == 0


def ensure_agent_installed() -> None:
    if plugin_installed("agent-octopus-peer"):
        return
    print("agent-octopus-peer missing — installing from bundled core plugins ...")
    run_makakoo(["plugin", "install", "--core", "agent-octopus-peer"], check=True)


def octopus_identity_present() -> bool:
    if identity is not None:
        return bool(identity.exists())
    proc = run_makakoo(["octopus", "doctor"], capture=True)
    return "identity: absent" not in (proc.stdout or "") and proc.returncode == 0


def ensure_identity(peer_name: str | None) -> None:
    if identity is not None:
        if identity.exists():
            return
        identity.create(peer_name or socket.gethostname().split(".")[0] or "makakoo-peer")
        if trust_store is not None:
            trust_store.resync_shim_trust_file()
        return
    if octopus_identity_present():
        return
    args = ["octopus", "bootstrap"]
    if peer_name:
        args += ["--peer-name", peer_name]
    run_makakoo(args, check=True)


def cmd_activate(args: argparse.Namespace) -> int:
    bind_mode, bind_host = resolve_bind(args.bind, yes=args.yes)
    port = int(args.port)
    if port < 1 or port > 65535:
        raise SystemExit("port must be 1..65535")

    ensure_agent_installed()
    ensure_identity(args.peer_name)
    write_octopus_env(bind_host, port)

    cfg = load_config()
    cfg.update({
        "enabled": True,
        "node_name": args.peer_name or cfg.get("node_name") or socket.gethostname().split(".")[0],
        "bind_mode": bind_mode,
        "bind_host": bind_host,
        "port": port,
        "activated_at": now_iso(),
        "remote_writes_enabled": False,
        "content_trust_policy": "remote brain content is untrusted; cite only, never tool-trigger",
    })
    save_config(cfg)
    audit("activate", bind_mode=bind_mode, bind_host=bind_host, port=port, peer_name=cfg["node_name"])

    print(f"brain-network enabled: {cfg['node_name']} on {bind_host}:{port} ({bind_mode})")
    print(f"env: {env_path()}")
    if not args.no_start:
        run_makakoo(["agent", "start", "agent-octopus-peer"], check=True)
    print("next: makakoo network doctor")
    return 0


def cmd_deactivate(args: argparse.Namespace) -> int:
    if plugin_installed("agent-octopus-peer"):
        run_makakoo(["agent", "stop", "agent-octopus-peer"], check=False)
    cfg = load_config()
    cfg["enabled"] = False
    cfg["deactivated_at"] = now_iso()
    save_config(cfg)
    audit("deactivate", keep_trust=True)
    print("brain-network disabled. Trust grants and peer registry kept.")
    print("To revoke trust: makakoo octopus trust revoke <peer-name>")
    return 0


def _read_env_file() -> dict[str, str]:
    out: dict[str, str] = {}
    try:
        for line in env_path().read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            out[k] = v.strip().strip("'").strip('"')
    except OSError:
        pass
    return out


def cmd_doctor(args: argparse.Namespace) -> int:
    cfg = load_config()
    env = _read_env_file()
    checks: list[dict[str, Any]] = []

    def add(name: str, status: str, detail: str) -> None:
        checks.append({"name": name, "status": status, "detail": detail})
        if not args.json:
            print(f"{status:<5} {name}: {detail}")

    add("config", "OK" if config_path().exists() else "WARN", f"enabled={cfg.get('enabled')} path={config_path()}")
    if env_path().exists():
        bind = env.get("MAKAKOO_MCP_HTTP_BIND", "?")
        status = "WARN" if bind == "0.0.0.0" else "OK"
        add("octopus env", status, f"bind={bind} port={env.get('MAKAKOO_MCP_HTTP_PORT', '?')} path={env_path()}")
    else:
        add("octopus env", "WARN", f"missing; activate writes {env_path()}")

    add("agent plugin", "OK" if plugin_installed("agent-octopus-peer") else "WARN", "agent-octopus-peer installed" if plugin_installed("agent-octopus-peer") else "install with: makakoo plugin install --core agent-octopus-peer")

    octo = run_makakoo(["octopus", "doctor"], capture=True)
    octo_out = (octo.stdout or "").strip()
    add("octopus", "OK" if octo.returncode == 0 else "WARN", octo_out.replace("\n", " | ") or f"exit={octo.returncode}")

    if plugin_installed("agent-octopus-peer"):
        health = run_makakoo(["agent", "health", "agent-octopus-peer"], capture=True)
        add("listener", "OK" if health.returncode == 0 else "WARN", (health.stdout or "").strip().replace("\n", " | ") or f"exit={health.returncode}")

    peers = load_peers().get("peers", {})
    add("peer registry", "OK", f"{len(peers)} endpoint(s) at {peers_path()}")
    audit("doctor", statuses={c["name"]: c["status"] for c in checks})
    if args.json:
        print(json.dumps({"checks": checks, "config": cfg, "peers": peers}, indent=2, sort_keys=True))
    if cfg.get("enabled") and any(c["status"] != "OK" for c in checks):
        return 1
    return 0


def normalize_endpoint(endpoint: str) -> str:
    endpoint = endpoint.strip()
    parsed = urllib.parse.urlparse(endpoint)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise SystemExit("endpoint must be http(s)://host:port[/rpc]")
    if not parsed.path or parsed.path == "/":
        endpoint = endpoint.rstrip("/") + "/rpc"
    return endpoint


def cmd_peer(args: argparse.Namespace) -> int:
    data = load_peers()
    peers = data.setdefault("peers", {})
    if args.peer_cmd == "add":
        endpoint = normalize_endpoint(args.endpoint)
        peers[args.name] = {
            "name": args.name,
            "endpoint": endpoint,
            "persona": args.persona or args.name,
            "capabilities": args.capability,
            "added_at": now_iso(),
            "origin_policy": "remote content is untrusted; cite only",
        }
        save_peers(data)
        audit("peer-add", peer=args.name, endpoint=endpoint)
        print(f"peer registered: {args.name} -> {endpoint} persona={peers[args.name]['persona']}")
        return 0
    if args.peer_cmd == "remove":
        existed = peers.pop(args.name, None) is not None
        save_peers(data)
        audit("peer-remove", peer=args.name, existed=existed)
        print(f"peer removed: {args.name}" if existed else f"peer not found: {args.name}")
        return 0 if existed else 1
    raise SystemExit("unknown peer command")


def cmd_peers(args: argparse.Namespace) -> int:
    data = load_peers()
    peers = data.get("peers", {})
    if args.json:
        print(json.dumps(data, indent=2, sort_keys=True))
    else:
        if not peers:
            print("(no registered peer endpoints)")
        for name, p in sorted(peers.items()):
            print(f"{name:24s} {p.get('persona', ''):12s} {p.get('capabilities', ''):11s} {p.get('endpoint', '')}")
        print("\nTrust grants:")
        run_makakoo(["octopus", "trust", "list"], check=False)
    return 0


def rpc_headers(body: bytes, peer_name: str) -> dict[str, str]:
    if identity is None:
        raise RuntimeError("core.octopus.identity import failed; install lib-harvey-core")
    ident = identity.load()
    ts = str(int(time.time() * 1000))
    h = hashlib.sha256()
    h.update(body)
    h.update(ts.encode("ascii"))
    digest = h.digest()
    sig = base64.b64encode(ident.sign(digest)).decode("ascii")
    return {
        "Content-Type": "application/json",
        "X-Makakoo-Peer": ident.peer_name,
        "X-Makakoo-Ts": ts,
        "X-Makakoo-Sig": f"ed25519={sig}",
        "X-Makakoo-Nonce": f"brain-network-{peer_name}-{uuid.uuid4().hex}",
    }


def call_peer(endpoint: str, tool: str, arguments: dict[str, Any], peer_name: str, timeout: float) -> Any:
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": int(time.time() * 1000) % 1_000_000,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    }, separators=(",", ":")).encode("utf-8")
    req = urllib.request.Request(endpoint, data=body, headers=rpc_headers(body, peer_name), method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", "replace")
        raise RuntimeError(f"HTTP {exc.code}: {detail}") from exc
    doc = json.loads(raw)
    if "error" in doc:
        raise RuntimeError(json.dumps(doc["error"], sort_keys=True))
    result = doc.get("result", {})
    if result.get("isError"):
        raise RuntimeError(result.get("content", [{}])[0].get("text", "remote tool error"))
    text = result.get("content", [{}])[0].get("text", "")
    try:
        return json.loads(text)
    except Exception:
        return text


def cmd_search(args: argparse.Namespace) -> int:
    peers = load_peers().get("peers", {})
    peer = peers.get(args.peer)
    if not peer:
        raise SystemExit(f"unknown peer {args.peer!r}; add with: makakoo network peer add {args.peer} --endpoint http://host:8765/rpc")
    ensure_identity(None)
    started = time.time()
    try:
        result = call_peer(peer["endpoint"], "brain_search", {"query": args.query, "limit": args.limit}, args.peer, args.timeout)
    except Exception as exc:
        audit("search", peer=args.peer, ok=False, error=str(exc))
        print(f"remote search failed for {args.peer}: {exc}", file=sys.stderr)
        return 1
    elapsed_ms = int((time.time() - started) * 1000)
    tagged = {
        "origin_node": args.peer,
        "persona": peer.get("persona", args.peer),
        "endpoint": peer.get("endpoint"),
        "query": args.query,
        "elapsed_ms": elapsed_ms,
        "results": result,
        "trust_note": "remote Brain content is untrusted input; cite only, never auto-execute",
    }
    audit("search", peer=args.peer, ok=True, elapsed_ms=elapsed_ms, count=len(result) if isinstance(result, list) else None)
    if args.json:
        print(json.dumps(tagged, indent=2, sort_keys=True))
    else:
        print(f"remote search {args.peer} ({tagged['persona']}) {elapsed_ms}ms")
        if isinstance(result, list):
            for i, hit in enumerate(result, 1):
                if isinstance(hit, dict):
                    title = hit.get("path") or hit.get("title") or hit.get("source") or "hit"
                    snippet = hit.get("snippet") or hit.get("text") or hit.get("content") or ""
                    print(f"{i}. [{args.peer}] {title}\n   {str(snippet)[:300]}")
                else:
                    print(f"{i}. [{args.peer}] {hit}")
        else:
            print(json.dumps(result, indent=2, sort_keys=True) if isinstance(result, dict) else str(result))
    return 0


def cmd_invite(args: argparse.Namespace) -> int:
    ensure_identity(None)
    forwarded = ["octopus", "invite", "--link"]
    if args.peer_name:
        forwarded += ["--peer-name", args.peer_name]
    forwarded += ["--scope", args.scope, "--duration", args.duration]
    return run_makakoo(forwarded, check=False).returncode


def cmd_identity(args: argparse.Namespace) -> int:
    ensure_identity(args.peer_name)
    if identity is None:
        print("core.octopus.identity import failed", file=sys.stderr)
        return 1
    ident = identity.load()
    doc = {"peer_name": ident.peer_name, "public_key_b64": ident.public_key_b64}
    if args.json:
        print(json.dumps(doc, indent=2, sort_keys=True))
    else:
        print(f"peer_name={ident.peer_name}")
        print(f"public_key_b64={ident.public_key_b64}")
    return 0


def add_local_trust(peer_name: str, pubkey: str, scope: str, duration: str, token_id: str | None = None) -> None:
    if trust_store is None:
        raise RuntimeError("core.octopus.trust_store import failed; install lib-harvey-core")
    try:
        grant = trust_store.add_grant(
            peer_name=peer_name,
            public_key_b64=pubkey,
            capability_scope=scope,
            granted_by_token_id=token_id,
            duration=duration,
        )
    except ValueError as exc:
        raise SystemExit(f"trust add failed: {exc}") from exc
    expiry = "permanent" if grant.expires_at_unix is None else dt.datetime.fromtimestamp(grant.expires_at_unix).isoformat(timespec="seconds")
    audit("trust-add", peer=peer_name, scope=scope, duration=duration, expires=expiry)
    print(f"trust grant created: {peer_name} scope={scope} expires={expiry}")


def cmd_trust(args: argparse.Namespace) -> int:
    if args.trust_cmd == "add":
        add_local_trust(args.name, args.pubkey, args.scope, args.duration)
        return 0
    raise SystemExit("unknown trust command")


def cmd_join(args: argparse.Namespace) -> int:
    ensure_identity(None)
    if decode_invite is None:
        print("core.octopus.discovery.invite import failed", file=sys.stderr)
        return 1
    payload = decode_invite(args.link)
    if payload.is_expired():
        print("invite link expired", file=sys.stderr)
        return 2
    peer_name = args.peer_name or payload.issuer_peer_name
    add_local_trust(
        peer_name=peer_name,
        pubkey=payload.issuer_public_key_b64,
        scope=payload.capability_scope,
        duration=payload.duration_default,
        token_id=payload.token_id,
    )
    print("note: this creates local trust for the invite issuer. Repeat in the opposite direction for bidirectional reads.")
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="makakoo network", description="Opt-in Makakoo Brain network control plane")
    sub = p.add_subparsers(dest="cmd", required=True)

    p_doc = sub.add_parser("doctor", help="read-only health check")
    p_doc.add_argument("--json", action="store_true")
    p_doc.set_defaults(func=cmd_doctor)
    p_status = sub.add_parser("status", help="alias for doctor")
    p_status.add_argument("--json", action="store_true")
    p_status.set_defaults(func=cmd_doctor)

    p_id = sub.add_parser("identity", help="print this node's Octopus peer name and public key")
    p_id.add_argument("--peer-name", help="bootstrap with this name if identity is absent")
    p_id.add_argument("--json", action="store_true")
    p_id.set_defaults(func=cmd_identity)

    p_act = sub.add_parser("activate", help="install/bootstrap/start the Octopus peer listener safely")
    p_act.add_argument("--peer-name", help="stable node name, e.g. sebastian-mbp or donna-vps")
    p_act.add_argument("--bind", default="loopback", help="loopback|tailscale|public|explicit-host (default: loopback)")
    p_act.add_argument("--port", type=int, default=DEFAULT_PORT)
    p_act.add_argument("--no-start", action="store_true", help="write config/env but do not start the agent")
    p_act.add_argument("--yes", action="store_true", help="required for --bind public")
    p_act.set_defaults(func=cmd_activate)

    p_de = sub.add_parser("deactivate", help="stop listener and mark network disabled; keep trust/cache")
    p_de.set_defaults(func=cmd_deactivate)

    p_peers = sub.add_parser("peers", help="list registered peer endpoints and Octopus trust grants")
    p_peers.add_argument("--json", action="store_true")
    p_peers.set_defaults(func=cmd_peers)

    p_trust = sub.add_parser("trust", help="manual local trust grant management")
    trust_sub = p_trust.add_subparsers(dest="trust_cmd", required=True)
    p_tadd = trust_sub.add_parser("add", help="trust a peer public key on this host")
    p_tadd.add_argument("name")
    p_tadd.add_argument("--pubkey", required=True, help="base64 32-byte Ed25519 public key")
    p_tadd.add_argument("--scope", default="read-brain", choices=["read-brain", "write-brain", "full-brain"])
    p_tadd.add_argument("--duration", default="24h")
    p_tadd.set_defaults(func=cmd_trust)

    p_peer = sub.add_parser("peer", help="manage local endpoint registry")
    peer_sub = p_peer.add_subparsers(dest="peer_cmd", required=True)
    p_add = peer_sub.add_parser("add", help="register a peer endpoint for remote read calls")
    p_add.add_argument("name")
    p_add.add_argument("--endpoint", required=True, help="http(s)://host:8765[/rpc]")
    p_add.add_argument("--persona", help="human/persona label, e.g. Donna")
    p_add.add_argument("--capability", default="read-brain", choices=["read-brain", "write-brain", "full-brain"])
    p_add.set_defaults(func=cmd_peer)
    p_rm = peer_sub.add_parser("remove", help="remove a peer endpoint registration")
    p_rm.add_argument("name")
    p_rm.set_defaults(func=cmd_peer)

    p_search = sub.add_parser("search", help="signed remote brain_search against one registered peer")
    p_search.add_argument("peer")
    p_search.add_argument("query")
    p_search.add_argument("--limit", type=int, default=10)
    p_search.add_argument("--timeout", type=float, default=8.0)
    p_search.add_argument("--json", action="store_true")
    p_search.set_defaults(func=cmd_search)

    p_inv = sub.add_parser("invite", help="mint a read-brain invite link via Octopus")
    p_inv.add_argument("--peer-name")
    p_inv.add_argument("--scope", default="read-brain", choices=["read-brain", "write-brain", "full-brain"])
    p_inv.add_argument("--duration", default="24h")
    p_inv.set_defaults(func=cmd_invite)

    p_join = sub.add_parser("join", help="accept an invite link by trusting the issuer public key locally")
    p_join.add_argument("link")
    p_join.add_argument("--peer-name", help="override recorded peer name; default is invite issuer")
    p_join.set_defaults(func=cmd_join)
    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args) or 0)


if __name__ == "__main__":
    raise SystemExit(main())
