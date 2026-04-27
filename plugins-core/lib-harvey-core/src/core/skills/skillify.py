"""
Skillify — Skill Generator from Natural Language

Generates new SKILL.md files from description + goal + steps.
Powers the /skillify command in Claude Code context.

This is a generator that creates skill documentation, not the skill execution engine.
The output matches Claude Code's skillify.ts output format.

Usage:
    skill_content = generate_skill(
        description="Build a Polymarket arbitrage strategy",
        goal="Generate profitable BTC trading signals",
        steps=[{"name": "Fetch BTC price", "action": "..."}]
    )
    Path("my-skill/SKILL.md").write_text(skill_content)

Path: plugins-core/lib-harvey-core/src/core/skills/skillify.py
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# System Prompt for Skill Generation
# ---------------------------------------------------------------------------

SKILLIFY_SYSTEM_PROMPT = """You are a Skill Generator. Your job is to create SKILL.md files from natural language descriptions.

Given a skill description, goal, and steps, generate a complete SKILL.md file following this format:

---
name: skill-name
description: Brief one-line description of what the skill does
allowed-tools:
  - Tool1
  - Tool2
when_to_use: |
  When to use this skill (1-3 sentences)
argument-hint: [arg1, arg2]  # JSON array of argument names
arguments: |
  - name: arg1
    description: What arg1 is for
    required: true
  - name: arg2
    description: What arg2 is for
    required: false
context: fork  # or "continue" or "solo"
agent: general-purpose  # or "specialized"

## Steps

### 1. Step Name
**Success Criteria:** What defines successful completion of this step

**Execution Mode:** automated  # or "human-checkpoint" or "manual"

**Artifacts:**
  - artifact1.txt  # Files created/modified by this step

**Human Checkpoint:** Optional instruction for human verification

**Rules:**
  - Rule 1 for this step
  - Rule 2 for this step

---

Generate ONLY the SKILL.md content. Start directly with the frontmatter.
Do not include any preamble or explanation.
"""


# ---------------------------------------------------------------------------
# Skill Generation Function
# ---------------------------------------------------------------------------


async def generate_skill(
    description: str,
    goal: str,
    steps: list[dict],
    switchailocal_url: str = "http://localhost:18080/v1",
    model: str = "minimax/MiniMax-M2.7",
) -> str:
    """
    Generate a new SKILL.md from description.

    Args:
        description: Natural language description of the skill
        goal: What the skill aims to achieve
        steps: List of step dicts with 'name', 'action', etc.
        switchailocal_url: URL for switchAI local API
        model: Model to use for generation

    Returns:
        Complete SKILL.md content as string

    Note:
        Does NOT write to disk — returns content for caller to save.
        This is async because it calls the LLM API.
    """
    import httpx

    # Build the prompt with the skill details
    steps_json = json.dumps(steps, indent=2)
    prompt = f"""Generate a SKILL.md for this skill:

Description: {description}
Goal: {goal}
Steps:
{steps_json}

{SKILLIFY_SYSTEM_PROMPT}"""

    # Call the LLM via switchAI local
    messages = [
        {"role": "system", "content": SKILLIFY_SYSTEM_PROMPT},
        {"role": "user", "content": prompt},
    ]

    payload = {
        "model": model,
        "messages": messages,
        "temperature": 0.3,  # Low temperature for deterministic output
        "max_tokens": 2000,
    }

    async with httpx.AsyncClient(timeout=120.0) as client:
        try:
            response = await client.post(
                f"{switchailocal_url}/chat/completions",
                json=payload,
            )
            response.raise_for_status()
            result = response.json()
            skill_md = result["choices"][0]["message"]["content"]
            return skill_md.strip()
        except httpx.HTTPError as e:
            raise RuntimeError(f"Failed to generate skill: {e}")


def generate_skill_sync(
    description: str,
    goal: str,
    steps: list[dict],
    switchailocal_url: str = "http://localhost:18080/v1",
    model: str = "minimax/MiniMax-M2.7",
) -> str:
    """
    Synchronous wrapper for generate_skill.

    For use in non-async contexts.
    """
    import asyncio
    import httpx

    # Build the prompt with the skill details
    steps_json = json.dumps(steps, indent=2)
    prompt = f"""Generate a SKILL.md for this skill:

Description: {description}
Goal: {goal}
Steps:
{steps_json}

{SKILLIFY_SYSTEM_PROMPT}"""

    # Call the LLM via switchAI local
    messages = [
        {"role": "system", "content": SKILLIFY_SYSTEM_PROMPT},
        {"role": "user", "content": prompt},
    ]

    payload = {
        "model": model,
        "messages": messages,
        "temperature": 0.3,
        "max_tokens": 2000,
    }

    try:
        response = httpx.post(
            f"{switchailocal_url}/chat/completions",
            json=payload,
            timeout=120.0,
        )
        response.raise_for_status()
        result = response.json()
        skill_md = result["choices"][0]["message"]["content"]
        return skill_md.strip()
    except httpx.HTTPError as e:
        raise RuntimeError(f"Failed to generate skill: {e}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main():
    import argparse
    import asyncio

    parser = argparse.ArgumentParser(
        description="Generate a new SKILL.md from description"
    )
    parser.add_argument("--description", "-d", required=True, help="Skill description")
    parser.add_argument("--goal", "-g", required=True, help="Skill goal")
    parser.add_argument("--steps", "-s", required=True, help="JSON array of steps")
    parser.add_argument("--output", "-o", help="Output file (default: stdout)")
    parser.add_argument(
        "--url", default="http://localhost:18080/v1", help="switchAI local URL"
    )
    parser.add_argument("--model", default="minimax/MiniMax-M2.7", help="Model to use")

    args = parser.parse_args()

    # Parse steps JSON
    try:
        steps = json.loads(args.steps)
    except json.JSONDecodeError as e:
        print(f"Error: Invalid JSON in steps: {e}", file=sys.stderr)
        sys.exit(1)

    # Generate skill
    print("Generating SKILL.md...", file=sys.stderr)
    skill_md = asyncio.run(
        generate_skill(
            description=args.description,
            goal=args.goal,
            steps=steps,
            switchailocal_url=args.url,
            model=args.model,
        )
    )

    # Output
    if args.output:
        Path(args.output).write_text(skill_md)
        print(f"SKILL.md written to {args.output}", file=sys.stderr)
    else:
        print(skill_md)


if __name__ == "__main__":
    main()
