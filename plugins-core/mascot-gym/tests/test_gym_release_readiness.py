from __future__ import annotations

import builtins
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
LIB_SRC = ROOT / "plugins-core" / "lib-harvey-core" / "src"
GYM_SRC = ROOT / "plugins-core" / "mascot-gym" / "src"

for p in (str(LIB_SRC), str(GYM_SRC)):
    if p not in sys.path:
        sys.path.insert(0, p)


def test_muon_import_is_lazy_and_quiet():
    env = os.environ.copy()
    env["PYTHONPATH"] = f"{LIB_SRC}:{GYM_SRC}"
    proc = subprocess.run(
        [sys.executable, "-c", "import core.gym.muon; print('ok')"],
        env=env,
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert proc.returncode == 0, proc.stderr
    assert proc.stdout.strip() == "ok"
    assert proc.stderr.strip() == ""


def test_new_gym_tasks_registered_in_inprocess_sancho():
    from core.sancho.engine import Sancho

    sancho = Sancho(subscribe_wake=False)
    tasks = sancho.registry.tasks if hasattr(sancho.registry, "tasks") else sancho.registry._tasks
    expected = {"gym_cascade", "gym_eval_run", "gym_polar_fan", "gym_simplify", "gym_snapshot"}
    assert expected <= set(tasks)


def test_eval_cache_respects_makakoo_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path))
    from core.gym.eval_harness import EvalResult, HarnessResult, save_benchmark_result

    result = HarnessResult(
        composite=1.0,
        task_results=[EvalResult("smoke", 1.0, 1.0, True, "ok")],
        elapsed_seconds=0.01,
        budget_hit=False,
        simplicity_score=1.0,
    )
    save_benchmark_result("smoke", result, tag="test")
    assert (tmp_path / "data" / "gym" / "eval_cache.json").exists()


def test_agent_flow_handler_does_not_require_pyyaml_to_import(monkeypatch):
    real_import = builtins.__import__

    def guarded_import(name, *args, **kwargs):
        if name == "yaml":
            raise ImportError("yaml intentionally unavailable")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", guarded_import)

    from gym.handlers.agent_flow import AgentFlowHandler

    handler = AgentFlowHandler()
    assert handler.detect(Path("flow.json"), '{"steps": [], "tools": []}') is True
    # YAML parsing should fail gracefully, not crash package import/detection.
    assert handler.detect(Path("flow.yaml"), "steps: []\ntools: []\n") is False


def test_run_sancho_task_gym_cascade_smoke(tmp_path):
    plugins_dir = tmp_path / "plugins"
    plugins_dir.mkdir()
    (tmp_path / "bootstrap").mkdir()
    (tmp_path / "data" / "improvements" / "approved").mkdir(parents=True)
    (plugins_dir / "lib-harvey-core").symlink_to(ROOT / "plugins-core" / "lib-harvey-core")
    (tmp_path / "bootstrap" / "global.md").write_text(
        "Makakoo bootstrap persona task tool output format constraints\n"
    )

    env = os.environ.copy()
    env["MAKAKOO_HOME"] = str(tmp_path)
    proc = subprocess.run(
        [
            sys.executable,
            str(ROOT / "plugins-core" / "mascot-gym" / "src" / "run-sancho-task.py"),
            "--task",
            "gym_cascade",
        ],
        env=env,
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert proc.returncode == 0, proc.stderr
    assert '"ok": true' in proc.stdout
