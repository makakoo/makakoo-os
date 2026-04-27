#!/usr/bin/env python3
"""
SANCHO Built-in Handlers — Plain functions returning result dicts.

Each handler is a standalone function that does one thing and returns
a summary dict. Handlers import their dependencies lazily to avoid
circular imports and keep SANCHO bootable even when subsystems are down.

Handlers:
  - handle_dream:                Run the DreamEngine memory consolidation cycle
  - handle_wiki_lint:            Lint the Brain wiki for orphans, missing pages, etc.
  - handle_index_rebuild:        Rebuild the Brain wiki index
  - handle_daily_briefing:       Summarize today's journal entries
  - handle_dynamic_checklist:    Reactive HEARTBEAT.md evaluator (H2/H4)
"""

import hashlib
import json
import logging
import os
import re
from datetime import datetime
from pathlib import Path
from typing import Callable, Dict, Optional

log = logging.getLogger("harvey.sancho.handlers")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))

HEARTBEAT_PATH = Path(HARVEY_HOME) / "HEARTBEAT.md"
HEARTBEAT_STATE_PATH = Path(HARVEY_HOME) / "data" / "sancho_heartbeat_state.json"
HEARTBEAT_OK_TOKEN = "HEARTBEAT_OK"
HEARTBEAT_LLM_BASE_URL = os.environ.get("AIL_BASE_URL", "http://localhost:18080/v1")
HEARTBEAT_LLM_MODEL = os.environ.get("SANCHO_HEARTBEAT_MODEL", "auto")
HEARTBEAT_LLM_TIMEOUT = int(os.environ.get("SANCHO_HEARTBEAT_TIMEOUT", "60"))


def handle_dream() -> Dict:
    """
    Run the DreamEngine memory consolidation cycle.

    Delegates to DreamEngine.dream() and returns a summary of what
    was consolidated, created, and pruned.
    """
    from core.dreams.consolidator import DreamEngine

    engine = DreamEngine()
    report = engine.dream()

    return {
        "pages_updated": report.pages_updated,
        "pages_created": report.pages_created,
        "pages_pruned": report.pages_pruned,
        "orphans_found": report.orphans_found,
        "conflicts_resolved": report.conflicts_resolved,
        "duration_sec": report.duration_sec,
        "timestamp": report.timestamp,
    }


def handle_wiki_lint() -> Dict:
    """
    Run wiki lint checks and return a health summary.

    Checks for orphan pages, missing pages, empty pages, stale journals,
    and hub pages.
    """
    from core.superbrain.wiki import WikiOps

    wiki = WikiOps()
    report = wiki.lint()

    # Build a concise summary from the lint report
    summary = {}
    for key in ("orphan_pages", "missing_pages", "empty_pages", "dead_links", "hub_pages"):
        items = report.get(key, [])
        summary[key] = len(items) if isinstance(items, list) else items

    summary["health"] = report.get("health", "unknown")
    return summary


def handle_index_rebuild() -> Dict:
    """
    Rebuild the Brain wiki index.

    Generates a categorized, linkable index of all Brain pages
    with summaries and link counts.
    """
    from core.superbrain.wiki import WikiOps

    wiki = WikiOps()
    index_content = wiki.build_index()

    # Count pages from the generated index
    page_count = index_content.count("[[") if index_content else 0

    return {
        "pages_indexed": page_count,
        "index_length": len(index_content) if index_content else 0,
        "status": "rebuilt",
    }


