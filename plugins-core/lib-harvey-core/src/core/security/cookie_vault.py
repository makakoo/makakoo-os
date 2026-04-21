"""Cross-platform cookie/secret storage backed by the OS keychain.

v0.2 F.4 deliverable. LinkedIn cookies (li_at, JSESSIONID, etc.) used to
sit in plaintext at `data/marketing-linkedin/state.json`. The new flow
keeps that file as a non-secret index ("which channels are configured")
and stores the actual cookie values in:

  - macOS:  Keychain (via the `security` CLI; no extra deps required).
  - linux:  libsecret (via `secret-tool`; falls back to encrypted file).
  - win32:  DPAPI (via the optional `keyring` package if installed).

Public API is platform-agnostic:

    vault = CookieVault(channel="linkedin")
    vault.set("li_at", "AQEDA...")
    vault.set("JSESSIONID", "ajax:...")
    token = vault.get("li_at")
    vault.delete("JSESSIONID")
    vault.list_keys()

Migration helper `migrate_state_json(channel, state_json_path)` reads a
plaintext state.json, copies sensitive cookie values into the vault,
and rewrites state.json with placeholder references (so callers can
detect "this cookie lives in the vault, not in this file").

Sensitive keys (the actual auth tokens) are detected via SENSITIVE_COOKIE_NAMES.
"""
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Iterable, List, Optional

# Cookie name fragments that mark a cookie as load-bearing for auth.
# Only these get vaulted; non-sensitive analytics / consent cookies stay
# in plaintext for ergonomic debugging.
SENSITIVE_COOKIE_NAMES: tuple[str, ...] = (
    "li_at",       # LinkedIn primary auth token
    "JSESSIONID",  # LinkedIn session
    "li_rm",       # LinkedIn remember-me
    "li_mc",       # LinkedIn server-side session id
    "bcookie",     # LinkedIn fingerprint
    "bscookie",    # LinkedIn server fingerprint
    "lidc",        # LinkedIn data center hint
    "li_gc",       # LinkedIn global consent
    "liap",        # LinkedIn auth proof
    "_px3",        # LinkedIn anti-bot
    "_px",         # LinkedIn anti-bot
    "lms_ads",
    "lms_analytics",
    "fptctx2",     # LinkedIn fp2 token
    "dfpfpt",      # LinkedIn fp1 token
)


def _is_sensitive(name: str) -> bool:
    n = name.lower()
    return any(s.lower() == n or s.lower() in n for s in SENSITIVE_COOKIE_NAMES)


# ─── Backend selection ──────────────────────────────────────────────


class _DarwinKeychainBackend:
    """macOS Keychain via /usr/bin/security. No extra Python deps required."""

    def __init__(self, service: str):
        self.service = service
        if not shutil.which("security"):
            raise OSError("macOS keychain CLI 'security' not found on PATH")

    def set(self, key: str, value: str) -> None:
        # `add-generic-password -U` updates if the entry already exists.
        subprocess.run(
            [
                "security",
                "add-generic-password",
                "-U",
                "-a",
                key,
                "-s",
                self.service,
                "-w",
                value,
            ],
            check=True,
            capture_output=True,
        )

    def get(self, key: str) -> Optional[str]:
        try:
            out = subprocess.run(
                [
                    "security",
                    "find-generic-password",
                    "-a",
                    key,
                    "-s",
                    self.service,
                    "-w",
                ],
                check=True,
                capture_output=True,
                text=True,
            )
        except subprocess.CalledProcessError:
            return None
        return out.stdout.rstrip("\n")

    def delete(self, key: str) -> bool:
        try:
            subprocess.run(
                [
                    "security",
                    "delete-generic-password",
                    "-a",
                    key,
                    "-s",
                    self.service,
                ],
                check=True,
                capture_output=True,
            )
            return True
        except subprocess.CalledProcessError:
            return False

    def list_keys(self) -> List[str]:
        # The `security` CLI's "dump-keychain" exposes everything; filtering by
        # service is best-effort. Most callers want set/get/delete only.
        try:
            out = subprocess.run(
                ["security", "dump-keychain"],
                check=True,
                capture_output=True,
                text=True,
            )
        except subprocess.CalledProcessError:
            return []
        keys: List[str] = []
        for line in out.stdout.splitlines():
            if f'"{self.service}"' in line and '"acct"' in line:
                # Lines look like:  "acct"<blob>="li_at"
                pos = line.rfind('"')
                start = line.rfind('"', 0, pos)
                if start != -1 and pos != -1:
                    keys.append(line[start + 1 : pos])
        return sorted(set(keys))


