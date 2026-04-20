"""Superbrain configuration — all tunables in one place."""

import os

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

# ── PostgreSQL (legacy — harvey_brain has zero tables, all data in SQLite)
# Kept for reference; not used by core superbrain. Remove after cleanup.
PG_HOST = os.environ.get("POSTGRES_HOST", "localhost")
PG_PORT = int(os.environ.get("POSTGRES_PORT", "5432"))
PG_DB = os.environ.get("POSTGRES_DB", "harvey_brain")
PG_USER = os.environ.get("POSTGRES_USER", "postgres")
PG_PASSWORD = os.environ.get("POSTGRES_PASSWORD", "")

# ── Qdrant (only used by multimodal-knowledge agent, not core superbrain)
QDRANT_HOST = os.environ.get("QDRANT_HOST", "localhost")
QDRANT_PORT = int(os.environ.get("QDRANT_PORT", "6333"))
QDRANT_COLLECTION = "multimodal"

# ── Embeddings (text) — switchAILocal, local, no external API ──
EMBEDDING_MODEL = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:0.6b")
# Dim is auto-detected from first embedding response. No hardcoding.

# ── Gemini (ONLY for multimodal/OCR agent — NOT core superbrain) ──
GEMINI_API_KEY = os.environ.get("GEMINI_API_KEY", "")
GEMINI_EMBED_MODEL = "gemini-embedding-001"
GEMINI_EMBED_URL = "https://generativelanguage.googleapis.com/v1beta/models/{model}:embedContent"
GEMINI_EMBEDDING_DIM = 3072  # multimodal OCR only

# ── switchAILocal (for entity extraction / synthesis) ───────
LLM_BASE_URL = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1")
LLM_API_KEY = os.environ.get("LLM_API_KEY", os.environ.get("SWITCHAI_KEY", ""))
LLM_MODEL = os.environ.get("LLM_MODEL", "auto")
# Synthesis model: fast, clean output (no reasoning chain overhead)
LLM_SYNTHESIS_MODEL = os.environ.get("LLM_SYNTHESIS_MODEL", "auto:fast")

# ── Brain paths ─────────────────────────────────────────────
BRAIN_DIR = os.path.join(HARVEY_HOME, "data", "Brain")
BRAIN_PAGES_DIR = os.path.join(BRAIN_DIR, "pages")
BRAIN_JOURNALS_DIR = os.path.join(BRAIN_DIR, "journals")

# ── Query tuning ────────────────────────────────────────────
QUERY_TOP_K = 10                    # results per system
QUERY_BRAIN_AUTHORITY_BOOST = 1.2   # Brain results score 20% higher
QUERY_JOURNAL_RECENCY_BOOST = 1.1   # recent journals score 10% higher
QUERY_MIN_SCORE = 0.3               # drop results below this similarity

# ── Ingestion tuning ───────────────────────────────────────
INGEST_MIN_TEXT_LENGTH = 50          # reject chunks with < 50 chars
INGEST_BATCH_SIZE = 20               # embed N items per API call

# ── Load .env if keys are missing ──────────────────────────
def _load_env():
    """Load from HARVEY_HOME/.env if env vars are missing."""
    env_path = os.path.join(HARVEY_HOME, ".env")
    if not os.path.exists(env_path):
        return
    with open(env_path) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, val = line.split("=", 1)
                key, val = key.strip(), val.strip()
                if key and not os.environ.get(key):
                    os.environ[key] = val

_load_env()

# Re-read after .env load
if not GEMINI_API_KEY:
    GEMINI_API_KEY = os.environ.get("GEMINI_API_KEY", "")
if not LLM_API_KEY:
    LLM_API_KEY = os.environ.get("LLM_API_KEY", os.environ.get("SWITCHAI_KEY", ""))
