#!/usr/bin/env python3
"""
Unified Auto-Improve — Uses ALL Harvey's improvement powers on any agent.

When you say "autoimprove trading agent", this orchestrates:
  1. SKILL.md quality (meta-harness behavioral eval)
  2. Strategy parameters (genetic algorithm + backtest)
  3. Knowledge enrichment (Superbrain: past failures, video insights)
  4. Autoresearch (LLM analyzes trade logs, suggests new approaches)
  5. Brain logging (every improvement attempt → journal)

Usage:
    python3 autoimprove.py trading          # full autoimprove on arbitrage-agent
    python3 autoimprove.py career           # full autoimprove on career-manager
    python3 autoimprove.py <agent-name>     # autoimprove any agent
    python3 autoimprove.py trading --only strategy   # just strategy params
    python3 autoimprove.py trading --only skills     # just SKILL.md docs
    python3 autoimprove.py trading --only research   # just autoresearch
"""

import json
import logging
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

# Load .env
env_path = os.path.join(HARVEY_HOME, ".env")
if os.path.exists(env_path):
    with open(env_path) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                os.environ.setdefault(k.strip(), v.strip())

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [autoimprove] %(levelname)s: %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("autoimprove")

# Agent aliases
AGENT_ALIASES = {
    "trading": "arbitrage-agent",
    "trader": "arbitrage-agent",
    "arbitrage": "arbitrage-agent",
    "career": "career-manager",
    "crm": "career-manager",
    "knowledge": "multimodal-knowledge",
    "docs": "multimodal-knowledge",
    "pg": "pg-watchdog",
    "postgres": "pg-watchdog",
    "extractor": "knowledge-extractor",
}

# Skill paths per agent (for SKILL.md improvement)
AGENT_SKILL_PATHS = {
    "arbitrage-agent": ["blockchain/polymarket"],
    "career-manager": ["career/career-manager", "career/linkedin-outreach"],
    "multimodal-knowledge": ["ai-ml/multimodal-knowledge", "research/multimodal-knowledge"],
    "pg-watchdog": ["infrastructure/pg-watchdog"],
    "knowledge-extractor": ["research/knowledge-extraction"],
}


def _write_brain(entry: str):
    """Append to today's Brain journal."""
    today = datetime.now().strftime("%Y_%m_%d")
    journal = Path(HARVEY_HOME, "data", "Brain", "journals", f"{today}.md")
    if not entry.startswith("- "):
        entry = f"- {entry}"
    if not entry.endswith("\n"):
        entry += "\n"
    with open(journal, "a") as f:
        f.write(entry)


def _run_subprocess(args, timeout=300):
    """Run subprocess, return (stdout, stderr, returncode)."""
    try:
        proc = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        return proc.stdout, proc.stderr, proc.returncode
    except subprocess.TimeoutExpired:
        return "", "TIMEOUT", -1
    except Exception as e:
        return "", str(e), -1


# ═══════════════════════════════════════════════════════════════
#  PHASE 1: Knowledge Gathering (Superbrain)
# ═══════════════════════════════════════════════════════════════

def gather_knowledge(agent_name: str) -> dict:
    """Query Superbrain for relevant knowledge about this agent."""
    log.info("Phase 1: Gathering knowledge from Superbrain...")
    knowledge = {"failures": "", "insights": "", "history": ""}

    try:
        from core.superbrain.superbrain import Superbrain
        sb = Superbrain()

        # What went wrong recently?
        failures = sb.query(
            f"What problems or failures happened with {agent_name}? What trades lost money? What didn't work?",
            synthesize=False,
        )
        if failures.sources:
            knowledge["failures"] = "\n".join(
                f"- [{s.source}: {s.title}] {s.text[:300]}" for s in failures.sources[:5]
            )
            log.info("  Found %d failure-related sources", len(failures.sources))

        # What strategies/approaches have worked?
        insights = sb.query(
            f"What strategies or approaches have worked well for {agent_name}? Best practices?",
            synthesize=False,
        )
        if insights.sources:
            knowledge["insights"] = "\n".join(
                f"- [{s.source}: {s.title}] {s.text[:300]}" for s in insights.sources[:5]
            )
            log.info("  Found %d insight sources", len(insights.sources))

        # Video knowledge about self-improvement
        video = sb.query(
            "meta harness self-evolving AI optimization strategy improvement",
            synthesize=False,
        )
        if video.sources:
            knowledge["history"] = "\n".join(
                f"- [{s.source}: {s.title}] {s.text[:300]}" for s in video.sources[:3]
            )
            log.info("  Found %d video/research sources", len(video.sources))

    except Exception as e:
        log.warning("  Superbrain unavailable: %s — continuing without knowledge", e)

    return knowledge


# ═══════════════════════════════════════════════════════════════
#  PHASE 2: Strategy Parameter Evolution (for trading agents)
# ═══════════════════════════════════════════════════════════════

