#!/usr/bin/env python3
"""
Harvey Agent Scaffolder — Create new agents with proper structure.

When a user says "Harvey, add a weather agent" or "create a new agent
that monitors my portfolio", this module scaffolds the correct directory
structure, boilerplate code, and manifest.

Usage:
    from core.agents.scaffold import scaffold_agent

    scaffold_agent(
        name="weather-monitor",
        description="Monitor weather forecasts and alert on severe conditions",
        pattern="cron",           # daemon | cron | cli
        interval="30m",           # for cron pattern
    )

    # Or via CLI:
    superbrain agent create weather-monitor --description "..." --pattern cron --interval 30m
    superbrain agent list
    superbrain agent info weather-monitor
"""

import json
import os
import stat
import textwrap
import time
from datetime import datetime
from pathlib import Path
from typing import Optional

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
AGENTS_DIR = Path(HARVEY_HOME) / "agents"
DATA_DIR = Path(HARVEY_HOME) / "data"


def scaffold_agent(
    name: str,
    description: str = "",
    pattern: str = "cli",       # daemon | cron | cli
    interval: str = "",         # e.g. "30m", "1h", "daily"
    dependencies: str = "",     # e.g. "requests, httpx"
) -> dict:
    """
    Create a new agent with proper Harvey OS structure.

    Creates:
      agents/{name}/
        agent.py          — Main entry point
        AGENT.md          — Manifest (description, pattern, requirements)
        requirements.txt  — Python dependencies (if any)
      data/{name}/         — State directory (empty, ready for use)

    Returns dict with created paths and next steps.
    """
    agent_dir = AGENTS_DIR / name
    state_dir = DATA_DIR / name

    if agent_dir.exists():
        return {"error": f"Agent '{name}' already exists at {agent_dir}"}

    # Create directories
    agent_dir.mkdir(parents=True, exist_ok=True)
    state_dir.mkdir(parents=True, exist_ok=True)

    # Generate agent.py based on pattern
    if pattern == "daemon":
        agent_code = _daemon_template(name, description)
    elif pattern == "cron":
        agent_code = _cron_template(name, description, interval)
    else:
        agent_code = _cli_template(name, description)

    agent_path = agent_dir / "agent.py"
    agent_path.write_text(agent_code)
    agent_path.chmod(agent_path.stat().st_mode | stat.S_IEXEC)

    # Generate AGENT.md manifest
    manifest = _manifest_template(name, description, pattern, interval, dependencies)
    (agent_dir / "AGENT.md").write_text(manifest)

    # Generate requirements.txt if dependencies specified
    if dependencies:
        deps = [d.strip() for d in dependencies.split(",")]
        (agent_dir / "requirements.txt").write_text("\n".join(deps) + "\n")

    # Return summary
    result = {
        "name": name,
        "agent_dir": str(agent_dir),
        "state_dir": str(state_dir),
        "pattern": pattern,
        "files_created": [
            str(agent_path),
            str(agent_dir / "AGENT.md"),
        ],
        "next_steps": [],
    }

    if pattern == "daemon":
        result["next_steps"] = [
            f"Edit {agent_path} to implement your agent logic",
            f"Test: python3 {agent_path}",
            f"Run as daemon: nohup python3 {agent_path} >> {DATA_DIR / name / 'agent.log'} 2>&1 &",
        ]
    elif pattern == "cron":
        cron_expr = _interval_to_cron(interval)
        result["next_steps"] = [
            f"Edit {agent_path} to implement your agent logic",
            f"Test: python3 {agent_path}",
            f"Add to cron: {cron_expr} python3 {agent_path}",
        ]
        result["cron_expression"] = cron_expr
    else:
        result["next_steps"] = [
            f"Edit {agent_path} to implement your agent logic",
            f"Run: python3 {agent_path}",
        ]

    return result


