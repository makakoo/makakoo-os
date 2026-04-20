"""Meta Harness Agent — Configuration"""

import os
from pathlib import Path

HARVEY_HOME = Path(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")))
DATA_DIR = HARVEY_HOME / "data" / "meta-harness-agent"
LOG_DIR = DATA_DIR / "logs"
STATE_DIR = DATA_DIR / "state"
RESULTS_TSV = DATA_DIR / "results.tsv"
TBENCH_DATA_DIR = Path(
    os.environ.get("TBENCH_DATA_DIR", str(HARVEY_HOME / "data" / "tbench2"))
)

DATA_DIR.mkdir(parents=True, exist_ok=True)
LOG_DIR.mkdir(parents=True, exist_ok=True)
STATE_DIR.mkdir(parents=True, exist_ok=True)

AI_URL = os.environ.get("SWITCHAI_URL", "http://localhost:18080/v1")
AI_KEY = os.environ.get("SWITCHAI_KEY", "sk-test-123")
AI_MODEL = os.environ.get("LLM_MODEL", "minimax:MiniMax-M2.7")

AGENT_TMUX_SESSION_PREFIX = "mh_agent_"
AGENT_TIMEOUT_SEC = 600
AGENT_MAX_TURNS = 30
AGENT_DEFAULT_DURATION = 1.0
AGENT_SLOW_DURATION = 10.0

HARVEY_SKILLS_ROOT = HARVEY_HOME / "harvey-os" / "skills"
HARVEY_ROOT_GIT = HARVEY_HOME / "harvey-os"