def improve_strategy_params(agent_name: str, knowledge: dict) -> dict:
    """Run genetic algorithm + LLM-guided strategy evolution."""
    log.info("Phase 2: Evolving strategy parameters...")

    agent_dir = Path(HARVEY_HOME, "agents", agent_name)
    autoimprove_script = agent_dir / "autoimprove.py"

    if not autoimprove_script.exists():
        log.info("  No autoimprove.py found for %s — skipping strategy evolution", agent_name)
        return {"status": "skipped", "reason": "no autoimprove.py"}

    # Run the agent's own autoimprove
    log.info("  Running %s/autoimprove.py...", agent_name)
    stdout, stderr, rc = _run_subprocess(
        [sys.executable, str(autoimprove_script)], timeout=120
    )

    if rc != 0:
        log.error("  Strategy evolution failed: %s", stderr[:200])
        return {"status": "error", "error": stderr[:200]}

    # Parse results from stdout
    log.info("  Strategy evolution output:\n%s", stdout[-500:] if stdout else "(empty)")

    # Check if best_params was updated
    params_file = agent_dir / "state" / "best_intraday_params.json"
    if not params_file.exists():
        params_file = Path(HARVEY_HOME, "data", "arbitrage-agent", "v2", "state", "best_intraday_params.json")

    result = {"status": "completed", "output": stdout[-300:]}
    if params_file.exists():
        try:
            params = json.loads(params_file.read_text())
            result["params"] = params.get("params", {})
            result["timestamp"] = params.get("timestamp", "")
            log.info("  Best params updated: %s", json.dumps(result["params"], indent=2)[:200])
        except Exception:
            pass

    return result


# ═══════════════════════════════════════════════════════════════
#  PHASE 3: SKILL.md Quality Improvement (meta-harness eval)
# ═══════════════════════════════════════════════════════════════

def improve_skill_docs(agent_name: str, knowledge: dict) -> dict:
    """Improve SKILL.md docs using meta-harness evaluation."""
    log.info("Phase 3: Improving SKILL.md documentation...")

    skill_paths = AGENT_SKILL_PATHS.get(agent_name, [])
    if not skill_paths:
        log.info("  No skill paths configured for %s — skipping", agent_name)
        return {"status": "skipped", "reason": "no skill paths"}

    eval_script = Path(HARVEY_HOME, "harvey-os", "skills", "meta", "autoimprover", "evaluate_skill.py")
    if not eval_script.exists():
        log.warning("  evaluate_skill.py not found — skipping SKILL.md improvement")
        return {"status": "skipped", "reason": "evaluate_skill.py missing"}

    results = []
    for skill_rel in skill_paths:
        skill_file = Path(HARVEY_HOME, "harvey-os", "skills", skill_rel, "SKILL.md")
        if not skill_file.exists():
            log.info("  Skill not found: %s — skipping", skill_rel)
            continue

        log.info("  Evaluating: %s", skill_rel)
        stdout, stderr, rc = _run_subprocess(
            [sys.executable, str(eval_script), skill_rel], timeout=300
        )

        if rc == 0:
            results.append({"skill": skill_rel, "status": "improved", "output": stdout[-200:]})
            log.info("  %s: improved", skill_rel)
        else:
            results.append({"skill": skill_rel, "status": "no-improvement", "output": stdout[-200:]})
            log.info("  %s: no improvement (rc=%d)", skill_rel, rc)

    return {"status": "completed", "skills": results}


# ═══════════════════════════════════════════════════════════════
#  PHASE 4: Autoresearch (LLM-driven analysis + new ideas)
# ═══════════════════════════════════════════════════════════════

def run_autoresearch(agent_name: str, knowledge: dict) -> dict:
    """LLM analyzes agent's performance and suggests improvements."""
    log.info("Phase 4: Running autoresearch...")

    import requests

    base_url = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1")
    api_key = os.environ.get("LLM_API_KEY", os.environ.get("SWITCHAI_KEY", ""))
    model = os.environ.get("LLM_MODEL", "auto")

    prompt = f"""You are analyzing the {agent_name} agent for Harvey OS.

## Recent Failures
{knowledge.get('failures', 'No failure data available.')}

## What Has Worked
{knowledge.get('insights', 'No insight data available.')}

## Research Context
{knowledge.get('history', 'No research context available.')}

## Task
Based on the above, suggest 3 concrete improvements for {agent_name}:
1. A parameter change (specific numbers)
2. A strategy change (new approach or rule)
3. A risk management improvement

Be specific. Return actionable recommendations with exact values where possible.
Format each as: RECOMMENDATION: <title>\nDETAIL: <explanation>\nACTION: <what to change>"""

    try:
        resp = requests.post(
            f"{base_url}/chat/completions",
            headers={"Authorization": f"Bearer {api_key}"},
            json={
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.4,
                "max_tokens": 1500,
            },
            timeout=60,
        )
        if resp.status_code == 200:
            suggestions = resp.json()["choices"][0]["message"]["content"]
            log.info("  Autoresearch suggestions:\n%s", suggestions[:500])
            return {"status": "completed", "suggestions": suggestions}
        else:
            log.error("  LLM call failed: %d", resp.status_code)
            return {"status": "error", "error": f"LLM {resp.status_code}"}
    except Exception as e:
        log.error("  Autoresearch failed: %s", e)
        return {"status": "error", "error": str(e)}


