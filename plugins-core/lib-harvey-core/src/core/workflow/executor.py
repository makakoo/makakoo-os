"""
Workflow Executor — Background process that runs workflow steps.

Designed to:
1. Poll for queued/running workflows
2. Execute steps sequentially (respecting dependencies)
3. Checkpoint after each step
4. Handle failures gracefully
5. Notify HarveyChat of pause points
6. Resume from checkpoints on restart

Can run as:
- Daemon (continuous polling)
- Cron job (run once, execute all due steps, exit)
- Triggered from HarveyChat (execute next step)
"""

import asyncio
import json
import logging
import signal
import time
from pathlib import Path
from typing import Callable, Dict, Optional

from core.workflow.engine import WorkflowEngine, WorkflowState

log = logging.getLogger("workflow.executor")


class WorkflowExecutor:
    """Background executor for workflows."""

    def __init__(
        self,
        engine: Optional[WorkflowEngine] = None,
        poll_interval: float = 30.0,
        max_steps_per_cycle: int = 5,
    ):
        self.engine = engine or WorkflowEngine()
        self.poll_interval = poll_interval
        self.max_steps_per_cycle = max_steps_per_cycle
        self._running = False

    def register_handler(self, agent: str, action: str, handler: Callable):
        """Register step handler."""
        self.engine.register_handler(agent, action, handler)
        log.info(f"Registered {agent}/{action}")

    async def run_daemon(self):
        """Run as daemon (continuous polling)."""
        self._running = True

        def signal_handler(sig, frame):
            log.info("Received signal, shutting down...")
            self._running = False

        signal.signal(signal.SIGTERM, signal_handler)
        signal.signal(signal.SIGINT, signal_handler)

        log.info(f"Workflow executor daemon started (poll interval: {self.poll_interval}s)")

        try:
            while self._running:
                self.execute_cycle()
                await asyncio.sleep(self.poll_interval)
        finally:
            log.info("Workflow executor daemon stopped")

    async def run_once(self):
        """Run single execution cycle (for cron)."""
        log.info("Workflow executor running single cycle")
        self.execute_cycle()

    def execute_cycle(self):
        """Execute one polling cycle."""
        # Find all running/queued workflows
        cursor = self.engine.db.execute(
            "SELECT id FROM workflows WHERE state IN (?, ?)",
            (WorkflowState.QUEUED.value, WorkflowState.RUNNING.value),
        )

        workflow_ids = [row[0] for row in cursor.fetchall()]
        steps_executed = 0

        for wf_id in workflow_ids:
            if steps_executed >= self.max_steps_per_cycle:
                log.info(f"Hit max steps per cycle ({self.max_steps_per_cycle}), pausing")
                break

            wf = self.engine.get_workflow(wf_id)
            if not wf:
                continue

            log.info(f"Processing workflow {wf.id} ({wf.name})")

            # Try to execute next step
            result = self.engine.execute_next_step(wf)
            if result:
                steps_executed += 1
                log.info(f"  → Executed step {result}")

                # Send pause notification if needed
                if wf.state == WorkflowState.PAUSED:
                    self._notify_pause(wf)

        log.info(f"Cycle complete: {steps_executed} steps executed")

    def _notify_pause(self, wf):
        """Notify HarveyChat that workflow is paused waiting for input."""
        # This would send a message to the user via Telegram
        # Example: "Image generation paused — please select your preferred style"
        log.info(f"Workflow {wf.id} paused: {wf.pause_reason}")
        # TODO: Call HarveyChat gateway to notify user

    def get_status(self) -> Dict:
        """Get executor status."""
        cursor = self.engine.db.execute(
            "SELECT state, COUNT(*) as count FROM workflows GROUP BY state"
        )
        states = {row[0]: row[1] for row in cursor.fetchall()}

        return {
            "running": self._running,
            "poll_interval": self.poll_interval,
            "workflows": states,
        }

    def stop(self):
        """Stop daemon."""
        self._running = False
        self.engine.close()


# ═══════════════════════════════════════════════════════════════
# Built-in Step Handlers
# ═══════════════════════════════════════════════════════════════


def handler_user_input(step, context: Dict) -> Dict:
    """
    Step handler: Pause workflow and ask user for input.

    Step config:
    {
        "agent": "user",
        "action": "input",
        "pause_prompt": "Which image variation do you prefer? (1-3)"
    }

    User responds → resume_workflow() with user_choice
    """
    # Pause and wait for user input
    step.pause_prompt = context.get("pause_prompt", "Your input needed")
    step.state = StepState.PAUSED
    return {}


def handler_log_event(step, context: Dict) -> Dict:
    """
    Step handler: Log event to Brain journal.

    Step config:
    {
        "agent": "brain",
        "action": "log_event",
        "log_content": "[[Image Generation]] campaign started"
    }
    """
    content = context.get("log_content", "")
    log.info(f"Logging to Brain: {content}")

    # In real impl, call Brain bridge
    # logseq_bridge.append_journal(content)

    return {"logged": True}


