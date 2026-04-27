#!/usr/bin/env python3
"""
Dev Orchestrator — Parallel Development Team

Breaks multi-component tasks into independent units, launches agents in parallel,
and consolidates results.

Usage:
    python3 orchestrate.py --feature "auth system" --plan-file plan.md
    python3 orchestrate.py --feature "checkout" --tasks-config tasks.json

Or import as a module:
    from orchestrate import plan_tasks, launch_parallel, wait_for_results, run_development_sprint
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field, asdict
from datetime import datetime
from pathlib import Path
from typing import List, Dict, Any, Optional, Callable

# ─────────────────────────────────────────────────────────────────────────────
# Configuration
# ─────────────────────────────────────────────────────────────────────────────

_HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
HARVEY_OS = _HARVEY_HOME
HARVEY_DATA = _HARVEY_HOME / "data"
SKILL_DIR = HARVEY_OS / "skills" / "dev" / "dev-orchestrator"
DEFAULT_OUTPUT_DIR = HARVEY_DATA / "dev-orchestrator"

# LLM gateway (matches dispatcher.py)
LLM_BASE_URL = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1")
LLM_MODEL = os.environ.get("LLM_MODEL", "auto")
LLM_API_KEY = os.environ.get("LLM_API_KEY", os.environ.get("SWITCHAI_KEY", ""))

# ─────────────────────────────────────────────────────────────────────────────
# Data Structures
# ─────────────────────────────────────────────────────────────────────────────

@dataclass
class AgentTask:
    """Defines a single agent task for parallel execution."""
    name: str                          # Task identifier (e.g., "auth-models")
    description: str                  # Human-readable description
    prompt: str                        # Full agent prompt
    output_file: str                  # Where to write results
    summary_file: str                 # Short summary file
    subagent_type: str = "general-purpose"  # Agent flavor
    model: str = ""                    # Override model (optional)
    depends_on: List[str] = field(default_factory=list)  # Task names this depends on
    status: str = "pending"            # pending, running, completed, failed
    agent_id: str = ""                 # Assigned agent ID
    pid: int = 0                      # Process ID
    result: Optional[Dict] = None      # Execution result
    started_at: Optional[str] = None
    completed_at: Optional[str] = None
    error: Optional[str] = None

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass
class Sprint:
    """A development sprint — collection of parallel tasks for one feature."""
    feature_name: str
    plan: str                          # Original plan text
    tasks: List[AgentTask] = field(default_factory=list)
    output_dir: Path = field(default_factory=lambda: DEFAULT_OUTPUT_DIR)
    created_at: str = ""
    completed_at: Optional[str] = None

    def save_state(self) -> None:
        """Persist sprint state to output_dir."""
        self.output_dir.mkdir(parents=True, exist_ok=True)
        state_file = self.output_dir / "sprint.json"
        state = {
            "feature_name": self.feature_name,
            "plan": self.plan,
            "tasks": [t.to_dict() for t in self.tasks],
            "created_at": self.created_at,
            "completed_at": self.completed_at,
        }
        state_file.write_text(json.dumps(state, indent=2))

    @classmethod
    def load_state(cls, output_dir: Path) -> Optional["Sprint"]:
        """Load sprint state from output_dir."""
        state_file = output_dir / "sprint.json"
        if not state_file.exists():
            return None
        data = json.loads(state_file.read_text())
        tasks = [AgentTask(**t) for t in data.get("tasks", [])]
        return cls(
            feature_name=data["feature_name"],
            plan=data["plan"],
            tasks=tasks,
            output_dir=output_dir,
            created_at=data.get("created_at", ""),
            completed_at=data.get("completed_at"),
        )


# ─────────────────────────────────────────────────────────────────────────────
# Task Planning
# ─────────────────────────────────────────────────────────────────────────────

def plan_tasks(description: str, plan: str = "", existing_tasks: List[Dict] = None) -> List[AgentTask]:
    """
    Break a feature description into independent parallel tasks.

    Args:
        description: The feature to build (e.g., "auth system with login, logout, session")
        plan: Optional implementation plan text
        existing_tasks: Optional pre-defined task specs (from tasks config)

    Returns:
        List of AgentTask objects ready for launch_parallel()

    The planning logic:
    1. If existing_tasks provided, use those directly
    2. Otherwise, analyze description + plan to extract components
    3. Create one AgentTask per independent component
    """
    if existing_tasks:
        return [_dict_to_task(t) for t in existing_tasks]

    # Parse description to extract components
    components = _extract_components(description)
    tasks = []

    # Component to agent mapping
    component_map = {
        "models": ("db_schema", "auth/models.py", "Database models and migrations"),
        "api": ("api_endpoint", "auth/routes.py", "REST API endpoints"),
        "middleware": ("middleware", "auth/middleware.py", "Auth middleware and guards"),
        "tests": ("test_suite", "auth/test_suite.py", "Test suite"),
        "frontend": ("frontend", "auth/components/", "UI components"),
        "ui": ("frontend", "auth/components/", "UI components"),
        "docs": ("documentation", "auth/README.md", "Documentation"),
        "validation": ("validation", "auth/validators.py", "Input validation"),
        "services": ("services", "auth/services.py", "Business logic services"),
    }

    plan_summary = plan[:500] if plan else description

    for component in components:
        key = component.lower().strip()
        if key not in component_map:
            # Default component
            task_name = f"team-{key}"
            output_file = f"{key}.output.md"
            summary_file = f"{key}.summary.md"
        else:
            agent_type, output_file, desc = component_map[key]
            task_name = f"team-{key}"

        prompt = _build_agent_prompt(
            task_name=task_name,
            feature=description,
            plan_summary=plan_summary,
            component=component,
            output_file=output_file,
        )

        tasks.append(AgentTask(
            name=task_name,
            description=desc or component,
            prompt=prompt,
            output_file=output_file,
            summary_file=summary_file,
            subagent_type=_get_agent_type(component),
        ))

    return tasks


def _extract_components(description: str) -> List[str]:
    """Extract independent components from a feature description."""
    description = description.lower()

    # Common component patterns
    patterns = {
        "models": ["model", "schema", "database", "migration", "entity"],
        "api": ["api", "endpoint", "route", "rest", "graphql"],
        "middleware": ["middleware", "guard", "auth", "authenticate", "permission"],
        "tests": ["test", "spec", "coverage", "integration test"],
        "frontend": ["frontend", "ui", "component", "page", "screen"],
        "docs": ["doc", "readme", "documentation"],
        "validation": ["validation", "validator", "schema"],
        "services": ["service", "business logic", "usecase"],
    }

    found = set()
    for component, keywords in patterns.items():
        if any(kw in description for kw in keywords):
            found.add(component)

    # If we couldn't detect components, split by "and" or commas
    if not found:
        parts = description.replace(",", " ").replace(" and ", " ").split()
        # Return key words as components
        found = [p.strip() for p in parts if len(p.strip()) > 3][:5]

    return sorted(found) if found else ["default"]


def _get_agent_type(component: str) -> str:
    """Map component to subagent type."""
    mapping = {
        "models": "general-purpose",
        "api": "general-purpose",
        "middleware": "general-purpose",
        "tests": "general-purpose",
        "frontend": "general-purpose",
        "ui": "general-purpose",
        "docs": "Explore",
    }
    return mapping.get(component.lower(), "general-purpose")


def _build_agent_prompt(
    task_name: str,
    feature: str,
    plan_summary: str,
    component: str,
    output_file: str,
) -> str:
    """Build the full agent prompt for a task."""
    return f"""You are {task_name}, a backend engineer on the Harvey OS project.