def handle_daily_briefing() -> Dict:
    """
    Read today's journal and return a quick summary.

    Counts entries, extracts entity references, runs the mascot patrol to
    surface any critical quality signals alongside the daily journal
    snapshot. The mascot aggregate is the whole point of "proactive" —
    if Pixel sees daemon errors or Cinder sees syntax failures, the
    daily briefing is where Sebastian notices.

    The mascot patrol is best-effort; failures never block the briefing.
    """
    today = datetime.now().strftime("%Y_%m_%d")
    journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"

    # Run the mascot patrol alongside the journal read. Cheap
    # (read-only, ~1s typical) and the critical flag is exactly what
    # the briefing should amplify.
    mascot_aggregate: Dict = {}
    mascot_status = "skipped"
    try:
        from core.mascots import patrol_all as _patrol_all

        mascot_report = _patrol_all()
        mascot_aggregate = mascot_report.get("aggregate", {})
        mascot_status = "ran"
    except Exception as exc:  # noqa: BLE001 — briefing must not fail
        mascot_aggregate = {"error": f"{type(exc).__name__}: {exc}"}
        mascot_status = "error"

    if not journal_path.exists():
        return {
            "date": today,
            "entries": 0,
            "entities": [],
            "mascot_aggregate": mascot_aggregate,
            "mascot_patrol": mascot_status,
            "status": "no journal yet",
        }

    content = journal_path.read_text()
    lines = [l for l in content.splitlines() if l.strip().startswith("- ")]

    # Extract [[entity]] references
    entities = re.findall(r"\[\[([^\]]+)\]\]", content)
    unique_entities = sorted(set(entities))

    return {
        "date": today,
        "entries": len(lines),
        "entities": unique_entities,
        "entity_count": len(unique_entities),
        "mascot_aggregate": mascot_aggregate,
        "mascot_patrol": mascot_status,
        "status": "briefing ready",
    }


def handle_memory_consolidation() -> Dict:
    """
    Run auto-memory consolidation task.

    Clusters daily facts, extracts insights, updates memory files,
    and rebuilds the knowledge graph. Returns a summary of what was
    consolidated.
    """
    import asyncio
    from core.memory.consolidator import ConsolidationEngine

    engine = ConsolidationEngine()
    # Run async function synchronously
    report = asyncio.run(engine.consolidate_daily())

    return {
        "facts_captured": report.facts_captured,
        "clusters_created": report.clusters_created,
        "insights_extracted": report.insights_extracted,
        "memory_files_updated": report.memory_files_updated,
        "knowledge_graph_edges_added": report.knowledge_graph_edges_added,
        "duration_sec": report.duration_sec,
        "status": "completed" if report.insights_extracted > 0 else "no insights",
    }


def handle_memory_promotion() -> Dict:
    """
    Active Memory promotion — score recall patterns and promote survivors.

    Reads recall_stats (populated by RecallTracker during searches),
    scores entries with 6-component algorithm (frequency, relevance,
    diversity, recency, consolidation, conceptual), and promotes top
    candidates to Brain pages. Also prunes old recall_log entries.

    Inspired by OpenClaw Active Memory (short-term-promotion.ts).
    """
    from core.memory.memory_promoter import MemoryPromoter
    from core.memory.recall_tracker import RecallTracker

    # Prune old recall_log entries (90-day TTL)
    tracker = RecallTracker()
    pruned = tracker.prune_old_logs(max_age_days=90)

    # Run promotion
    promoter = MemoryPromoter()
    report = promoter.promote()

    return {
        "candidates_scored": report["candidates"],
        "promoted": report["promoted"],
        "pruned_log_entries": pruned,
        "entries": [
            {
                "snippet": e.get("snippet", "")[:100],
                "score": e.get("promotion_score", 0),
                "recalls": e.get("recall_count", 0),
                "days": e.get("unique_days", 0),
            }
            for e in report.get("entries", [])
        ],
        "status": f"promoted {report['promoted']}" if report["promoted"] > 0 else "no promotions",
    }


def handle_superbrain_sync_embed() -> Dict:
    """Nightly safety net: re-sync Brain + embed any orphaned docs.

    Phase 3 of SPRINT-SUPERBRAIN-REPAIR closed the tool_brain_write orphan
    gap, but a periodic sync is still valuable: if any write path ever
    skips sync_file (new tool, third-party agent, or temporary backend
    outage), this task catches the drift within 24h instead of letting it
    accumulate silently.
    """
    from core.superbrain.superbrain import Superbrain

    sb = Superbrain()
    if not sb._check_embedding():
        return {"status": "skipped", "reason": "embedding backend not available"}

    # Non-forcing sync — only re-indexes files that changed, but embeds
    # any existing brain_docs row that is missing a vector.
    result = sb.sync(force=False, embed=True)
    vectors = result.get("vectors", {})
    return {
        "pages": result.get("pages", 0),
        "journals": result.get("journals", 0),
        "vectors_added": vectors.get("embedded", 0),
        "vector_errors": vectors.get("errors", 0),
        "model": vectors.get("model", "n/a"),
        "status": "ok",
    }


