"""
D.3 — workflow pause/resume E2E.

Exercises the full HarveyChat ↔ WorkflowEngine handshake without actually
booting HarveyChat: we synthesize the exact sequence of calls the chat
gateway makes, kill the engine mid-workflow, reopen it against the same
db, and confirm the resumed workflow completes on the first instance's
checkpoints. This is the safety net that proves the feature flag is
actually safe to turn on.

Scenarios covered:
  * pause_then_resume_from_same_process: cheapest sanity check
  * pause_then_resume_from_fresh_engine: mimics a daemon crash mid-run
  * failed_step_pauses_and_resumes_on_retry: error-path recovery
  * checkpoints_are_strictly_monotonic: checkpoint table grows, never replaces
"""

import os
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

# PEP-420 — add src/ to path so `core.workflow.engine` resolves.
_SRC = Path(__file__).resolve().parents[3]
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from core.workflow.engine import (  # noqa: E402
    StepState,
    Workflow,
    WorkflowEngine,
    WorkflowState,
    WorkflowStep,
)


def _drive_until_paused_or_done(engine: WorkflowEngine, wf: Workflow) -> int:
    """Run execute_next_step until the workflow either pauses or completes.

    Returns the number of execute_next_step calls made. The engine only
    fires one step per call, so production code (HarveyChat, SANCHO) is
    expected to loop; replicating that loop in tests avoids off-by-one
    surprises around the "all steps complete" transition.
    """
    calls = 0
    while True:
        calls += 1
        engine.execute_next_step(wf)
        if wf.state in (
            WorkflowState.PAUSED,
            WorkflowState.COMPLETED,
            WorkflowState.FAILED,
            WorkflowState.CANCELLED,
        ):
            return calls


def _make_wf(engine: WorkflowEngine) -> Workflow:
    """Build a 3-step workflow where step 2 pauses for user input."""
    wf = engine.create_workflow(
        name="pause-resume-e2e",
        description="test scaffold — step1→pause(step2)→step3",
        steps=[
            WorkflowStep(id="s1", name="gather", agent="harvey", action="prepare"),
            WorkflowStep(
                id="s2",
                name="ask-user",
                agent="harvey",
                action="pause_for_input",
                depends_on=["s1"],
            ),
            WorkflowStep(
                id="s3",
                name="finalize",
                agent="harvey",
                action="finalize",
                depends_on=["s2"],
            ),
        ],
    )
    return wf


def _register_handlers(engine: WorkflowEngine, pause_step_calls: list):
    """Register (agent, action) handlers. The 'pause_for_input' handler
    flips the step into PAUSED state on its first call and must not be
    invoked a second time by a well-behaved resume."""

    def prepare_handler(step, ctx):
        return {"gathered": True, "fact_from_step1": "hello"}

    def pause_handler(step, ctx):
        pause_step_calls.append(step.id)
        # Pause ourselves — engine sees PAUSED state and parks.
        step.state = StepState.PAUSED
        step.pause_prompt = "what should I do next?"
        return {}

    def finalize_handler(step, ctx):
        # Resumed context must surface the user's reply + the upstream fact.
        return {
            "final": True,
            "echo": ctx.get("user_choice", "<missing>"),
            "upstream": ctx.get("fact_from_step1", "<missing>"),
        }

    engine.register_handler("harvey", "prepare", prepare_handler)
    engine.register_handler("harvey", "pause_for_input", pause_handler)
    engine.register_handler("harvey", "finalize", finalize_handler)


