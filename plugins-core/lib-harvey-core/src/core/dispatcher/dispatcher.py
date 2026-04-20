#!/usr/bin/env python3
"""
Dispatcher — Spawns LLM-powered sub-agents for Harvey OS projects.
Each agent runs via localhost:18080 LLM gateway, writes output to project directory.
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

# Paths
HARVEY_OS = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))) / "harvey-os"
ACTIVE_DIR = HARVEY_OS / "planning" / "ACTIVE"
SKILLS_DIR = HARVEY_OS / "skills"

# LLM config
LLM_BASE_URL = "http://localhost:18080/v1"
LLM_MODEL = os.environ.get("LLM_MODEL", "auto")
LLM_API_KEY = os.environ.get("SWITCHAI_KEY", "")


def load_context():
    """Load Harvey OS context for agent prompts."""
    soul = ""
    agents_md = ""

    soul_path = HARVEY_OS / "SOUL.md"
    if soul_path.exists():
        soul = soul_path.read_text()

    agents_path = HARVEY_OS / "AGENTS.md"
    if agents_path.exists():
        agents_md = agents_path.read_text()

    return soul, agents_md


def load_project_files(project_dir: Path) -> dict:
    """Load all relevant project files for agent context."""
    files = {}
    for name in ["research.md", "plan.md", "implementation.md"]:
        path = project_dir / name
        if path.exists():
            files[name] = path.read_text()[:4000]  # cap at 4k chars each
    return files


def get_agent_prompt(agent_type, project_name, project_md, project_files: dict):
    """Get prompt template for agent type."""
    best_patterns = ""
    design_md = ""
    best_path = HARVEY_OS / "planning" / "KNOWLEDGE" / "best_patterns.md"
    design_path = HARVEY_OS / "orchestration" / "DESIGN.md"
    if best_path.exists():
        best_patterns = best_path.read_text()[:3000]
    if design_path.exists():
        design_md = design_path.read_text()[:2000]

    project_files_md = ""
    if project_files.get("research.md"):
        project_files_md += f"\n## research.md\n{project_files['research.md']}"
    if project_files.get("plan.md"):
        project_files_md += f"\n## plan.md\n{project_files['plan.md']}"
    if project_files.get("implementation.md"):
        project_files_md += f"\n## implementation.md\n{project_files['implementation.md']}"

    system = "You are a text-only assistant. Do NOT use any tools, plugins, or function calls. Output ONLY plain text markdown."

    prompts = {
        "research": f"""{system}

You are a research agent for Harvey OS.

Project: {project_name}
{project_md}

## Harvey OS Best Patterns (condensed)
{best_patterns}

## Harvey OS Orchestration Design (condensed)
{design_md}

Produce a detailed research report covering:
1. What this feature does
2. How similar features work in known projects
3. Recommended implementation approach
4. Potential challenges
5. Dependencies

Output the full research report as your response.""",

        "plan": f"""{system}

You are a planning agent for Harvey OS.

Project: {project_name}
{project_md}

## Research Report (condensed)
{project_files.get('research.md', '[not available]')}

Create a detailed implementation plan with:
1. Phase breakdown (Phase 1, 2, 3...)
2. For each phase: specific files to create/modify
3. Code snippets for key implementations
4. Test strategy
5. Verification steps

Output the full plan as your response.""",

        "execute": f"""{system}

You are an executor agent for Harvey OS.

Project: {project_name}
{project_md}

## Implementation Plan (condensed)
{project_files.get('plan.md', '[not available]')}

Implement the code according to the plan. Create all necessary files.
Write implementation summary to implementation.md.

Output the full implementation summary as your response.""",

        "verify": f"""{system}

You are a verification agent for Harvey OS.

Project: {project_name}
{project_md}

## Implementation (condensed)
{project_files.get('implementation.md', '[not available]')}

## Plan (condensed)
{project_files.get('plan.md', '[not available]')}

Verify the implementation against plan.md.
Write verification report to verification.md with:
- Status: PASS/FAIL
- Issues found (if any)
- Recommendations

Output the full verification report as your response."""
    }
    return prompts.get(agent_type, "")


def update_running_agents(project_name, agent_type, pid=None, status="running", output=None):
    """Update running_agents.json."""
    project_dir = ACTIVE_DIR / project_name
    running_file = project_dir / "running_agents.json"
    data = {}
    if running_file.exists():
        data = json.loads(running_file.read_text())
    data[agent_type] = {"pid": pid, "status": status, "output": output}
    running_file.write_text(json.dumps(data, indent=2))


def run_agent(agent_type, project_name):
    """Dispatch a single agent via LLM."""
    project_dir = ACTIVE_DIR / project_name
    if not project_dir.exists():
        print(f"Error: Project {project_name} not found in ACTIVE/")
        return False

    project_md = ""
    project_md_path = project_dir / "PROJECT.md"
    if project_md_path.exists():
        project_md = project_md_path.read_text()

    project_files = load_project_files(project_dir)

    output_files = {
        "research": "research.md",
        "plan": "plan.md",
        "execute": "implementation.md",
        "verify": "verification.md"
    }
    output_file = output_files.get(agent_type, f"{agent_type}.md")

    update_running_agents(project_name, agent_type, status="running")

    # Build prompt
    prompt = get_agent_prompt(agent_type, project_name, project_md, project_files)

    # Call LLM
    try:
        from openai import OpenAI
        client = OpenAI(api_key=LLM_API_KEY, base_url=LLM_BASE_URL)
        response = client.chat.completions.create(
            model=LLM_MODEL,
            messages=[{"role": "user", "content": prompt}],
            timeout=120
        )
        result = response.choices[0].message.content
    except Exception as e:
        print(f"LLM call failed: {e}")
        update_running_agents(project_name, agent_type, status="failed")
        return False

    # Write output
    output_path = project_dir / output_file
    output_path.write_text(result)
    update_running_agents(project_name, agent_type, status="complete", output=output_file)
    print(f"Agent completed. Output: {output_path}")
    return True


def main():
    parser = argparse.ArgumentParser(description="Dispatch sub-agents for Harvey OS")
    parser.add_argument("--task", required=True,
                        choices=["research", "plan", "execute", "verify"],
                        help="Task type to dispatch")
    parser.add_argument("--project", required=True,
                        help="Project name in planning/ACTIVE/")
    args = parser.parse_args()

    success = run_agent(args.task, args.project)
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