def handle_graph_rebuild() -> Dict:
    """Rebuild the knowledge graph from entity_graph table.

    Re-runs rebuild_entity_graph() on the superbrain store, which
    re-extracts wikilinks from all indexed brain_docs. Then clears
    the PPR cache so subsequent queries use fresh topology.

    Interval: 6 hours. Cheap operation (~100ms on 1000 nodes).
    """
    from core.superbrain.store import SuperbrainStore

    store = SuperbrainStore()
    old_count = store.entity_graph_count()
    store.rebuild_entity_graph()
    new_count = store.entity_graph_count()

    # Invalidate PPR cache so queries use fresh graph topology
    try:
        from core.superbrain.graph import invalidate_ppr_cache
        invalidate_ppr_cache()
    except Exception:
        pass

    return {
        "status": "ok",
        "old_triples": old_count,
        "new_triples": new_count,
        "delta": new_count - old_count,
    }


# ═══════════════════════════════════════════════════════════════════
#  H2 / H4 — Reactive HEARTBEAT.md handler
# ═══════════════════════════════════════════════════════════════════
#
# Why reactive (not polling): the original H2 audit recommended evaluating
# HEARTBEAT.md every tick, which would burn LLM tokens on a static file.
# This handler reads the file content hash and short-circuits when nothing
# has changed since the last evaluation. The file only needs re-eval when:
#   1. content hash differs from last_seen_hash, OR
#   2. forced via force=True (e.g. sancho.wake from a watcher)
#
# H4 suppression: when the LLM responds with HEARTBEAT_OK (or content that
# normalizes to it), the handler still updates the hash so we don't re-eval
# the same file, but it returns suppressed=True so the engine knows not to
# broadcast a sancho.heartbeat.signal event to chat. Operators stay quiet
# unless there's something actionable.