def install_agent(source: str, name: str = None) -> dict:
    """
    Install an agent from a GitHub URL or local path.

    The URL IS the manifest. No registry, no framework.

    Usage:
        install_agent("https://github.com/traylinx/career-manager")
        install_agent("https://github.com/user/my-agent", name="custom-name")
        install_agent("/path/to/local/agent")

    Does:
        1. git clone into ~/MAKAKOO/agents/<repo-name>
        2. pip install -r requirements.txt (if exists)
        3. mkdir data/<repo-name> for state
        4. Done — agent auto-discovered by registry
    """
    import re
    import subprocess

    # Parse source
    is_url = source.startswith("http://") or source.startswith("https://") or source.startswith("git@")
    is_local = os.path.isdir(source)

    if not is_url and not is_local:
        return {"error": f"Source must be a GitHub URL or local directory path: {source}"}

    # Extract agent name from URL or path
    if name is None:
        if is_url:
            # https://github.com/user/repo-name.git → repo-name
            name = source.rstrip("/").rstrip(".git").split("/")[-1]
        else:
            name = os.path.basename(os.path.abspath(source))

    # Sanitize name
    name = re.sub(r'[^a-zA-Z0-9_-]', '-', name).strip('-').lower()
    if not name:
        return {"error": "Could not determine agent name from source"}

    agent_dir = AGENTS_DIR / name
    state_dir = DATA_DIR / name

    if agent_dir.exists():
        return {"error": f"Agent '{name}' already exists at {agent_dir}. Use 'superbrain agent uninstall {name}' first."}

    # Ensure agents dir exists
    AGENTS_DIR.mkdir(parents=True, exist_ok=True)

    # Clone or copy
    if is_url:
        try:
            result = subprocess.run(
                ["git", "clone", "--depth", "1", source, str(agent_dir)],
                capture_output=True, text=True, timeout=120,
            )
            if result.returncode != 0:
                return {"error": f"git clone failed: {result.stderr.strip()}"}
        except subprocess.TimeoutExpired:
            return {"error": "git clone timed out (120s)"}
        except FileNotFoundError:
            return {"error": "git not found. Install git first."}
    else:
        # Local path — symlink it
        os.symlink(os.path.abspath(source), str(agent_dir))

    # Create state directory
    state_dir.mkdir(parents=True, exist_ok=True)

    # Install dependencies
    deps_installed = False
    for req_file in ["requirements.txt", "requirements.in", "pyproject.toml"]:
        req_path = agent_dir / req_file
        if req_path.exists():
            if req_file == "pyproject.toml":
                cmd = ["pip3", "install", "-e", str(agent_dir), "--quiet"]
            else:
                cmd = ["pip3", "install", "-r", str(req_path), "--quiet"]
            try:
                subprocess.run(cmd, capture_output=True, text=True, timeout=300)
                deps_installed = True
            except Exception:
                pass  # Non-fatal — user can install manually
            break

    # Detect agent info
    has_agent_md = (agent_dir / "AGENT.md").exists()
    has_skill_md = (agent_dir / "SKILL.md").exists()
    has_readme = (agent_dir / "README.md").exists()
    has_entry = any((agent_dir / f).exists() for f in ["agent.py", "main.py", "__main__.py"])

    return {
        "name": name,
        "source": source,
        "agent_dir": str(agent_dir),
        "state_dir": str(state_dir),
        "deps_installed": deps_installed,
        "has_manifest": has_agent_md or has_skill_md,
        "has_readme": has_readme,
        "has_entry": has_entry,
    }


def uninstall_agent(name: str, keep_data: bool = False) -> dict:
    """
    Uninstall an agent. Removes code, optionally keeps state data.

    Usage:
        uninstall_agent("weather-monitor")
        uninstall_agent("weather-monitor", keep_data=True)
    """
    import shutil

    agent_dir = AGENTS_DIR / name
    state_dir = DATA_DIR / name

    if not agent_dir.exists():
        return {"error": f"Agent '{name}' not found at {agent_dir}"}

    # Remove agent code
    if agent_dir.is_symlink():
        agent_dir.unlink()
    else:
        shutil.rmtree(agent_dir)

    # Remove state data (unless keep_data)
    data_removed = False
    if not keep_data and state_dir.exists():
        shutil.rmtree(state_dir)
        data_removed = True

    return {
        "name": name,
        "code_removed": True,
        "data_removed": data_removed,
        "data_kept": keep_data and state_dir.exists(),
    }