# ═══════════════════════════════════════════════════════════════
#  ORCHESTRATOR
# ═══════════════════════════════════════════════════════════════

def autoimprove(agent_name: str, phases: list = None) -> dict:
    """
    Full autoimprove cycle using ALL available improvement powers.

    Phases:
      1. knowledge  — Query Superbrain for past failures + insights
      2. strategy   — Genetic algorithm + backtest (trading agents)
      3. skills     — SKILL.md quality improvement (meta-harness)
      4. research   — LLM autoresearch (new ideas + recommendations)
      5. log        — Write everything to Brain journal
    """
    all_phases = ["knowledge", "strategy", "skills", "research"]
    phases = phases or all_phases

    log.info("=" * 60)
    log.info("AUTOIMPROVE: %s", agent_name)
    log.info("Phases: %s", ", ".join(phases))
    log.info("=" * 60)

    start = time.time()
    results = {"agent": agent_name, "phases": {}}

    # Phase 1: Knowledge gathering
    knowledge = {}
    if "knowledge" in phases:
        knowledge = gather_knowledge(agent_name)
        results["phases"]["knowledge"] = {
            "failures": len(knowledge.get("failures", "").split("\n")),
            "insights": len(knowledge.get("insights", "").split("\n")),
        }

    # Phase 2: Strategy evolution
    if "strategy" in phases:
        results["phases"]["strategy"] = improve_strategy_params(agent_name, knowledge)

    # Phase 3: SKILL.md improvement
    if "skills" in phases:
        results["phases"]["skills"] = improve_skill_docs(agent_name, knowledge)

    # Phase 4: Autoresearch
    if "research" in phases:
        results["phases"]["research"] = run_autoresearch(agent_name, knowledge)

    elapsed = time.time() - start
    results["elapsed_sec"] = round(elapsed, 1)

    # Phase 5: Log to Brain (always)
    phase_summary = []
    for phase, data in results["phases"].items():
        status = data.get("status", "unknown")
        phase_summary.append(f"{phase}: {status}")

    _write_brain(
        f"- [[autoimprove]] ran on [[{agent_name}]] ({elapsed:.0f}s): "
        + " | ".join(phase_summary)
    )

    if "research" in results["phases"]:
        research = results["phases"]["research"]
        if research.get("suggestions"):
            _write_brain(
                f"  - Autoresearch suggestions for [[{agent_name}]]:\n"
                + "\n".join(f"    {line}" for line in research["suggestions"].split("\n")[:10])
            )

    log.info("=" * 60)
    log.info("AUTOIMPROVE COMPLETE: %s (%.1fs)", agent_name, elapsed)
    for phase, data in results["phases"].items():
        log.info("  %s: %s", phase, data.get("status", "unknown"))
    log.info("=" * 60)

    return results


# ═══════════════════════════════════════════════════════════════
#  CLI
# ═══════════════════════════════════════════════════════════════

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] == "--help":
        print("Usage:")
        print("  python3 autoimprove.py trading              # all phases on arbitrage-agent")
        print("  python3 autoimprove.py career               # all phases on career-manager")
        print("  python3 autoimprove.py trading --only strategy  # just strategy evolution")
        print("  python3 autoimprove.py trading --only skills    # just SKILL.md improvement")
        print("  python3 autoimprove.py trading --only research  # just autoresearch")
        print()
        print("Aliases: trading/trader/arbitrage, career/crm, knowledge/docs, pg/postgres")
        sys.exit(0)

    agent_input = sys.argv[1]
    agent_name = AGENT_ALIASES.get(agent_input, agent_input)

    # Check agent exists
    agent_dir = Path(HARVEY_HOME, "agents", agent_name)
    if not agent_dir.exists():
        print(f"Error: Agent not found: {agent_dir}")
        print(f"Available agents: {', '.join(os.listdir(Path(HARVEY_HOME, 'agents')))}")
        sys.exit(1)

    # Parse --only flag
    phases = None
    if "--only" in sys.argv:
        idx = sys.argv.index("--only")
        if idx + 1 < len(sys.argv):
            phases = [sys.argv[idx + 1]]

    results = autoimprove(agent_name, phases=phases)

    # Print summary
    print("\n" + json.dumps(results, indent=2, default=str))