def handler_send_notification(step, context: Dict) -> Dict:
    """
    Step handler: Send notification via Telegram.

    Step config:
    {
        "agent": "telegram",
        "action": "notify",
        "message": "Your images are ready!"
    }
    """
    message = context.get("message", "")
    log.info(f"Would send Telegram: {message}")

    # In real impl, call Telegram API or HarveyChat gateway
    # await telegram_channel.send(user_id, message)

    return {"notified": True}


def handler_delay(step, context: Dict) -> Dict:
    """
    Step handler: Wait for specified time.

    Useful for: rate limiting, scheduled steps, cooling down

    Step config:
    {
        "agent": "system",
        "action": "delay",
        "seconds": 60
    }
    """
    duration = context.get("seconds", 1)
    log.info(f"Delaying for {duration} seconds...")
    time.sleep(duration)
    return {"delayed": True}


# ═══════════════════════════════════════════════════════════════
# Workflow Templates
# ═══════════════════════════════════════════════════════════════


class WorkflowTemplates:
    """Pre-built workflow templates."""

    @staticmethod
    def image_generation_workflow(user_id: str, initial_request: str) -> "Workflow":
        """
        Multi-step image generation with user refinement.

        Flow:
        1. Gather detailed requirements
        2. Generate 3 variations
        3. Get user preference (pause point)
        4. Upscale selected image
        5. Archive to storage
        """
        from core.workflow.engine import Workflow, WorkflowStep, StepState

        wf = Workflow(
            id=f"img_gen_{int(time.time())}",
            name="Image Generation Campaign",
            description=f"Generate custom image for @{user_id}",
        )

        # Step 1: Gather requirements
        wf.steps.append(
            WorkflowStep(
                id="step_1_requirements",
                name="Gather Requirements",
                agent="harvey",
                action="gather_requirements",
                input_context={"initial_request": initial_request},
            )
        )

        # Step 2: Generate variations
        wf.steps.append(
            WorkflowStep(
                id="step_2_generate",
                name="Generate Variations",
                agent="image_gen",
                action="generate_variations",
                depends_on=["step_1_requirements"],
                input_context={},
            )
        )

        # Step 3: Get user feedback (PAUSE POINT)
        wf.steps.append(
            WorkflowStep(
                id="step_3_feedback",
                name="Get User Preference",
                agent="user",
                action="input",
                depends_on=["step_2_generate"],
                pause_prompt="Which variation do you prefer? (1-3) or request changes",
            )
        )

        # Step 4: Upscale
        wf.steps.append(
            WorkflowStep(
                id="step_4_upscale",
                name="Upscale Selected Image",
                agent="processor",
                action="upscale",
                depends_on=["step_3_feedback"],
            )
        )

        # Step 5: Archive
        wf.steps.append(
            WorkflowStep(
                id="step_5_archive",
                name="Archive Result",
                agent="storage",
                action="save_to_brain",
                depends_on=["step_4_upscale"],
            )
        )

        return wf

    @staticmethod
    def research_workflow(topic: str, depth: str = "medium") -> "Workflow":
        """
        Multi-day research campaign.

        Flow:
        1. Define research scope
        2. Search literature (with delays to respect rate limits)
        3. Extract findings (checkpoint after each paper)
        4. Synthesize insights
        5. Generate report
        6. Save to Brain
        """
        from core.workflow.engine import Workflow, WorkflowStep

        wf = Workflow(
            id=f"research_{int(time.time())}",
            name=f"Research: {topic}",
            description=f"Deep research on {topic} ({depth} depth)",
        )

        wf.steps.extend(
            [
                WorkflowStep(
                    id="step_1_scope",
                    name="Define Research Scope",
                    agent="harvey",
                    action="define_scope",
                    input_context={"topic": topic, "depth": depth},
                ),
                WorkflowStep(
                    id="step_2_search_lit",
                    name="Search Literature",
                    agent="researcher",
                    action="search_arxiv",
                    depends_on=["step_1_scope"],
                ),
                # Multiple extraction steps with delays
                WorkflowStep(
                    id="step_3_extract_1",
                    name="Extract Findings (Batch 1)",
                    agent="researcher",
                    action="extract_findings",
                    depends_on=["step_2_search_lit"],
                ),
                WorkflowStep(
                    id="step_3_delay_1",
                    name="Rate Limit Delay",
                    agent="system",
                    action="delay",
                    depends_on=["step_3_extract_1"],
                    input_context={"seconds": 30},
                ),
                WorkflowStep(
                    id="step_3_extract_2",
                    name="Extract Findings (Batch 2)",
                    agent="researcher",
                    action="extract_findings",
                    depends_on=["step_3_delay_1"],
                ),
                WorkflowStep(
                    id="step_4_synthesize",
                    name="Synthesize Insights",
                    agent="synthesizer",
                    action="synthesize",
                    depends_on=["step_3_extract_2"],
                ),
                WorkflowStep(
                    id="step_5_report",
                    name="Generate Report",
                    agent="report_gen",
                    action="generate_markdown",
                    depends_on=["step_4_synthesize"],
                ),
                WorkflowStep(
                    id="step_6_save",
                    name="Save to Brain",
                    agent="storage",
                    action="save_research_page",
                    depends_on=["step_5_report"],
                ),
            ]
        )

        return wf