def list_agents() -> list:
    """List all installed agents with their status."""
    agents = []
    if not AGENTS_DIR.exists():
        return agents

    for d in sorted(AGENTS_DIR.iterdir()):
        if not d.is_dir() or d.name.startswith("."):
            continue

        agent = {"name": d.name, "path": str(d)}

        # Read AGENT.md if exists
        manifest_path = d / "AGENT.md"
        if manifest_path.exists():
            content = manifest_path.read_text(encoding="utf-8", errors="replace")
            # Parse basic metadata from markdown
            for line in content.split("\n"):
                stripped = line.strip()
                if stripped.startswith("**Pattern:**"):
                    agent["pattern"] = stripped.split("**Pattern:**")[1].strip()
                elif stripped.startswith("**Description:**"):
                    agent["description"] = stripped.split("**Description:**")[1].strip()

        # Check if agent.py exists
        agent["has_entry"] = (d / "agent.py").exists()

        # Check if state dir exists
        state_dir = DATA_DIR / d.name
        agent["has_state"] = state_dir.exists()

        # Check if running (simple PID check)
        pid_file = state_dir / "agent.pid"
        if pid_file.exists():
            try:
                pid = int(pid_file.read_text().strip())
                os.kill(pid, 0)  # Check if process exists
                agent["status"] = "running"
            except (ValueError, ProcessLookupError, PermissionError):
                agent["status"] = "stopped"
        else:
            agent["status"] = "installed"

        if not agent.get("description"):
            agent["description"] = "(no AGENT.md)"

        agents.append(agent)

    return agents


def agent_info(name: str) -> str:
    """Get detailed info about an agent."""
    agent_dir = AGENTS_DIR / name
    if not agent_dir.exists():
        return f"Agent '{name}' not found. Run: superbrain agent list"

    manifest_path = agent_dir / "AGENT.md"
    if manifest_path.exists():
        return manifest_path.read_text(encoding="utf-8", errors="replace")

    # Fallback: list files
    files = list(agent_dir.rglob("*"))
    return f"Agent: {name}\nPath: {agent_dir}\nFiles: {len(files)}\n" + \
           "\n".join(f"  {f.relative_to(agent_dir)}" for f in files[:20])


# ═══════════════════════════════════════════════════════════════
#  Templates
# ═══════════════════════════════════════════════════════════════

def _daemon_template(name: str, description: str) -> str:
    return textwrap.dedent(f'''\
        #!/usr/bin/env python3
        """
        {name} — {description or "Harvey OS Agent (daemon)"}

        Runs continuously, polling at a regular interval.
        State stored in $HARVEY_HOME/data/{name}/
        """

        import json
        import logging
        import os
        import signal
        import sys
        import time
        from datetime import datetime
        from pathlib import Path

        HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        STATE_DIR = Path(HARVEY_HOME) / "data" / "{name}"
        STATE_DIR.mkdir(parents=True, exist_ok=True)

        LOG_FILE = STATE_DIR / "agent.log"
        PID_FILE = STATE_DIR / "agent.pid"

        logging.basicConfig(
            level=logging.INFO,
            format="%(asctime)s [{name}] %(message)s",
            handlers=[
                logging.FileHandler(LOG_FILE),
                logging.StreamHandler(),
            ],
        )
        log = logging.getLogger("{name}")

        POLL_INTERVAL = 60  # seconds


        def setup():
            """Initialize agent. Runs once at startup."""
            PID_FILE.write_text(str(os.getpid()))
            signal.signal(signal.SIGTERM, lambda *_: shutdown())
            log.info("Agent started (PID %d)", os.getpid())


        def tick():
            """
            Main work function. Called every POLL_INTERVAL seconds.

            TODO: Implement your agent logic here.
            """
            log.info("Tick — implement your logic here")


        def shutdown():
            """Clean shutdown."""
            log.info("Agent shutting down")
            PID_FILE.unlink(missing_ok=True)
            sys.exit(0)


        def main():
            setup()
            try:
                while True:
                    tick()
                    time.sleep(POLL_INTERVAL)
            except KeyboardInterrupt:
                shutdown()


        if __name__ == "__main__":
            main()
    ''')