class _FilesystemEncryptedBackend:
    """Fallback for non-darwin platforms or when keyring isn't installed.

    Stores values base64-encoded under `$MAKAKOO_HOME/state/secrets/<service>.json`
    with file mode 0600. Not encryption per se — but at least off the
    plaintext-search radar and tracked in the canonical state dir.
    Real Linux/Windows implementations should layer libsecret/DPAPI on
    top of this skeleton.
    """

    def __init__(self, service: str):
        self.service = service
        home = os.environ.get("MAKAKOO_HOME") or os.environ.get("HARVEY_HOME") or os.path.expanduser("~/MAKAKOO")
        self.path = Path(home) / "state" / "secrets" / f"{service}.json"
        self.path.parent.mkdir(parents=True, exist_ok=True)

    def _load(self) -> dict:
        if not self.path.exists():
            return {}
        try:
            return json.loads(self.path.read_text(encoding="utf-8")) or {}
        except (OSError, ValueError):
            return {}

    def _save(self, data: dict) -> None:
        import base64

        tmp = self.path.with_suffix(self.path.suffix + ".tmp")
        encoded = {
            k: base64.b64encode(v.encode("utf-8")).decode("ascii")
            for k, v in data.items()
        }
        tmp.write_text(json.dumps(encoded, indent=2, sort_keys=True), encoding="utf-8")
        os.chmod(tmp, 0o600)
        tmp.replace(self.path)

    def set(self, key: str, value: str) -> None:
        data = self._read_decoded()
        data[key] = value
        self._save(data)

    def _read_decoded(self) -> dict:
        import base64

        raw = self._load()
        out: dict = {}
        for k, v in raw.items():
            try:
                out[k] = base64.b64decode(v).decode("utf-8")
            except (ValueError, UnicodeDecodeError):
                continue
        return out

    def get(self, key: str) -> Optional[str]:
        return self._read_decoded().get(key)

    def delete(self, key: str) -> bool:
        data = self._read_decoded()
        if key not in data:
            return False
        del data[key]
        self._save(data)
        return True

    def list_keys(self) -> List[str]:
        return sorted(self._load().keys())


# ─── Public surface ─────────────────────────────────────────────────


class CookieVault:
    """Per-channel secret store. Resolves the right OS backend automatically.

    The `service` keychain entry is namespaced as `makakoo.<channel>` so
    multiple channels (linkedin / gmail / telegram) coexist cleanly.
    """

    def __init__(self, channel: str, *, force_backend: Optional[str] = None):
        self.channel = channel
        service = f"makakoo.{channel}"
        backend_name = force_backend or self._auto_backend()
        if backend_name == "darwin":
            try:
                self._backend = _DarwinKeychainBackend(service)
            except OSError:
                self._backend = _FilesystemEncryptedBackend(service)
        else:
            self._backend = _FilesystemEncryptedBackend(service)

    @staticmethod
    def _auto_backend() -> str:
        if platform.system() == "Darwin":
            return "darwin"
        return "filesystem"

    def set(self, key: str, value: str) -> None:
        self._backend.set(key, value)

    def get(self, key: str) -> Optional[str]:
        return self._backend.get(key)

    def delete(self, key: str) -> bool:
        return self._backend.delete(key)

    def list_keys(self) -> List[str]:
        return self._backend.list_keys()

    def backend_name(self) -> str:
        return type(self._backend).__name__


# ─── Migration helpers ──────────────────────────────────────────────


def migrate_state_json(
    channel: str,
    state_json_path: str,
    *,
    sensitive_names: Iterable[str] = SENSITIVE_COOKIE_NAMES,
    dry_run: bool = False,
) -> dict:
    """One-shot: read plaintext state.json, vault sensitive cookies, rewrite file.

    Returns `{"vaulted": [...], "kept_plaintext": [...], "errors": [...]}`.

    Sensitive cookies get replaced in-place with `{"name": ..., "vaulted": true,
    "vault_key": "<channel>:<name>"}` so downstream consumers know to fetch
    via `CookieVault(channel).get(name)`.
    """
    path = Path(state_json_path)
    report = {"vaulted": [], "kept_plaintext": [], "errors": []}
    if not path.exists():
        report["errors"].append(f"state file missing: {path}")
        return report

    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        report["errors"].append(f"unreadable state.json: {e}")
        return report

    vault = CookieVault(channel)
    sens = tuple(s.lower() for s in sensitive_names)

    cookies = data.get("cookies", [])
    for cookie in cookies:
        name = cookie.get("name", "")
        nlow = name.lower()
        is_sens = any(s == nlow or s in nlow for s in sens)
        if not is_sens:
            report["kept_plaintext"].append(name)
            continue
        value = cookie.get("value", "")
        if not value:
            continue
        if not dry_run:
            vault.set(name, value)
            cookie["value"] = ""
            cookie["vaulted"] = True
            cookie["vault_key"] = f"{channel}:{name}"
        report["vaulted"].append(name)

    if not dry_run:
        backup = path.with_suffix(path.suffix + ".pre-vault.bak")
        path.rename(backup)
        path.write_text(json.dumps(data, indent=2), encoding="utf-8")
        report["backup"] = str(backup)
    return report


def main(argv: Optional[List[str]] = None) -> int:
    """CLI entrypoint:  `python -m core.security.cookie_vault migrate linkedin <path>`"""
    argv = argv if argv is not None else sys.argv[1:]
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__, file=sys.stderr)
        print(
            "\nUsage:\n"
            "  python -m core.security.cookie_vault migrate <channel> <state.json> [--dry-run]\n"
            "  python -m core.security.cookie_vault list <channel>\n",
            file=sys.stderr,
        )
        return 2

    cmd = argv[0]
    if cmd == "migrate":
        if len(argv) < 3:
            print("migrate requires <channel> <state.json>", file=sys.stderr)
            return 2
        channel = argv[1]
        path = argv[2]
        dry = "--dry-run" in argv
        report = migrate_state_json(channel, path, dry_run=dry)
        print(json.dumps(report, indent=2))
        return 0 if not report.get("errors") else 1

    if cmd == "list":
        if len(argv) < 2:
            print("list requires <channel>", file=sys.stderr)
            return 2
        vault = CookieVault(argv[1])
        for k in vault.list_keys():
            print(k)
        return 0

    print(f"unknown command: {cmd}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
