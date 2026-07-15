"""Tests for config loader / registry."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "src"))

import config as cfg  # noqa: E402


@pytest.fixture
def tmp_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    monkeypatch.delenv("HARVEY_HOME", raising=False)
    return tmp_path


def test_load_registry_returns_default_when_config_absent(tmp_home):
    registry = cfg.load_registry()
    assert "default" in registry.names()
    assert registry.default_name == "default"


def test_add_source_creates_config_if_missing(tmp_home):
    path = cfg.add_source({
        "name": "personal",
        "type": "obsidian",
        "path": str(tmp_home / "vault"),
        "writable": True,
    })
    assert path.exists()
    data = json.loads(path.read_text())
    names = {s["name"] for s in data["sources"]}
    assert "default" in names  # seeded automatically
    assert "personal" in names


def test_add_source_rejects_unknown_type_before_write(tmp_home):
    with pytest.raises(ValueError, match="unknown brain source type"):
        cfg.add_source({"name": "typo", "type": "plian", "path": "/tmp/typo"})

    assert not cfg.config_path().exists()


def test_add_source_replaces_existing_by_name(tmp_home):
    cfg.add_source({"name": "v", "type": "obsidian", "path": "/tmp/a"})
    cfg.add_source({"name": "v", "type": "obsidian", "path": "/tmp/b"})
    data = json.loads(cfg.config_path().read_text())
    v_entries = [s for s in data["sources"] if s["name"] == "v"]
    assert len(v_entries) == 1
    assert v_entries[0]["path"] == str(Path("/tmp/b").resolve(strict=False))


def test_add_source_rejects_overlapping_roots_before_write(tmp_home):
    vault = tmp_home / "vault"
    cfg.add_source({"name": "vault", "type": "obsidian", "path": str(vault)})

    with pytest.raises(ValueError, match="brain source roots overlap"):
        cfg.add_source({"name": "nested", "type": "plain", "path": str(vault / "nested")})
    with pytest.raises(ValueError, match="brain source roots overlap"):
        cfg.apply_changes([{"name": "duplicate", "type": "plain", "path": str(vault)}])

    data = json.loads(cfg.config_path().read_text())
    assert {source["name"] for source in data["sources"]} == {"default", "vault"}


def test_add_source_persists_expanded_absolute_path(tmp_home, monkeypatch):
    vault = tmp_home / "external-vault"
    monkeypatch.setenv("VAULT_ROOT", str(vault))

    cfg.add_source({"name": "vault", "type": "obsidian", "path": "$VAULT_ROOT"})

    data = json.loads(cfg.config_path().read_text())
    stored = next(source for source in data["sources"] if source["name"] == "vault")
    assert stored["path"] == str(vault.resolve(strict=False))
    assert cfg.load_registry().get("vault").root == vault.resolve(strict=False)

    with pytest.raises(ValueError, match="brain source roots overlap"):
        cfg.add_source(
            {"name": "nested", "type": "plain", "path": "$VAULT_ROOT/nested"}
        )


def test_source_path_alias_expansion_is_token_aware(tmp_home, monkeypatch):
    suffix_root = tmp_home / "suffix-root"
    monkeypatch.setenv("MAKAKOO_HOME_SUFFIX", str(suffix_root))

    cfg.add_source({
        "name": "suffix",
        "type": "plain",
        "path": "$MAKAKOO_HOME_SUFFIX/catalog",
    })

    data = json.loads(cfg.config_path().read_text())
    stored = next(source for source in data["sources"] if source["name"] == "suffix")
    assert stored["path"] == str((suffix_root / "catalog").resolve(strict=False))


def test_remove_source(tmp_home):
    cfg.add_source({"name": "extra", "type": "plain", "path": "/tmp/x"})
    cfg.remove_source("extra")
    data = json.loads(cfg.config_path().read_text())
    names = {s["name"] for s in data["sources"]}
    assert "extra" not in names


def test_cannot_remove_default(tmp_home):
    cfg.add_source({"name": "extra", "type": "plain", "path": "/tmp/d"})
    with pytest.raises(ValueError, match="cannot remove canonical"):
        cfg.remove_source("default")


def test_cannot_replace_fixed_default(tmp_home):
    with pytest.raises(ValueError, match="cannot be replaced"):
        cfg.add_source({"name": "default", "type": "plain", "path": "/tmp/d"})
    with pytest.raises(ValueError, match="cannot be replaced"):
        cfg.apply_changes([{"name": "default", "type": "plain", "path": "/tmp/d"}])


def test_canonical_role_cannot_bypass_enrichment_read_only_default(tmp_home):
    cfg.add_source({"name": "notes", "role": "canonical", "type": "plain", "path": "/tmp/n"})
    cfg.apply_changes([{"name": "more", "role": "canonical", "type": "plain", "path": "/tmp/m"}])

    data = json.loads(cfg.config_path().read_text())
    notes = next(source for source in data["sources"] if source["name"] == "notes")
    more = next(source for source in data["sources"] if source["name"] == "more")
    assert notes["role"] == "enrichment"
    assert more["role"] == "enrichment"
    assert notes["writable"] is False
    assert more["writable"] is False


def test_normalize_repairs_legacy_canonical_redirect(tmp_home):
    normalized = cfg._normalize_data({
        "canonical": "vault",
        "default": "vault",
        "sources": [
            {"name": "default", "type": "plain", "path": "/tmp/hijacked"},
            {"name": "vault", "role": "canonical", "type": "obsidian", "path": "/tmp/vault"},
        ],
    })

    assert normalized["canonical"] == "default"
    assert normalized["default"] == "default"
    default = next(source for source in normalized["sources"] if source["name"] == "default")
    vault = next(source for source in normalized["sources"] if source["name"] == "vault")
    assert default == cfg._default_config()["sources"][0]
    assert vault["role"] == "enrichment"


def test_normalize_uppercase_okf_is_canonicalized_and_read_only(tmp_home):
    normalized = cfg._normalize_data({
        "sources": [
            {
                "name": "catalog",
                "type": " OKF ",
                "path": "/tmp/catalog",
                "writable": True,
            },
        ],
    })

    catalog = next(source for source in normalized["sources"] if source["name"] == "catalog")
    assert catalog["type"] == "okf"
    assert catalog["writable"] is False


def test_remove_nonexistent_raises(tmp_home):
    cfg.add_source({"name": "real", "type": "plain", "path": "/tmp/y"})
    with pytest.raises(KeyError):
        cfg.remove_source("ghost")


def test_set_default_refuses_enrichment_source(tmp_home):
    cfg.add_source({"name": "vault", "type": "obsidian", "path": "/tmp/v"})
    with pytest.raises(ValueError, match="enrichment"):
        cfg.set_default("vault")
    registry = cfg.load_registry()
    assert registry.default_name == "default"
    assert registry.get("vault").role == "enrichment"


def test_set_default_unknown_raises(tmp_home):
    with pytest.raises(KeyError):
        cfg.set_default("nonexistent")


def test_registry_get_raises_on_unknown(tmp_home):
    registry = cfg.load_registry()
    with pytest.raises(KeyError):
        registry.get("does-not-exist")


def test_registry_get_default_works(tmp_home):
    registry = cfg.load_registry()
    default = registry.get_default()
    assert default.name == registry.default_name


def test_corrupt_config_falls_back_to_default(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("{not valid json")
    registry = cfg.load_registry()
    # Falls back to default instead of crashing
    assert len(registry.sources) >= 1


def test_registry_recovers_shared_backup_and_removes_stale_temp(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    backup = path.parent / cfg.BACKUP_FILENAME
    temporary = path.parent / cfg.TEMP_FILENAME
    body = json.dumps(cfg._default_config())
    backup.write_text(body, encoding="utf-8")
    temporary.write_text("partial", encoding="utf-8")
    (path.parent / cfg.OWNER_FILENAME).write_text(
        cfg.RECOVERY_MARKER_PREFIX + body,
        encoding="utf-8",
    )

    registry = cfg.load_registry()

    assert registry.default_name == "default"
    assert path.exists()
    assert not backup.exists()
    assert not temporary.exists()
    assert not (path.parent / cfg.OWNER_FILENAME).exists()


def test_registry_replacement_syncs_directory_entries(tmp_home, monkeypatch):
    synced = []
    monkeypatch.setattr(cfg, "_sync_directory", lambda directory: synced.append(directory))

    cfg.save_registry(cfg._default_config())
    cfg.add_source({"name": "notes", "type": "plain", "path": "/tmp/notes"})

    assert synced
    assert set(synced) == {cfg.config_path().parent}


def test_recovery_syncs_promoted_primary_before_backup_cleanup(tmp_home, monkeypatch):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    body = json.dumps(cfg._default_config())
    path.write_text(body, encoding="utf-8")
    (path.parent / cfg.BACKUP_FILENAME).write_text(body, encoding="utf-8")
    (path.parent / cfg.OWNER_FILENAME).write_text(
        cfg.RECOVERY_MARKER_PREFIX + body,
        encoding="utf-8",
    )
    synced = []
    monkeypatch.setattr(cfg, "_sync_directory", lambda directory: synced.append(directory))

    cfg.load_registry()

    assert synced == [path.parent, path.parent, path.parent]
    assert not (path.parent / cfg.BACKUP_FILENAME).exists()


def test_registry_preserves_unowned_recovery_artifact(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    backup = path.parent / cfg.BACKUP_FILENAME
    backup.write_text("unrelated", encoding="utf-8")

    with pytest.raises(RuntimeError, match="unowned brain source recovery artifacts"):
        cfg.load_registry()

    assert backup.read_text(encoding="utf-8") == "unrelated"
    assert not path.exists()


def test_registry_preserves_backup_when_primary_does_not_match_transaction(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    primary = json.dumps(cfg._default_config())
    intended = json.dumps({"canonical": "default", "sources": []})
    path.write_text(primary, encoding="utf-8")
    backup = path.parent / cfg.BACKUP_FILENAME
    backup.write_text("known-good", encoding="utf-8")
    (path.parent / cfg.OWNER_FILENAME).write_text(
        cfg.RECOVERY_MARKER_PREFIX + intended,
        encoding="utf-8",
    )

    with pytest.raises(RuntimeError, match="does not match the owned transaction"):
        cfg.load_registry()

    assert path.read_text(encoding="utf-8") == primary
    assert backup.read_text(encoding="utf-8") == "known-good"


def test_marker_only_recovery_reconstructs_initial_registry(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    intended = json.dumps(cfg._default_config(), indent=2) + "\n"
    (path.parent / cfg.OWNER_FILENAME).write_text(
        cfg.RECOVERY_MARKER_PREFIX + intended,
        encoding="utf-8",
    )

    registry = cfg.load_registry()

    assert registry.default_name == "default"
    assert path.read_text(encoding="utf-8") == intended
    assert not (path.parent / cfg.OWNER_FILENAME).exists()


def test_marker_before_temp_keeps_existing_registry(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    existing = json.dumps(cfg._default_config(), indent=2) + "\n"
    intended = json.dumps({"canonical": "default", "sources": []}, indent=2) + "\n"
    path.write_text(existing, encoding="utf-8")
    (path.parent / cfg.OWNER_FILENAME).write_text(
        cfg.RECOVERY_MARKER_PREFIX + intended,
        encoding="utf-8",
    )

    registry = cfg.load_registry()

    assert registry.default_name == "default"
    assert path.read_text(encoding="utf-8") == existing
    assert not (path.parent / cfg.OWNER_FILENAME).exists()


def test_temp_only_recovery_reconstructs_initial_registry(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    intended = json.dumps(cfg._default_config(), indent=2) + "\n"
    (path.parent / cfg.OWNER_FILENAME).write_text(
        cfg.RECOVERY_MARKER_PREFIX + intended,
        encoding="utf-8",
    )
    (path.parent / cfg.TEMP_FILENAME).write_text("partial", encoding="utf-8")

    registry = cfg.load_registry()

    assert registry.default_name == "default"
    assert path.read_text(encoding="utf-8") == intended
    assert not (path.parent / cfg.TEMP_FILENAME).exists()
    assert not (path.parent / cfg.OWNER_FILENAME).exists()


@pytest.mark.skipif(os.name == "nt", reason="dangling symlink semantics are Unix-specific")
def test_dangling_registry_symlink_fails_closed(tmp_home):
    path = cfg.config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.symlink_to(tmp_home / "missing-registry")

    with pytest.raises(RuntimeError, match="not a regular file"):
        cfg.load_registry()

    assert path.is_symlink()


@pytest.mark.skipif(os.name == "nt", reason="uses Unix flock to exercise cross-process compatibility")
def test_python_mutation_waits_for_shared_registry_lock(tmp_home):
    import fcntl

    lock_path = cfg.config_path().parent / cfg.LOCK_FILENAME
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    ready = tmp_home / "child-ready"
    script = """
import os
from pathlib import Path
Path(os.environ["READY_PATH"]).write_text("ready", encoding="utf-8")
import config
config.add_source({"name": "child", "type": "plain", "path": "/tmp/child"})
"""
    env = os.environ.copy()
    env["MAKAKOO_HOME"] = str(tmp_home)
    env["READY_PATH"] = str(ready)
    env["PYTHONPATH"] = str(HERE.parent / "src")

    with lock_path.open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        process = subprocess.Popen([sys.executable, "-c", script], env=env)
        deadline = time.monotonic() + 5
        while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.01)
        assert ready.exists(), "child did not reach the locked registry operation"
        time.sleep(0.1)
        assert process.poll() is None, "mutation bypassed brain_sources.lock"
        assert not cfg.config_path().exists()
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)

    assert process.wait(timeout=5) == 0
    data = json.loads(cfg.config_path().read_text(encoding="utf-8"))
    assert any(source["name"] == "child" for source in data["sources"])
    assert not (cfg.config_path().parent / cfg.OWNER_FILENAME).exists()
