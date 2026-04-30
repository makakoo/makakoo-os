from core.cortex.config import CortexConfig


def test_default_disabled():
    assert CortexConfig().enabled is False


def test_from_mapping_and_env(monkeypatch):
    cfg = CortexConfig.from_mapping({"enabled": True, "memory_limit": 9, "min_confidence": 0.8})
    assert cfg.enabled is True
    assert cfg.memory_limit == 9
    assert cfg.min_confidence == 0.8
    monkeypatch.setenv("MAKAKOO_CORTEX_MEMORY_LIMIT", "3")
    cfg.apply_env()
    assert cfg.memory_limit == 3
