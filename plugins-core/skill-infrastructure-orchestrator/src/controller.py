"""
Orchestrator controller - main event loop for task orchestration.
Coordinates task queue, message bus, task graph, and agent spawning.
"""

import json
import os
import signal
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# Ensure stdout is unbuffered for daemon logging
sys.stdout = os.fdopen(sys.stdout.fileno(), "w", buffering=1)
sys.stderr = os.fdopen(sys.stderr.fileno(), "w", buffering=1)


# Add parent path for imports
sys.path.insert(0, str(Path(__file__).parent))

from task_queue import TaskQueue
from task_graph import TaskGraph
from message_bus import MessageBus
from router import Router
from llm_gateway import call_llm


@dataclass
class AgentConfig:
    """Configuration for spawning an agent."""
    task_id: str
    agent_id: str
    agent_type: str
    model: str
    endpoint: str
    instructions: str
    context: dict


class Orchestrator:
    """
    Main orchestration controller.
    Runs the event loop that processes tasks, dispatches agents, and aggregates results.
    """

    def __init__(
        self,
        queue_path: str = None,
        messages_path: str = None,
        state_path: str = None,
        poll_interval: float = 1.0,
    ):
        _harvey_home = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        _orch_base = os.path.join(_harvey_home, "data", "orchestrator")
        if queue_path is None:
            queue_path = os.path.join(_orch_base, "queues")
        if messages_path is None:
            messages_path = os.path.join(_orch_base, "messages")
        if state_path is None:
            state_path = os.path.join(_orch_base, "state")
        self._harvey_home = _harvey_home
        self.queue = TaskQueue(queue_path)
        self.message_bus = MessageBus(messages_path)
        self.graph = TaskGraph(state_path)
        self.router = Router()
        self.poll_interval = poll_interval

        self.running_agents: dict[str, subprocess.Popen] = {}
        self._shutdown = False
        self._task_results: dict[str, dict] = {}

        # Register signal handlers
        signal.signal(signal.SIGINT, self._handle_shutdown)
        signal.signal(signal.SIGTERM, self._handle_shutdown)

    def _handle_shutdown(self, signum, frame) -> None:
        """Graceful shutdown on SIGINT/SIGTERM."""
        print("Orchestrator shutting down...")
        self._shutdown = True

    def submit(self, task: dict) -> str:
        """
        Submit a new task for orchestration.
        If task has dependencies, adds to graph first.
        Returns the task_id.
        """
        if "task_id" not in task:
            task["task_id"] = str(uuid.uuid4())

        if "created_at" not in task:
            task["created_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())

        if "status" not in task:
            task["status"] = "pending"

        # Add to graph for dependency tracking
        self.graph.add_task(task)

        # Enqueue for processing
        self.queue.enqueue(task)

        return task["task_id"]

    def _process_incoming(self) -> None:
        """
        Move runnable tasks from incoming to running queue.
        Reads incoming queue, adds to graph, checks deps, moves to running.
        """
        # Read all pending tasks from incoming queue
        incoming_tasks = self.queue.list_tasks("incoming")

        for task in incoming_tasks:
            task_id = task["task_id"]

            # Add to graph if not already there
            if task_id not in self.graph.nodes:
                self.graph.add_task(task)

            # Check if dependencies are satisfied
            deps = task.get("dependencies", [])
            deps_satisfied = all(
                self.graph.nodes.get(dep_id, None)
                and self.graph.nodes[dep_id].status == "completed"
                for dep_id in deps
            )

            if deps_satisfied:
                # Move to running
                if self.queue._move_task(task_id, "incoming", "running"):
                    self.graph.update_status(task_id, "running")

    def _dispatch_runnable(self) -> None:
        """Spawn agents for tasks that are in running state."""
        running_tasks = self.queue.list_tasks("running")

        for task in running_tasks:
            task_id = task["task_id"]

            # Skip if already dispatched
            if task_id in self.running_agents:
                continue

            # Get routing
            model, endpoint = self.router.route(task)

            # Prepare instructions
            instructions = task.get("payload", {}).get(
                "instructions", task.get("description", "")
            )

            # Spawn agent
            agent_id = self._spawn_agent(
                AgentConfig(
                    task_id=task_id,
                    agent_id=str(uuid.uuid4()),
                    agent_type=task.get("agent_type", "general-purpose"),
                    model=model,
                    endpoint=endpoint,
                    instructions=instructions,
                    context=task.get("payload", {}).get("context", {}),
                )
            )
            self.running_agents[task_id] = agent_id

    def _spawn_agent(self, config: AgentConfig) -> subprocess.Popen:
        """
        Spawn a subprocess agent.
        Returns the Popen handle.
        """
        env = os.environ.copy()
        env["HARVEY_TASK_ID"] = config.task_id
        env["HARVEY_AGENT_ID"] = config.agent_id
        env["HARVEY_AGENT_TYPE"] = config.agent_type
        env["HARVEY_MODEL"] = config.model
        env["HARVEY_ENDPOINT"] = config.endpoint

        # Write task context to a temp file for the agent to pick up
        context_file = Path(self._harvey_home) / "data" / "orchestrator" / f"{config.task_id}.context.json"
        context_file.write_text(
            json.dumps(
                {
                    "task_id": config.task_id,
                    "agent_id": config.agent_id,
                    "instructions": config.instructions,
                    "context": config.context,
                }
            )
        )

        # Build inner script as a plain string to avoid f-string nesting issues
        inner_script = (
            "import sys\n"
            "sys.path.insert(0, '%s')\n"
            "from llm_gateway import call_llm\n"
            "from pathlib import Path\n"
            "import json\n"
            "\n"
            "with open('%s') as f:\n"
            "    ctx = json.load(f)\n"
            "\n"
            "result = call_llm(\n"
            "    ctx['instructions'],\n"
            "    model='%s',\n"
            "    endpoint='%s'\n"
            ")\n"
            "\n"
            "# Write result\n"
            "result_file = Path('%s') / 'data' / 'orchestrator' / '%s.result.json'\n"
            "result_file.write_text(json.dumps({'result': result}))\n"
            "print('Agent completed:', ctx['task_id'])\n"
        ) % (
            str(Path(__file__).parent),
            str(context_file),
            config.model,
            config.endpoint,
            self._harvey_home,
            config.task_id,
        )

        proc = subprocess.Popen(
            [sys.executable, "-c", inner_script],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )

        return proc

    def _collect_results(self) -> None:
        """Check for completed agents and collect their results."""
        completed = []
        for task_id, proc in list(self.running_agents.items()):
            if proc.poll() is not None:
                # Process finished
                completed.append(task_id)
                stdout, stderr = proc.communicate()

                # Try to read result
                result_file = (
                    Path(self._harvey_home) / "data" / "orchestrator"
                    / f"{task_id}.result.json"
                )
                if result_file.exists():
                    try:
                        result_data = json.loads(result_file.read_text())
                        self._task_results[task_id] = result_data.get("result", {})
                        self.queue.complete(task_id, self._task_results[task_id])
                        self.graph.notify_completed(task_id, self._task_results[task_id])
                    except (json.JSONDecodeError, KeyError):
                        self.queue.fail(task_id, f"Agent error: {stderr.decode()[:500]}")
                        self.graph.notify_failed(task_id, f"Agent error: {stderr.decode()[:500]}")
                    result_file.unlink()
                else:
                    # No result file - check if it was a graceful completion
                    if proc.returncode == 0:
                        self.queue.complete(task_id, {"output": stdout.decode()[:1000]})
                        self.graph.notify_completed(task_id, {"output": stdout.decode()[:1000]})
                    else:
                        self.queue.fail(task_id, f"Agent exited with code {proc.returncode}")
                        self.graph.notify_failed(task_id, f"Agent exited with code {proc.returncode}")

                # Clean up context file
                context_file = (
                    Path(self._harvey_home) / "data" / "orchestrator"
                    / f"{task_id}.context.json"
                )
                if context_file.exists():
                    context_file.unlink()

        for task_id in completed:
            del self.running_agents[task_id]

    def _handle_failures(self) -> None:
        """Propagate failures through the DAG."""
        failed_tasks = self.queue.list_tasks("failed")

        for task in failed_tasks:
            task_id = task["task_id"]
            error = task.get("result", {}).get("error", "Unknown error")

            # Propagate to dependents
            failed_dependents = self.graph.notify_failed(task_id, error)

            # Move dependent tasks to failed state in queue
            for dep_id in failed_dependents:
                dep_task = self.queue.get_task(dep_id, "running")
                if dep_task:
                    self.queue.fail(dep_id, f"Dependency {task_id} failed: {error}")

    def run(self) -> None:
        """
        Main event loop.
        Runs until shutdown signal received.
        """
        print("Orchestrator started")
        iteration = 0

        while not self._shutdown:
            iteration += 1

            try:
                # Step 1: Process incoming tasks
                self._process_incoming()

                # Step 2: Dispatch runnable tasks
                self._dispatch_runnable()

                # Step 3: Collect results
                self._collect_results()

                # Step 4: Handle failures
                self._handle_failures()

                # Check for messages
                self._process_messages()

            except Exception as e:
                print(f"Error in orchestration loop (iter {iteration}): {e}")

            time.sleep(self.poll_interval)

        # Graceful shutdown
        self._shutdown_agents()
        print("Orchestrator stopped")

    def _process_messages(self) -> None:
        """Process messages in the orchestrator's inbox."""
        while True:
            msg = self.message_bus.receive("orchestrator", timeout=0.1)
            if msg is None:
                break

            msg_type = msg.get("type")
            payload = msg.get("payload", {})

            if msg_type == "result":
                # Agent result
                task_id = payload.get("task_id")
                if task_id:
                    self._task_results[task_id] = payload.get("result", {})
            elif msg_type == "error":
                task_id = payload.get("task_id")
                error = payload.get("error")
                if task_id and error:
                    if task_id in self.running_agents:
                        self.running_agents[task_id].terminate()
                    self.queue.fail(task_id, error)
                    self.graph.notify_failed(task_id, error)

    def _shutdown_agents(self) -> None:
        """Terminate all running agents on shutdown."""
        for task_id, proc in self.running_agents.items():
            try:
                proc.terminate()
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        self.running_agents.clear()


def main():
    """Run the orchestrator."""
    orchestrator = Orchestrator()
    orchestrator.run()


if __name__ == "__main__":
    main()