def _heartbeat_hash(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _heartbeat_load_state() -> dict:
    if HEARTBEAT_STATE_PATH.exists():
        try:
            return json.loads(HEARTBEAT_STATE_PATH.read_text())
        except (json.JSONDecodeError, OSError):
            pass
    return {"last_hash": None, "last_eval_ts": None, "last_verdict": None}


def _heartbeat_save_state(state: dict) -> None:
    HEARTBEAT_STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    HEARTBEAT_STATE_PATH.write_text(json.dumps(state, indent=2))


def _heartbeat_normalize(response: str) -> str:
    if not response:
        return ""
    cleaned = response.strip()
    cleaned = cleaned.strip("`").strip()
    cleaned = re.sub(r"^(?:json|markdown|text)\s*\n", "", cleaned, flags=re.IGNORECASE)
    return cleaned.strip()


def _heartbeat_is_ok(response: str) -> bool:
    """True if the LLM response means 'nothing actionable, stay quiet'.

    Matches the bare HEARTBEAT_OK token, optionally with surrounding
    whitespace, code fences, or short prefix/suffix punctuation. Anything
    longer or with additional content is treated as actionable.
    """
    cleaned = _heartbeat_normalize(response).upper()
    if not cleaned:
        return False
    if cleaned == HEARTBEAT_OK_TOKEN:
        return True
    # Tolerate trivial trailing punctuation: "HEARTBEAT_OK." / "HEARTBEAT_OK!"
    if re.fullmatch(rf"{HEARTBEAT_OK_TOKEN}[.!]*", cleaned):
        return True
    return False


def _heartbeat_default_llm(prompt: str) -> str:
    """Default LLM caller — switchAILocal at localhost:18080.

    Isolated so tests can monkeypatch with a stub. Returns the raw
    completion text. Raises on transport errors so the handler can
    catch and report.
    """
    from openai import OpenAI
    _key = os.environ.get("AIL_API_KEY")
    if not _key:
        raise RuntimeError(
            "AIL_API_KEY env var not set — heartbeat handler cannot call LLM. "
            "Run `makakoo secret set AIL_API_KEY` to configure."
        )
    client = OpenAI(api_key=_key, base_url=HEARTBEAT_LLM_BASE_URL)
    response = client.chat.completions.create(
        model=HEARTBEAT_LLM_MODEL,
        messages=[{"role": "user", "content": prompt}],
        timeout=HEARTBEAT_LLM_TIMEOUT,
    )
    return response.choices[0].message.content or ""


def _heartbeat_build_prompt(content: str) -> str:
    return (
        "You are Harvey's reactive HEARTBEAT evaluator. Read the checklist "
        "below and decide whether anything is actionable RIGHT NOW.\n\n"
        "Rules:\n"
        f"- If nothing is actionable, respond with EXACTLY this token and nothing else: {HEARTBEAT_OK_TOKEN}\n"
        "- If something IS actionable, respond with a single tight paragraph "
        "(<= 4 sentences) describing what to do and why. No preamble, no "
        "apologies, no markdown headers.\n\n"
        "HEARTBEAT.md:\n"
        "---\n"
        f"{content}\n"
        "---\n"
    )


def handle_dynamic_checklist(
    *,
    force: bool = False,
    heartbeat_path: Optional[Path] = None,
    state_path: Optional[Path] = None,
    llm_caller: Optional[Callable[[str], str]] = None,
) -> Dict:
    """Reactive HEARTBEAT.md evaluator.

    Reads `HEARTBEAT.md`, hashes it, skips evaluation if the hash matches
    `last_hash` from state. On hash change (or force=True), calls the LLM
    to decide if anything is actionable. If the response is HEARTBEAT_OK,
    state is still updated but the result is marked `suppressed=True` so
    the engine knows not to broadcast a chat-visible signal.

    All side-effects (hash store, LLM call) are dependency-injected for
    test-stubbing — production callers pass nothing and get the defaults.
    """
    hb_path = heartbeat_path or HEARTBEAT_PATH
    st_path = state_path or HEARTBEAT_STATE_PATH

    if not hb_path.exists():
        return {
            "status": "no_heartbeat_file",
            "path": str(hb_path),
            "evaluated": False,
            "suppressed": True,
            "verdict": None,
        }

    try:
        content = hb_path.read_text(encoding="utf-8")
    except OSError as e:
        return {
            "status": "read_error",
            "error": str(e),
            "evaluated": False,
            "suppressed": True,
            "verdict": None,
        }

    content_hash = _heartbeat_hash(content)

    # Load state from the (possibly overridden) state path
    if st_path == HEARTBEAT_STATE_PATH:
        state = _heartbeat_load_state()
    else:
        if st_path.exists():
            try:
                state = json.loads(st_path.read_text())
            except (json.JSONDecodeError, OSError):
                state = {"last_hash": None, "last_eval_ts": None, "last_verdict": None}
        else:
            state = {"last_hash": None, "last_eval_ts": None, "last_verdict": None}

    last_hash = state.get("last_hash")

    if not force and last_hash == content_hash:
        return {
            "status": "unchanged",
            "evaluated": False,
            "suppressed": True,
            "verdict": None,
            "hash": content_hash,
        }

    # Hash differs (or forced) — evaluate
    prompt = _heartbeat_build_prompt(content)
    caller = llm_caller or _heartbeat_default_llm
    try:
        raw_response = caller(prompt)
    except Exception as e:
        log.warning("HEARTBEAT LLM call failed: %s", e)
        # Don't update hash on transport failure — retry next cycle
        return {
            "status": "llm_error",
            "error": str(e),
            "evaluated": False,
            "suppressed": True,
            "verdict": None,
            "hash": content_hash,
        }

    is_ok = _heartbeat_is_ok(raw_response)
    verdict = HEARTBEAT_OK_TOKEN if is_ok else _heartbeat_normalize(raw_response)
    # Log every evaluation so operators can see the handler is alive even
    # when HEARTBEAT_OK suppresses the chat-visible signal
    log.info("HEARTBEAT eval: %s (hash=%s)", "OK" if is_ok else "ACTIONABLE", content_hash[:12])

    # Update state — even on OK, so we don't re-eval an unchanged file
    state["last_hash"] = content_hash
    state["last_eval_ts"] = datetime.now().isoformat(timespec="seconds")
    state["last_verdict"] = verdict

    if st_path == HEARTBEAT_STATE_PATH:
        _heartbeat_save_state(state)
    else:
        st_path.parent.mkdir(parents=True, exist_ok=True)
        st_path.write_text(json.dumps(state, indent=2))

    return {
        "status": "ok" if is_ok else "actionable",
        "evaluated": True,
        "suppressed": is_ok,
        "verdict": verdict,
        "hash": content_hash,
    }


def handle_gym_classify() -> Dict:
    """
    Layer 2 of Harvey's Mascot GYM.

    Reads every jsonl file under data/errors/<today>/, classifies each
    entry (environmental/skill/user/unknown), clusters by normalized
    signature, writes clustered.json. Hot path — runs hourly, must not
    call an LLM, must not write to anything outside data/errors/.
    """
    from core.gym.classifier import scan_day

    summary = scan_day()
    return {
        "status": "ok",
        "date": summary["date"],
        "total_entries": summary["total_entries"],
        "by_class": summary["by_class"],
        "cluster_count": summary.get("cluster_count", 0),
        "top_cluster": (summary["clusters"][0]["sample_cmd"][:60] if summary["clusters"] else None),
    }


def handle_gym_hypothesize() -> Dict:
    """
    Layer 3 of Harvey's Mascot GYM — nightly hypothesis generator.

    Reads today's clustered.json, generates two types of hypotheses:
      - SKILL: autoimprover-proposed SKILL.md edits for skill-class clusters
      - CODE:  LLM-generated unified diff patches for code-class clusters

    Writes draft sprints to data/improvements/pending/. Slow — this is
    the cold path. Runs once per night behind a 23.5h time_gate
    + 01:00-04:00 active hours.
    """
    from core.gym.hypothesis import generate_hypotheses

    hyps = generate_hypotheses()
    return {
        "status": "ok",
        "count": len(hyps),
        "skill_count": sum(1 for h in hyps if h.patch_type == "skill"),
        "code_count": sum(1 for h in hyps if h.patch_type == "code"),
        "hypotheses": [
            {
                "id": h.id,
                "patch_type": h.patch_type,
                "skill_or_file": h.skill,
                "delta": h.delta,
                "baseline": h.baseline_score,
                "improved": h.improved_score,
                "gap_desc": h.gap_desc[:120],
            }
            for h in hyps
        ],
    }


def handle_gym_lope_gate() -> Dict:
    """
    Layer 4 of Harvey's Mascot GYM — lope validation gate.

    Runs every pending hypothesis through lope's validator pool. PASS
    moves to approved/, anything else moves to rejected/. This is the
    second-order noise filter that keeps escalated sprints from reaching
    the human queue.
    """
    from core.gym.lope_gate import validate_pending

    summary = validate_pending()
    return {
        "status": "ok",
        "date": summary.date,
        "approved": len(summary.approved),
        "rejected": len(summary.rejected),
        "malformed": len(summary.malformed),
        "errors": len(summary.errors),
    }


def handle_mascot_patrol() -> Dict:
    """
    Mascot Patrol — run all 4 nursery mascots on their patrol chores.

    Each mascot does one small, useful, read-only check matched to their
    highest stat profile:
      - Pixel (DEBUGGING 53)  → tail daemon logs for fresh error bursts
      - Cinder (DEBUGGING 65) → py_compile every .py file changed in 24h
      - Ziggy (SNARK 60)      → docstring coverage on recent public symbols
      - Glimmer (PATIENCE 56) → stale TODO / FIXME patrol across Brain pages

    One mascot failing cannot block the others; patrol_all() wraps each
    call in its own try/except and reports errors in the result dict.

    Default return is an aggregates-only summary so SANCHO's tick log stays
    small. Set `SANCHO_MASCOT_PATROL_VERBOSE=1` (or any truthy value) to
    include the full per-mascot `results` payload — useful when debugging
    what each mascot is actually seeing.

    See core/mascots/patrol.py for the implementation.
    """
    from core.mascots import patrol_all

    report = patrol_all()
    agg = report.get("aggregate", {})
    results = report.get("results", {})

    summary = {
        "daemon_error_hits": agg.get("daemon_error_hits", 0),
        "syntax_failures": agg.get("syntax_failures", 0),
        "missing_docstrings": agg.get("missing_docstrings", 0),
        "stale_todos": agg.get("stale_todos", 0),
        "critical": agg.get("critical", False),
        "mascot_errors": report.get("errors", {}),
        "pixel_flavor": results.get("pixel", {}).get("flavor", ""),
        "cinder_flavor": results.get("cinder", {}).get("flavor", ""),
        "ziggy_flavor": results.get("ziggy", {}).get("flavor", ""),
        "glimmer_flavor": results.get("glimmer", {}).get("flavor", ""),
    }

    if os.environ.get("SANCHO_MASCOT_PATROL_VERBOSE", "").strip().lower() in {
        "1",
        "true",
        "yes",
    }:
        summary["full_report"] = report

    return summary


def handle_gym_morning_report() -> Dict:
    """
    Layer 4b — Brain-journal rollup of last night's GYM activity.

    Appends an outliner-format block to today's journal summarizing the
    entire pipeline (errors → clusters → hypotheses → verdicts). This
    is the one notification channel — no Slack, no Telegram, Brain only.
    """
    from core.gym.lope_gate import build_morning_report

    report = build_morning_report()
    today = datetime.now().strftime("%Y_%m_%d")
    journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
    journal_path.parent.mkdir(parents=True, exist_ok=True)
    # Append; Logseq outliner files are plain markdown.
    existing = journal_path.read_text() if journal_path.exists() else ""
    if "[[Harvey Mascot GYM]] morning report" in existing:
        # Idempotent within the day — don't duplicate
        return {"status": "already_logged", "date": today}
    with open(journal_path, "a", encoding="utf-8") as f:
        if existing and not existing.endswith("\n"):
            f.write("\n")
        f.write(report)
    return {"status": "ok", "date": today, "lines": report.count("\n")}


def handle_gym_weekly_report() -> Dict:
    """
    Weekly rollup of the GYM pipeline — 7-day aggregates dropped into
    the Brain journal every Sunday morning. Counts hypotheses generated,
    approved, rejected, merged + average delta + top skills touched.

    Also refreshes `data/improvements/blocklist.json` — the set of
    cluster_ids whose recent hypotheses all failed the lope gate. The
    classifier consults this blocklist to skip noisy signatures that
    keep generating duds.
    """
    from datetime import timedelta

    improvements_dir = Path(HARVEY_HOME) / "data" / "improvements"
    provenance_dir = improvements_dir / "provenance"
    rejected_dir = improvements_dir / "rejected"
    merged_dir = improvements_dir / "merged"
    approved_dir = improvements_dir / "approved"

    now = datetime.now()
    week_ago = now - timedelta(days=7)

    def _iter_prov():
        if not provenance_dir.exists():
            return
        for p in provenance_dir.glob("*.json"):
            try:
                data = json.loads(p.read_text(encoding="utf-8"))
                gen = data.get("generated_at", "")
                stamp = datetime.fromisoformat(gen.replace("Z", "+00:00")) if gen else None
                if stamp and stamp.replace(tzinfo=None) < week_ago:
                    continue
                yield p, data
            except Exception:
                continue

    prov_entries = list(_iter_prov())
    total_generated = len(prov_entries)
    deltas = [int(d.get("delta", 0) or 0) for _, d in prov_entries]
    skill_counts: Dict[str, int] = {}
    for _, d in prov_entries:
        skill_counts[d.get("skill", "unknown")] = skill_counts.get(d.get("skill", "unknown"), 0) + 1

    def _count(d: Path) -> int:
        if not d.exists():
            return 0
        return len([p for p in d.glob("2*.md") if not p.name.endswith(".skill.md")])

    counts = {
        "approved_awaiting_human": _count(approved_dir),
        "rejected": _count(rejected_dir),
        "merged": _count(merged_dir),
    }

    # Build blocklist: cluster_ids whose only provenance is in rejected/
    rejected_ids = set()
    if rejected_dir.exists():
        for vj in rejected_dir.glob("*.verdict.json"):
            try:
                v = json.loads(vj.read_text(encoding="utf-8"))
                rejected_ids.add(v.get("hypothesis_id", ""))
            except Exception:
                continue

    blocklist_clusters = set()
    for _, d in prov_entries:
        hyp_id = d.get("id") or ""
        if hyp_id in rejected_ids:
            cid = d.get("cluster_id")
            if cid:
                blocklist_clusters.add(cid)

    blocklist_path = improvements_dir / "blocklist.json"
    blocklist_path.parent.mkdir(parents=True, exist_ok=True)
    blocklist_payload = {
        "refreshed_at": now.isoformat(),
        "cluster_ids": sorted(blocklist_clusters),
        "reason": "auto: cluster produced a hypothesis rejected by the lope gate within the last 7 days",
    }
    blocklist_path.write_text(
        json.dumps(blocklist_payload, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )

    # Journal entry
    today = now.strftime("%Y_%m_%d")
    journal_path = Path(HARVEY_HOME) / "data" / "Brain" / "journals" / f"{today}.md"
    journal_path.parent.mkdir(parents=True, exist_ok=True)
    avg_delta = (sum(deltas) / len(deltas)) if deltas else 0.0
    top_skills = sorted(skill_counts.items(), key=lambda kv: -kv[1])[:5]
    lines = [
        f"- [[Harvey Mascot GYM]] weekly rollup — {today}",
        f"  - Window: last 7 days",
        f"  - Hypotheses generated: {total_generated}",
        f"  - Avg delta: {avg_delta:.1f}",
        f"  - Pipeline: pending-human={counts['approved_awaiting_human']} rejected={counts['rejected']} merged={counts['merged']}",
        f"  - Blocklist: {len(blocklist_clusters)} noisy clusters suppressed for next week",
    ]
    if top_skills:
        lines.append(f"  - Top skills: " + ", ".join(f"[[{s}]] x{n}" for s, n in top_skills))
    lines.append("  - Next: `harvey improve review` to process the approved queue")
    block = "\n".join(lines) + "\n"

    existing = journal_path.read_text() if journal_path.exists() else ""
    if "[[Harvey Mascot GYM]] weekly rollup" in existing:
        return {"status": "already_logged", "date": today}
    with open(journal_path, "a", encoding="utf-8") as f:
        if existing and not existing.endswith("\n"):
            f.write("\n")
        f.write(block)

    return {
        "status": "ok",
        "date": today,
        "total_generated": total_generated,
        "avg_delta": round(avg_delta, 2),
        "blocklist_size": len(blocklist_clusters),
        "counts": counts,
    }


# ═════════════════════════════════════════════════════════════════════
# Mascot Missions — proactive daily chores (one per mascot)
# See core/mascots/missions.py for the implementations.
# ═════════════════════════════════════════════════════════════════════


def handle_mascot_pixel_doctor() -> Dict:
    """Pixel — SANCHO Doctor. Parse recent journals for repeated task failures."""
    from core.mascots.missions import pixel_sancho_doctor
    return pixel_sancho_doctor()


def handle_mascot_cinder_sentinel() -> Dict:
    """Cinder — Plugin Entrypoint Sentinel. Byte-compile every Python plugin entrypoint."""
    from core.mascots.missions import cinder_entrypoint_sentinel
    return cinder_entrypoint_sentinel()


def handle_mascot_ziggy_doctor() -> Dict:
    """Ziggy — SKILL.md Doctor. Lint every SKILL.md for completeness."""
    from core.mascots.missions import ziggy_skill_md_doctor
    return ziggy_skill_md_doctor()


def handle_mascot_glimmer_garden() -> Dict:
    """Glimmer — Brain Gardener. Archive empty Lead/Inbox pages >14d old."""
    from core.mascots.missions import glimmer_brain_gardener
    return glimmer_brain_gardener()


def handle_mascot_olibia_weekly() -> Dict:
    """Olibia — Weekly Home Digest. Aggregate commits/sancho/prospects/gym/brain."""
    from core.mascots.missions import olibia_weekly_digest
    return olibia_weekly_digest()