class WorkflowPauseResumeE2E(unittest.TestCase):
    """v0.2 D.3 — the feature flag's safety net."""

    def setUp(self):
        self._tmpdir = tempfile.TemporaryDirectory()
        self.db_path = str(Path(self._tmpdir.name) / "workflows.db")

    def tearDown(self):
        self._tmpdir.cleanup()

    # ──────────────────────────────────────────────────────────────────

    def test_pause_then_resume_from_same_process(self):
        """Basic lifecycle: run, pause, resume with input, run to completion."""
        pause_calls: list = []
        engine = WorkflowEngine(db_path=self.db_path)
        try:
            _register_handlers(engine, pause_calls)
            wf = _make_wf(engine)
            engine.start_workflow(wf)

            # Drive until the pause handler fires.
            _drive_until_paused_or_done(engine, wf)
            self.assertTrue(wf.get_step("s1").is_complete())
            self.assertEqual(wf.state, WorkflowState.PAUSED)
            self.assertEqual(wf.get_step("s2").state, StepState.PAUSED)
            self.assertEqual(pause_calls, ["s2"])

            # Further execute_next_step calls while paused must be no-ops.
            self.assertIsNone(engine.execute_next_step(wf))
            self.assertEqual(pause_calls, ["s2"], "no-op must not reinvoke handler")

            # User replies. Engine resumes and finishes.
            engine.resume_workflow(wf, user_input={"user_choice": "forward"})
            _drive_until_paused_or_done(engine, wf)

            self.assertEqual(wf.state, WorkflowState.COMPLETED)
            s3 = wf.get_step("s3")
            self.assertEqual(s3.state, StepState.CHECKPOINTED)
            self.assertEqual(s3.output_context.get("echo"), "forward")
            self.assertEqual(s3.output_context.get("upstream"), "hello")
        finally:
            engine.close()

    def test_pause_then_resume_from_fresh_engine(self):
        """Mimic a daemon crash: run until pause, drop engine, reopen db,
        resume → workflow completes. No step re-runs."""
        pause_calls_1: list = []
        engine_1 = WorkflowEngine(db_path=self.db_path)
        try:
            _register_handlers(engine_1, pause_calls_1)
            wf = _make_wf(engine_1)
            wf_id = wf.id
            engine_1.start_workflow(wf)

            _drive_until_paused_or_done(engine_1, wf)
            self.assertEqual(wf.state, WorkflowState.PAUSED)
        finally:
            engine_1.close()

        # New process, same DB — every piece of state hydrates from disk.
        pause_calls_2: list = []
        engine_2 = WorkflowEngine(db_path=self.db_path)
        try:
            _register_handlers(engine_2, pause_calls_2)
            wf_reloaded = engine_2.get_workflow(wf_id)
            self.assertIsNotNone(wf_reloaded)
            self.assertEqual(wf_reloaded.state, WorkflowState.PAUSED)
            self.assertTrue(wf_reloaded.get_step("s1").is_complete())
            self.assertEqual(wf_reloaded.get_step("s2").state, StepState.PAUSED)
            self.assertFalse(wf_reloaded.get_step("s3").is_complete())

            # Resume + finish.
            engine_2.resume_workflow(
                wf_reloaded, user_input={"user_choice": "after-restart"}
            )
            _drive_until_paused_or_done(engine_2, wf_reloaded)

            self.assertEqual(wf_reloaded.state, WorkflowState.COMPLETED)
            s3 = wf_reloaded.get_step("s3")
            self.assertEqual(
                s3.output_context.get("echo"),
                "after-restart",
                "user input must land in resumed context",
            )
            self.assertEqual(
                s3.output_context.get("upstream"),
                "hello",
                "pre-pause context must survive the restart",
            )

            # Critical safety invariant: s2 handler must NOT be re-invoked
            # after resume. Double-execute would re-prompt the user for
            # input they already gave.
            self.assertEqual(
                pause_calls_2,
                [],
                "resume must not re-run the pause handler",
            )
        finally:
            engine_2.close()

    def test_failed_step_pauses_and_resumes_on_retry(self):
        """Transient failure path: handler raises the first time, caller
        resumes after fixing the fault, the engine walks forward."""
        attempts = {"count": 0}
        engine = WorkflowEngine(db_path=self.db_path)

        def flaky_handler(step, ctx):
            attempts["count"] += 1
            if attempts["count"] == 1:
                raise RuntimeError("transient outage")
            return {"recovered": True}

        def finalize_handler(step, ctx):
            return {"final": True, "recovered": ctx.get("recovered", False)}

        try:
            engine.register_handler("harvey", "flaky", flaky_handler)
            engine.register_handler("harvey", "finalize", finalize_handler)

            wf = engine.create_workflow(
                name="retry-e2e",
                steps=[
                    WorkflowStep(id="f1", name="flaky", agent="harvey", action="flaky"),
                    WorkflowStep(
                        id="f2",
                        name="fin",
                        agent="harvey",
                        action="finalize",
                        depends_on=["f1"],
                    ),
                ],
            )
            engine.start_workflow(wf)

            # First attempt — raises → step FAILED, workflow PAUSED.
            _drive_until_paused_or_done(engine, wf)
            self.assertEqual(wf.state, WorkflowState.PAUSED)
            self.assertEqual(wf.get_step("f1").state, StepState.FAILED)

            # Operator retries: unblock the step by flipping it back and
            # resuming. This mirrors what a HarveyChat /retry command does.
            wf.get_step("f1").state = StepState.PENDING
            wf.get_step("f1").error = ""
            engine.resume_workflow(wf)

            _drive_until_paused_or_done(engine, wf)
            self.assertEqual(wf.state, WorkflowState.COMPLETED)
            self.assertTrue(wf.get_step("f2").output_context["recovered"])
        finally:
            engine.close()

    def test_checkpoints_are_strictly_monotonic(self):
        """Every step completion writes a fresh checkpoint row; the table
        must only grow (never UPDATE an existing row)."""
        engine = WorkflowEngine(db_path=self.db_path)
        try:
            _register_handlers(engine, [])
            wf = _make_wf(engine)
            engine.start_workflow(wf)

            _drive_until_paused_or_done(engine, wf)  # s1 done, s2 pauses
            engine.resume_workflow(wf, user_input={"user_choice": "go"})
            _drive_until_paused_or_done(engine, wf)  # s3 done

            # Walk the checkpoint table directly.
            conn = sqlite3.connect(self.db_path)
            rows = conn.execute(
                "SELECT workflow_id, step_id, checkpoint_at "
                "FROM workflow_checkpoints "
                "WHERE workflow_id = ? ORDER BY id",
                (wf.id,),
            ).fetchall()
            conn.close()

            self.assertEqual(len(rows), 3, f"expected 3 checkpoints, got {rows}")
            # Checkpoint timestamps are non-decreasing.
            ts_list = [r[2] for r in rows]
            self.assertEqual(ts_list, sorted(ts_list))
            # Each checkpoint is tied to the step that just completed/paused.
            self.assertEqual([r[1] for r in rows], ["s1", "s2", "s3"])
        finally:
            engine.close()

    def test_makakoo_home_takes_precedence_over_harvey_home(self):
        """v0.2 opportunistic fix: default db_path honors MAKAKOO_HOME
        before falling back to HARVEY_HOME."""
        with tempfile.TemporaryDirectory() as tmp_makakoo, tempfile.TemporaryDirectory() as tmp_harvey:
            saved = {
                "MAKAKOO_HOME": os.environ.pop("MAKAKOO_HOME", None),
                "HARVEY_HOME": os.environ.pop("HARVEY_HOME", None),
            }
            try:
                os.environ["MAKAKOO_HOME"] = tmp_makakoo
                os.environ["HARVEY_HOME"] = tmp_harvey
                engine = WorkflowEngine()
                try:
                    # _save_workflow writes to disk synchronously.
                    wf = engine.create_workflow(name="probe", steps=[])
                    self.assertIsNotNone(wf)
                finally:
                    engine.close()
                makakoo_db = Path(tmp_makakoo) / "data" / "workflow" / "workflows.db"
                harvey_db = Path(tmp_harvey) / "data" / "workflow" / "workflows.db"
                self.assertTrue(makakoo_db.exists(), "MAKAKOO_HOME branch must be used")
                self.assertFalse(harvey_db.exists(), "HARVEY_HOME must not be preferred")
            finally:
                for k, v in saved.items():
                    if v is None:
                        os.environ.pop(k, None)
                    else:
                        os.environ[k] = v


if __name__ == "__main__":
    unittest.main()
