"""
Harvey Superbrain — Unified Knowledge Layer

Primary: SQLite FTS5 (instant keyword search, zero dependencies)
Optional: Embeddings via switchAILocal/Gemini + cosine similarity
Always: Entity graph from [[wikilinks]], 4-layer memory stack

Usage:
    from core.superbrain.superbrain import Superbrain
    sb = Superbrain()
    result = sb.query("question")

CLI:
    superbrain query "question"
    superbrain search "keywords"
    superbrain context
    superbrain sync
"""

import os

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
