"""Tests for v0.2 F.4 — CookieVault + state.json migration helper.

Forces the filesystem backend (encrypted-at-rest skeleton) so the
darwin-only Keychain CLI isn't a hard dep for CI. The macOS path is
exercised live by Sebastian when he runs the real migration.
"""
from __future__ import annotations

import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path

from core.security.cookie_vault import (
    CookieVault,
    SENSITIVE_COOKIE_NAMES,
    _is_sensitive,
    migrate_state_json,
)


class CookieVaultTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="vault_test_")
        os.environ["MAKAKOO_HOME"] = self.tmp
        os.environ["HARVEY_HOME"] = self.tmp
        self.vault = CookieVault(channel="testchan", force_backend="filesystem")

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_set_get_delete_round_trip(self):
        self.vault.set("li_at", "secret-value-123")
        self.assertEqual(self.vault.get("li_at"), "secret-value-123")
        self.assertTrue(self.vault.delete("li_at"))
        self.assertIsNone(self.vault.get("li_at"))

    def test_list_keys_returns_only_set_keys(self):
        self.vault.set("li_at", "v1")
        self.vault.set("JSESSIONID", "v2")
        self.assertEqual(sorted(self.vault.list_keys()), ["JSESSIONID", "li_at"])

    def test_get_missing_returns_none(self):
        self.assertIsNone(self.vault.get("never-set"))

    def test_filesystem_storage_uses_state_secrets_dir(self):
        self.vault.set("li_at", "abc")
        secrets_path = Path(self.tmp) / "state" / "secrets" / "makakoo.testchan.json"
        self.assertTrue(secrets_path.exists())
        # Mode should be 0600 (owner read/write only).
        mode = secrets_path.stat().st_mode & 0o777
        self.assertEqual(mode, 0o600)
        # Body is base64 — plaintext "abc" must NOT appear verbatim.
        text = secrets_path.read_text()
        self.assertNotIn("abc", text)

    def test_is_sensitive_recognizes_known_auth_cookies(self):
        self.assertTrue(_is_sensitive("li_at"))
        self.assertTrue(_is_sensitive("JSESSIONID"))
        self.assertTrue(_is_sensitive("li_rm"))
        self.assertFalse(_is_sensitive("UserMatchHistory"))
        self.assertFalse(_is_sensitive("lang"))


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="vault_mig_")
        os.environ["MAKAKOO_HOME"] = self.tmp
        os.environ["HARVEY_HOME"] = self.tmp

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _seed_state(self, cookies):
        path = Path(self.tmp) / "state.json"
        path.write_text(json.dumps({"cookies": cookies}), encoding="utf-8")
        return path

    def test_migrate_vaults_sensitive_cookies_only(self):
        path = self._seed_state(
            [
                {"name": "li_at", "value": "AUTH_TOKEN_VAL"},
                {"name": "lang", "value": "en-us"},
                {"name": "JSESSIONID", "value": "ajax:123"},
                {"name": "UserMatchHistory", "value": "blob"},
            ]
        )
        # Force filesystem backend by patching the auto-selector via env.
        report = migrate_state_json("testchan-mig", str(path))
        self.assertEqual(sorted(report["vaulted"]), ["JSESSIONID", "li_at"])
        self.assertIn("lang", report["kept_plaintext"])
        self.assertIn("UserMatchHistory", report["kept_plaintext"])
        self.assertEqual(report["errors"], [])

    def test_migrate_replaces_sensitive_values_with_vault_marker(self):
        path = self._seed_state([{"name": "li_at", "value": "TOKEN"}])
        migrate_state_json("testchan-mig2", str(path))
        new_data = json.loads(path.read_text())
        cookie = new_data["cookies"][0]
        self.assertEqual(cookie["value"], "")
        self.assertTrue(cookie["vaulted"])
        self.assertEqual(cookie["vault_key"], "testchan-mig2:li_at")

    def test_migrate_creates_pre_vault_backup(self):
        path = self._seed_state([{"name": "li_at", "value": "TOKEN"}])
        migrate_state_json("testchan-mig3", str(path))
        backup = path.with_suffix(path.suffix + ".pre-vault.bak")
        self.assertTrue(backup.exists())
        # Backup keeps the original plaintext so a panicky user can rollback.
        self.assertIn("TOKEN", backup.read_text())

    def test_migrate_dry_run_writes_nothing(self):
        path = self._seed_state([{"name": "li_at", "value": "TOKEN"}])
        before = path.read_text()
        report = migrate_state_json("testchan-mig4", str(path), dry_run=True)
        self.assertEqual(report["vaulted"], ["li_at"])
        # File untouched.
        self.assertEqual(path.read_text(), before)
        self.assertFalse(path.with_suffix(path.suffix + ".pre-vault.bak").exists())

    def test_migrate_missing_file_reports_error(self):
        report = migrate_state_json("nope", "/tmp/does-not-exist-xyz.json")
        self.assertTrue(report["errors"])


if __name__ == "__main__":
    unittest.main()