## Your Task
Build the {component} for the {feature} feature.

## Context
- Feature plan: {plan_summary}
- Working directory: {_HARVEY_HOME}
- Target branch: feature/{feature.replace(" ", "-").lower()}

## Output Contract
Write your implementation to: {{output_dir}}/{output_file}
Summarize results to: {{output_dir}}/{output_file.replace(".output.", ".summary.")}

## What to include in your summary:
- Files created or modified
- Key implementation decisions
- Tests added (if applicable)
- Any assumptions made

## Constraints
- Do NOT commit — write results only
- Follow existing code patterns in the codebase
- If you need to make an assumption, note it clearly

Start immediately. Report when complete."""


def _dict_to_task(d: Dict) -> AgentTask:
    """Convert a dict to an AgentTask."""
    return AgentTask(
        name=d["name"],
        description=d.get("description", ""),
        prompt=d["prompt"],
        output_file=d["output_file"],
        summary_file=d.get("summary_file", d["output_file"].replace(".output.", ".summary.")),
        subagent_type=d.get("subagent_type", "general-purpose"),
        model=d.get("model", ""),
        depends_on=d.get("depends_on", []),
    )


# ─────────────────────────────────────────────────────────────────────────────
# Agent Launching (Parallel)
# ─────────────────────────────────────────────────────────────────────────────

def launch_parallel(tasks: List[AgentTask], output_dir: Path = None) -> List[str]:
    """
    Launch all tasks in parallel as background agents.

    Each agent:
    - Runs as a subprocess with isolated context
    - Writes output to its designated file
    - Is named "team-{task_name}" for tracking

    Args:
        tasks: List of AgentTask objects to launch
        output_dir: Directory for output files

    Returns:
        List of agent IDs (task names) that were launched
    """
    if output_dir is None:
        output_dir = DEFAULT_OUTPUT_DIR
    output_dir.mkdir(parents=True, exist_ok=True)

    launched = []

    for task in tasks:
        if task.status != "pending":
            continue

        # Mark as running
        task.status = "running"
        task.started_at = datetime.utcnow().isoformat() + "Z"
        task.agent_id = task.name

        # Fill in prompt placeholders
        prompt = task.prompt.format(output_dir=output_dir)

        # Spawn subprocess
        pid = _spawn_agent_subprocess(
            task_name=task.name,
            prompt=prompt,
            output_path=output_dir / task.output_file,
            model=task.model or LLM_MODEL,
        )
        task.pid = pid
        launched.append(task.name)

    return launched


def _spawn_agent_subprocess(
    task_name: str,
    prompt: str,
    output_path: Path,
    model: str,
) -> int:
    """
    Spawn a background subprocess that calls the LLM and writes output.

    Returns the subprocess PID.
    """
    # Build the inner Python script that runs in the subprocess
    import openai
    from openai import OpenAI

    script_content = f'''
import sys
sys.path.insert(0, "{str(SKILL_DIR)}")
import json
from pathlib import Path
from openai import OpenAI

output_path = Path("{output_path}")
output_path.parent.mkdir(parents=True, exist_ok=True)

client = OpenAI(api_key="{LLM_API_KEY}", base_url="{LLM_BASE_URL}")

prompt = {json.dumps(prompt)}

try:
    response = client.chat.completions.create(
        model="{model}",
        messages=[{{"role": "user", "content": prompt}}],
        timeout=300,
    )
    result = response.choices[0].message.content
    output_path.write_text(result)
    print(f"Agent {{"{task_name}"}} completed successfully")
except Exception as e:
    error_msg = f"Agent {{"{task_name}"}} failed: {{str(e)}}"
    print(error_msg, file=sys.stderr)
    # Write error to output file too
    output_path.write_text(f"ERROR: {{str(e)}}")
    sys.exit(1)
'''

    proc = subprocess.Popen(
        [sys.executable, "-c", script_content],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={**os.environ, "AGENT_NAME": task_name},
    )

    return proc.pid


# ─────────────────────────────────────────────────────────────────────────────
# Result Collection
# ─────────────────────────────────────────────────────────────────────────────

def wait_for_results(
    tasks: List[AgentTask],
    output_dir: Path = None,
    timeout_ms: int = 600000,
    poll_interval: float = 5.0,
    progress_callback: Callable[[str, str], None] = None,
) -> Dict[str, Any]:
    """
    Wait for all launched agents to complete and collect their results.

    Uses polling (not blocking TaskOutput) — checks agent processes at
    poll_interval seconds until all complete or timeout is reached.

    Args:
        tasks: List of AgentTask objects (must have pid set by launch_parallel)
        output_dir: Directory containing output files
        timeout_ms: Timeout in milliseconds (default 10 minutes)
        poll_interval: How often to check agent status (seconds)
        progress_callback: Optional callback(task_name, status) for progress updates

    Returns:
        Dict mapping task name -> result dict with keys:
        - status: "completed" | "failed" | "timeout"
        - output: the content written to output file
        - error: error message if failed
        - duration_ms: how long the agent ran
    """
    if output_dir is None:
        output_dir = DEFAULT_OUTPUT_DIR

    timeout_sec = timeout_ms / 1000.0
    start_time = time.time()
    results = {}

    # Track running tasks by PID
    running_tasks = {t: t.pid for t in tasks if t.status == "running" and t.pid > 0}

    while running_tasks and (time.time() - start_time) < timeout_sec:
        # Check each running task
        for task, pid in list(running_tasks.items()):
            try:
                # Check if process has terminated
                proc = subprocess.Popen(
                    ["ps", "-p", str(pid)],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
                proc.wait()
                is_running = proc.returncode == 0
            except Exception:
                is_running = True  # Assume running on error

            if not is_running:
                # Process terminated — collect result
                output_file = output_dir / task.output_file
                task.completed_at = datetime.utcnow().isoformat() + "Z"

                if output_file.exists():
                    content = output_file.read_text()
                    if content.startswith("ERROR:"):
                        task.status = "failed"
                        task.error = content[6:].strip()
                        results[task.name] = {
                            "status": "failed",
                            "error": task.error,
                            "output": content,
                        }
                    else:
                        task.status = "completed"
                        task.result = {"output": content}
                        results[task.name] = {
                            "status": "completed",
                            "output": content,
                        }
                else:
                    task.status = "failed"
                    task.error = "Output file not found"
                    results[task.name] = {
                        "status": "failed",
                        "error": "Output file not found",
                    }

                if progress_callback:
                    progress_callback(task.name, task.status)

                del running_tasks[task]

        if running_tasks:
            time.sleep(poll_interval)

    # Handle timeouts
    for task in running_tasks:
        task.status = "failed"
        task.error = f"Timeout after {timeout_sec}s"
        results[task.name] = {
            "status": "timeout",
            "error": f"Timeout after {timeout_sec}s",
        }
        if progress_callback:
            progress_callback(task.name, "timeout")

    return results


def check_agent_status(tasks: List[AgentTask]) -> Dict[str, str]:
    """
    Check current status of running agents.

    Returns:
        Dict mapping task name -> status string
    """
    status = {}
    for task in tasks:
        if task.pid > 0:
            try:
                proc = subprocess.Popen(
                    ["ps", "-p", str(task.pid)],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
                proc.wait()
                is_running = proc.returncode == 0
                status[task.name] = "running" if is_running else task.status
            except Exception:
                status[task.name] = task.status
        else:
            status[task.name] = task.status
    return status


# ─────────────────────────────────────────────────────────────────────────────
# Sprint Management
# ─────────────────────────────────────────────────────────────────────────────

def run_development_sprint(
    feature: str,
    plan: str = "",
    tasks_config: str = None,
    output_dir: Path = None,
    timeout_ms: int = 600000,
) -> Dict[str, Any]:
    """
    Main entry point — run a complete development sprint.

    1. Parse feature + plan -> extract tasks
    2. Launch all tasks in parallel
    3. Wait for completion
    4. Return consolidated results

    Args:
        feature: Feature description (e.g., "auth system with login/logout")
        plan: Implementation plan text
        tasks_config: Optional path to JSON file with pre-defined tasks
        output_dir: Where to store output files
        timeout_ms: Max time to wait for all agents

    Returns:
        Dict with keys:
        - feature: the feature name
        - total_tasks: number of tasks
        - completed: list of completed task names
        - failed: list of failed task names
        - results: full results dict from wait_for_results
    """
    if output_dir is None:
        output_dir = DEFAULT_OUTPUT_DIR / feature.replace(" ", "-").lower()

    # Load tasks config if provided
    existing_tasks = None
    if tasks_config:
        config_path = Path(tasks_config)
        if config_path.exists():
            existing_tasks = json.loads(config_path.read_text()).get("tasks", [])

    # Create sprint
    sprint = Sprint(
        feature_name=feature,
        plan=plan,
        tasks=plan_tasks(feature, plan, existing_tasks),
        output_dir=output_dir,
        created_at=datetime.utcnow().isoformat() + "Z",
    )
    sprint.save_state()

    # Launch all tasks in parallel
    print(f"[DevOrchestrator] Launching {len(sprint.tasks)} agents for: {feature}")
    for task in sprint.tasks:
        print(f"  -> {task.name}: {task.description}")

    launched = launch_parallel(sprint.tasks, output_dir)
    print(f"[DevOrchestrator] Launched {len(launched)} agents")

    # Wait for results
    def progress(name: str, status: str):
        print(f"[DevOrchestrator] {name}: {status}")

    results = wait_for_results(
        sprint.tasks,
        output_dir,
        timeout_ms,
        progress_callback=progress,
    )

    # Update sprint with results
    sprint.completed_at = datetime.utcnow().isoformat() + "Z"
    sprint.save_state()

    # Build summary
    completed = [t.name for t in sprint.tasks if t.status == "completed"]
    failed = [t.name for t in sprint.tasks if t.status == "failed"]

    summary = {
        "feature": feature,
        "total_tasks": len(sprint.tasks),
        "completed": completed,
        "failed": failed,
        "results": results,
    }

    print(f"\n[DevOrchestrator] Sprint complete:")
    print(f"  Completed: {len(completed)}/{len(sprint.tasks)}")
    if failed:
        print(f"  Failed: {', '.join(failed)}")

    return summary


def load_sprint(output_dir: Path) -> Optional[Sprint]:
    """Load an existing sprint from output_dir."""
    return Sprint.load_state(output_dir)


# ─────────────────────────────────────────────────────────────────────────────
# CLI
# ─────────────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Dev Orchestrator — Parallel Development Team",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python3 orchestrate.py --feature "auth system" --plan-file plan.md
  python3 orchestrate.py --feature "checkout flow" --tasks-config tasks.json
  python3 orchestrate.py --feature "user dashboard" --output-dir data/dashboard-sprint
        """,
    )

    parser.add_argument(
        "--feature",
        required=True,
        help="Feature description (e.g., 'auth system with login and logout')",
    )
    parser.add_argument(
        "--plan-file",
        help="Path to implementation plan markdown file",
    )
    parser.add_argument(
        "--tasks-config",
        help="Path to JSON file with pre-defined task specifications",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Output directory for sprint results (default: data/dev-orchestrator/<feature>)",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=600,
        help="Timeout in seconds for all agents (default: 600 = 10 minutes)",
    )
    parser.add_argument(
        "--check-only",
        action="store_true",
        help="Load sprint state and check agent status without launching",
    )

    args = parser.parse_args()

    # Load plan
    plan = ""
    if args.plan_file:
        plan_path = Path(args.plan_file)
        if plan_path.exists():
            plan = plan_path.read_text()

    # Check-only mode
    if args.check_only:
        output_dir = args.output_dir or DEFAULT_OUTPUT_DIR / args.feature.replace(" ", "-").lower()
        sprint = load_sprint(output_dir)
        if not sprint:
            print(f"No sprint found in {output_dir}")
            sys.exit(1)

        print(f"Sprint: {sprint.feature_name}")
        print(f"Created: {sprint.created_at}")
        print("\nTasks:")
        for task in sprint.tasks:
            print(f"  {task.name}: {task.status}")
        sys.exit(0)

    # Run sprint
    result = run_development_sprint(
        feature=args.feature,
        plan=plan,
        tasks_config=args.tasks_config,
        output_dir=args.output_dir,
        timeout_ms=args.timeout * 1000,
    )

    # Exit code based on results
    sys.exit(0 if not result["failed"] else 1)


if __name__ == "__main__":
    main()