def _cron_template(name: str, description: str, interval: str) -> str:
    return textwrap.dedent(f'''\
        #!/usr/bin/env python3
        """
        {name} — {description or "Harvey OS Agent (scheduled)"}

        Runs on a schedule (cron). Each invocation does one unit of work.
        State stored in $HARVEY_HOME/data/{name}/
        Schedule: {interval or "configure via cron"}
        """

        import json
        import logging
        import os
        from datetime import datetime
        from pathlib import Path

        HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        STATE_DIR = Path(HARVEY_HOME) / "data" / "{name}"
        STATE_DIR.mkdir(parents=True, exist_ok=True)

        logging.basicConfig(
            level=logging.INFO,
            format="%(asctime)s [{name}] %(message)s",
        )
        log = logging.getLogger("{name}")


        def run():
            """
            Main work function. Called once per cron invocation.

            TODO: Implement your agent logic here.
            """
            log.info("Running scheduled task")

            # Example: load previous state
            state_file = STATE_DIR / "state.json"
            state = {{}}
            if state_file.exists():
                state = json.loads(state_file.read_text())

            # TODO: Do your work here
            state["last_run"] = datetime.now().isoformat()
            state["run_count"] = state.get("run_count", 0) + 1

            # Save state
            state_file.write_text(json.dumps(state, indent=2))
            log.info("Done (run #%d)", state["run_count"])


        if __name__ == "__main__":
            run()
    ''')


def _cli_template(name: str, description: str) -> str:
    return textwrap.dedent(f'''\
        #!/usr/bin/env python3
        """
        {name} — {description or "Harvey OS Agent (CLI tool)"}

        Manual invocation for on-demand tasks.
        State stored in $HARVEY_HOME/data/{name}/
        """

        import argparse
        import json
        import logging
        import os
        from pathlib import Path

        HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
        STATE_DIR = Path(HARVEY_HOME) / "data" / "{name}"
        STATE_DIR.mkdir(parents=True, exist_ok=True)

        logging.basicConfig(level=logging.INFO, format="%(asctime)s [{name}] %(message)s")
        log = logging.getLogger("{name}")


        def main():
            parser = argparse.ArgumentParser(description="{description or name}")
            parser.add_argument("command", nargs="?", default="run", help="Command to execute")
            parser.add_argument("--input", "-i", help="Input data")
            args = parser.parse_args()

            if args.command == "run":
                # TODO: Implement your agent logic here
                log.info("Running — implement your logic here")
            elif args.command == "status":
                state_file = STATE_DIR / "state.json"
                if state_file.exists():
                    print(state_file.read_text())
                else:
                    print("No state yet.")
            else:
                print(f"Unknown command: {{args.command}}")


        if __name__ == "__main__":
            main()
    ''')


def _manifest_template(name: str, description: str, pattern: str,
                       interval: str, dependencies: str) -> str:
    cron_line = ""
    if pattern == "cron" and interval:
        cron_expr = _interval_to_cron(interval)
        cron_line = f"\n**Cron:** `{cron_expr} python3 $HARVEY_HOME/agents/{name}/agent.py`\n"

    return textwrap.dedent(f'''\
        # {name}

        **Description:** {description or "A Harvey OS agent."}

        **Pattern:** {pattern}
        {cron_line}
        ## What It Does

        {description or "TODO: Describe what this agent does."}

        ## How to Run

        ```bash
        python3 $HARVEY_HOME/agents/{name}/agent.py
        ```

        ## State & Data

        State stored in `$HARVEY_HOME/data/{name}/`

        ## Requirements

        {("- " + chr(10).join("- " + d.strip() for d in dependencies.split(","))) if dependencies else "- Python 3.10+"}

        ## Brain Integration

        Log significant actions to the Brain journal:
        ```python
        from pathlib import Path
        from datetime import date

        journal = Path(HARVEY_HOME) / "data/Brain/journals" / f"{{date.today().strftime('%Y_%m_%d')}}.md"
        with open(journal, "a") as f:
            f.write(f"- [[{name}]] completed task: {{result}}\\n")
        ```
    ''')


def _interval_to_cron(interval: str) -> str:
    """Convert human interval to cron expression."""
    interval = interval.lower().strip()
    if interval in ("1m", "every minute"):
        return "* * * * *"
    elif interval in ("5m", "5min"):
        return "*/5 * * * *"
    elif interval in ("10m", "10min"):
        return "*/10 * * * *"
    elif interval in ("15m", "15min"):
        return "*/15 * * * *"
    elif interval in ("30m", "30min"):
        return "*/30 * * * *"
    elif interval in ("1h", "hourly"):
        return "0 * * * *"
    elif interval in ("2h"):
        return "0 */2 * * *"
    elif interval in ("6h"):
        return "0 */6 * * *"
    elif interval in ("12h"):
        return "0 */12 * * *"
    elif interval in ("daily", "24h"):
        return "0 6 * * *"
    elif interval in ("weekly"):
        return "0 6 * * 1"
    else:
        return f"# TODO: set cron for interval '{interval}'"
