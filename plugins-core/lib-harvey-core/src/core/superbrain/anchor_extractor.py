"""
anchor_extractor — produces structured memory anchors from Brain passages via switchAILocal.

Part of the brain-anchors memory system. See
`harvey-os/skills/meta/brain-anchors/SKILL.md` for architecture.

Contract: text in → JSON out with fields {anchor, anchor_level, triples,
entities, keywords, decision, anchor_hash, anchor_model}.

Routing: all LLM calls go through switchAILocal (localhost:18080), per
Harvey Operating Rule #7. Primary model = BRAIN_ANCHOR_MODEL_PRIMARY env
(default "auto", letting the gateway route). Fallback model =
BRAIN_ANCHOR_MODEL_FALLBACK env (default "claude-sonnet-4-5") — used when
the primary model's output fails the fact-preservation validator.

Zero direct Anthropic SDK usage. CLI-agnostic — works under any Harvey host.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import re
import time
from typing import Any, TypedDict

import httpx

log = logging.getLogger(__name__)


# ═══════════════════════════════════════════════════════════════════════
#  Config — all via env, sensible defaults
# ═══════════════════════════════════════════════════════════════════════

LLM_BASE_URL = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")

# API key precedence: AIL_API_KEY (canonical, globally exported in
# ~/.zshrc) > SWITCHAI_KEY (legacy) > LLM_API_KEY (legacy). All three
# point at the same switchAILocal gateway key on Sebastian's machine.
# Keeping the module-level name `SWITCHAI_KEY` so the existing header
# builder at line 185 doesn't need to change — only the resolution
# source is new.
SWITCHAI_KEY = (
    os.environ.get("AIL_API_KEY")
    or os.environ.get("SWITCHAI_KEY")
    or os.environ.get("LLM_API_KEY")
    or ""
)

PRIMARY_MODEL = os.environ.get("BRAIN_ANCHOR_MODEL_PRIMARY", "minimax:MiniMax-M2.7")
# Fallback is a non-Anthropic model because Sebastian's Anthropic credit
# balance is currently empty (switchAILocal returns 400 on any claude-*
# model). See memory: project_anthropic_out_of_credits.md.
#
# Canonical fallback per Sebastian 2026-04-14: `xiaomi-tp:mimo-v2-pro`
# (Xiaomi MiMo Pro via switchAILocal). Previous default `qwen3-max` was
# removed — even though it responds, it is NOT the approved fallback.
FALLBACK_MODEL = os.environ.get("BRAIN_ANCHOR_MODEL_FALLBACK", "xiaomi-tp:mimo-v2-pro")

# Truncate very long passages before extraction — small local models have
# tight context windows. Anchor still captures the lead.
MAX_CONTENT_CHARS = int(os.environ.get("BRAIN_ANCHOR_MAX_CHARS", "16000"))

REQUEST_TIMEOUT = float(os.environ.get("BRAIN_ANCHOR_TIMEOUT", "90.0"))
RETRIES = int(os.environ.get("BRAIN_ANCHOR_RETRIES", "3"))


# ═══════════════════════════════════════════════════════════════════════
#  Prompt
# ═══════════════════════════════════════════════════════════════════════

EXTRACTION_SYSTEM_PROMPT = """You are a memory-anchor extractor for Harvey, an autonomous agent memory system.

Given a passage of internal notes (a journal entry, page, or skill doc), extract a STRUCTURED ANCHOR that captures the essence for fast recall later. The anchor will be matched against future queries — it must be declarative, fact-dense, and searchable.

You MUST return ONLY a valid JSON object matching exactly this schema:

{
  "anchor": "ONE declarative sentence (20-40 words) stating the single most load-bearing fact this passage encodes",
  "anchor_level": "atomic",
  "triples": [["subject", "predicate", "object"]],
  "entities": ["[[EntityName]]"],
  "keywords": ["keyword1"],
  "decision": "ADD"
}

STRICT JSON RULES (read carefully — the #1 cause of rejection is malformed JSON):
- Every triple is EXACTLY three JSON strings: [string, string, string]. NEVER nest arrays inside a triple position. NEVER put a list of entities inside a single triple slot — emit one triple per relationship instead.
- Triple subjects and objects are PLAIN strings WITHOUT double-square-bracket wikilink notation. Write "KRIMDA", not "[[KRIMDA]]". Wikilinks with [[ ]] syntax appear ONLY in the "entities" field, never in "triples".
- "entities" is a JSON array of STRINGS. Each entity is a JSON string literal containing the text "[[Name]]" — e.g. the JSON literal "\"[[KRIMDA]]\"" (opening quote, open bracket, open bracket, Name, close bracket, close bracket, closing quote). The [[ ]] characters are part of the string content; they must be inside JSON quotes, not interpreted as JSON arrays.
- No trailing commas. No comments. No unquoted keys. No markdown fencing around the JSON.

Rules:
- "anchor" is ONE sentence. Not a paragraph. Not a bullet list. 20-40 words. Declarative, plain English. State the most load-bearing fact. If the passage is a decision, state the decision. If it's an incident, state what broke and what fixed it. If it's a plan, state the goal.
- "triples" are [subject, predicate, object] — STRICT MAX 6 entries. Pick the 6 most load-bearing facts; do not enumerate every relationship. Subjects and objects should be concrete but PLAIN strings: company names, file paths, tool names, commit hashes, version strings, person names. NO brackets. Predicates are verbs like "depends_on", "uses", "fixes", "mentions", "replaces", "tracks", "owns", "introduces".
- "entities" are [[wikilink]]-style references (double square brackets around a name) to people, projects, tools, concepts mentioned in the passage. 2-10 entries. Copy exact wikilinks from the passage if they exist.
- "keywords" are lowercase single words or short phrases (2 words max) for FTS5 matching. 3-10 entries. Include distinctive technical terms, error-signature words, project codenames.
- "anchor_level" is always the literal string "atomic" for this extraction.
- "decision" is always the literal string "ADD" for this extraction.

PRESERVE in the anchor (literal copy from passage when applicable):
- Exact numbers, dates, version strings, percentages
- File paths (e.g., harvey-os/skills/meta/X/SKILL.md)
- Command names, tool names, model IDs, API endpoints
- Error messages or log lines (quoted verbatim inside the sentence)
- [[WikiLink]] entity names

DO NOT include in the anchor:
- Hedging words ("might", "perhaps", "seems", "could be")
- Filler ("basically", "simply", "just", "really")
- Meta-commentary about the passage itself ("this page describes...", "the following notes")
- Multiple sentences — if you feel you need more than one, you're summarizing too much context; pick the most important fact
- Markdown fencing or code blocks inside the anchor field

Return ONLY the JSON object. No preamble, no markdown fencing, no trailing explanation."""


EXTRACTION_USER_TEMPLATE = """Extract the anchor for this passage.

Title: {name}
Type: {doc_type}

Content:
{content}"""


# Phase E — Mem0-style write-time dedup. Used ONLY when the caller passes
# existing_candidates. The extractor is asked to pick a decision instead
# of always returning "ADD".
DEDUP_EXTENSION_PROMPT = """

Dedup candidates — EXISTING memories that may already cover part or all of this passage:

{candidates_block}

You must additionally set the "decision" field to one of:
- "ADD"          — the new passage encodes a genuinely new fact that none of the candidates already cover. Default when in doubt.
- "UPDATE:<id>"  — the new passage REFINES or EXTENDS an existing candidate (corrects a number, adds a date, adds a relationship). The caller will merge your new anchor into candidate <id>. Replace <id> with the literal numeric id.
- "DELETE:<id>"  — the new passage OBSOLETES an existing candidate entirely. The caller will replace candidate <id>'s anchor with yours.
- "NOOP"         — the new passage is a near-duplicate of an existing candidate with no new information. Do NOT emit a new anchor; the caller will skip this write.

Rules:
- When choosing UPDATE or DELETE, your "anchor" field should be the MERGED or REPLACEMENT anchor that will land in the target row.
- When choosing NOOP, your "anchor" field can be a one-sentence restatement of the existing candidate — it will be discarded.
- When in doubt, pick ADD. False-positive dedup loses information; false-negative dedup only wastes a row.
- The decision syntax is STRICT: either a literal "ADD" / "NOOP", or "UPDATE:<N>" / "DELETE:<N>" with no spaces, no brackets, no quotes inside the id."""


def _format_dedup_candidates(candidates: list[dict]) -> str:
    """Render dedup candidates into the prompt block."""
    lines = []
    for c in candidates:
        cid = c.get("id", "?")
        name = (c.get("name") or "").replace("\n", " ")[:80]
        anchor = (c.get("anchor") or "").replace("\n", " ")[:300]
        lines.append(f"  id={cid} name={name!r}\n     anchor: {anchor}")
    return "\n".join(lines) if lines else "  (none — emit ADD)"


# ═══════════════════════════════════════════════════════════════════════
#  Types
# ═══════════════════════════════════════════════════════════════════════

class AnchorResult(TypedDict):
    anchor: str
    anchor_level: str
    triples: list[list[str]]
    entities: list[str]
    keywords: list[str]
    decision: str
    anchor_hash: str
    anchor_model: str


class AnchorExtractionError(RuntimeError):
    """Raised when anchor extraction cannot produce a valid result."""


# ═══════════════════════════════════════════════════════════════════════
#  LLM call
# ═══════════════════════════════════════════════════════════════════════

def _supports_json_mode(model: str) -> bool:
    """Known providers that reject response_format={'type':'json_object'}.

    switchAILocal's claude provider returns 400 on the response_format
    param. MiniMax and most OpenAI-compatible providers accept it. The
    prompt instructs JSON-only output as a belt-and-suspenders fallback.
    """
    m = model.lower()
    return not (m.startswith("claude") or m.startswith("anthropic"))


def _call_llm(model: str, messages: list[dict], request_json: bool = True) -> tuple[str, str]:
    """POST to switchAILocal chat/completions. Returns (content, resolved_model)."""
    headers: dict[str, str] = {"Content-Type": "application/json"}
    if SWITCHAI_KEY:
        headers["Authorization"] = f"Bearer {SWITCHAI_KEY}"

    payload: dict[str, Any] = {
        "model": model,
        "messages": messages,
        "temperature": 0.2,
        "max_tokens": 3072,
    }
    if request_json and _supports_json_mode(model):
        # OpenAI-compatible structured output. Skipped for claude/anthropic
        # providers which reject the param via switchAILocal's router.
        payload["response_format"] = {"type": "json_object"}

    last_error: Exception | None = None
    for attempt in range(RETRIES):
        try:
            with httpx.Client(timeout=REQUEST_TIMEOUT) as client:
                resp = client.post(
                    f"{LLM_BASE_URL}/chat/completions",
                    json=payload,
                    headers=headers,
                )
            resp.raise_for_status()
            data = resp.json()
            choices = data.get("choices", [])
            if not choices:
                raise RuntimeError(f"empty choices in response: {data}")
            content = choices[0].get("message", {}).get("content", "")
            if not content:
                raise RuntimeError(f"empty content in response choice: {choices[0]}")
            # Return the resolved model name from the response, not the
            # requested alias. Otherwise `auto` routing would end up written
            # as the literal string "auto" into brain_docs.anchor_model,
            # which destroys observability (see the model='auto' bug from
            # SPRINT-SUPERBRAIN-REPAIR 2026-04-11).
            resolved_model = data.get("model", model)
            return content, resolved_model
        except (httpx.ConnectError, httpx.TimeoutException) as e:
            last_error = AnchorExtractionError(
                f"switchAILocal unreachable ({model}, attempt {attempt+1}): {e}"
            )
        except Exception as e:
            last_error = AnchorExtractionError(
                f"LLM call failed ({model}, attempt {attempt+1}): {e}"
            )
        if attempt < RETRIES - 1:
            time.sleep(1 * (attempt + 1))

    assert last_error is not None
    raise last_error


# ═══════════════════════════════════════════════════════════════════════
#  Parse + validate
# ═══════════════════════════════════════════════════════════════════════

def _strip_markdown_fencing(raw: str) -> str:
    """Defensively strip ```json ... ``` fencing the model might emit."""
    raw = raw.strip()
    if raw.startswith("```"):
        lines = raw.split("\n")
        if lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].rstrip().startswith("```"):
            lines = lines[:-1]
        raw = "\n".join(lines).strip()
    return raw


def _parse_anchor_json(raw: str) -> dict:
    """Parse LLM response as JSON. Raises AnchorExtractionError on failure."""
    cleaned = _strip_markdown_fencing(raw)
    try:
        return json.loads(cleaned)
    except json.JSONDecodeError as e:
        # Last-ditch: find first `{` and last `}` and try the slice
        start = cleaned.find("{")
        end = cleaned.rfind("}")
        if start != -1 and end > start:
            try:
                return json.loads(cleaned[start : end + 1])
            except json.JSONDecodeError:
                pass
        raise AnchorExtractionError(
            f"could not parse JSON: {e}\nraw head: {cleaned[:400]!r}"
        )


def _validate_anchor(obj: dict, original_content: str, name: str = "") -> tuple[bool, list[str]]:
    """
    Fact-preservation validator.

    Checks:
    - Required fields present and correctly typed
    - Anchor length in sensible range (10-500 chars, roughly one sentence)
    - Triples well-formed [s, p, o] lists
    - Entities and keywords are lists
    - At least one extracted entity appears in the original content (anti-hallucination)
    - Decision is one of the allowed strings

    Returns (is_valid, list_of_issues). Empty issues list means valid.
    """
    issues: list[str] = []
    required = ("anchor", "anchor_level", "triples", "entities", "keywords", "decision")
    for field in required:
        if field not in obj:
            issues.append(f"missing field: {field}")
    if issues:
        return False, issues

    anchor = obj.get("anchor", "")
    if not isinstance(anchor, str):
        issues.append(f"anchor wrong type: {type(anchor).__name__}")
    elif len(anchor) < 10:
        issues.append(f"anchor too short: {len(anchor)} chars")
    elif len(anchor) > 500:
        issues.append(f"anchor too long: {len(anchor)} chars (should be one sentence)")

    triples = obj.get("triples", [])
    if not isinstance(triples, list):
        issues.append("triples must be a list")
    else:
        for t in triples:
            if not (isinstance(t, list) and len(t) == 3 and all(isinstance(x, str) for x in t)):
                issues.append(f"malformed triple: {t!r}")
                break

    entities = obj.get("entities", [])
    if not isinstance(entities, list):
        issues.append("entities must be a list")

    keywords = obj.get("keywords", [])
    if not isinstance(keywords, list):
        issues.append("keywords must be a list")

    decision = obj.get("decision", "")
    if not isinstance(decision, str):
        issues.append(f"decision wrong type: {type(decision).__name__}")
    elif decision not in ("ADD", "UPDATE", "DELETE", "NOOP") and not (
        decision.startswith("UPDATE:") or decision.startswith("DELETE:")
    ):
        issues.append(f"invalid decision: {decision!r}")

    # Anti-hallucination: at least one extracted entity should textually
    # appear in the original content OR the document name. Also check
    # individual word tokens so "SPS Consulting & Services GmbH" matches
    # "sps_consulting" in the filename. Loose on purpose — the point is
    # to catch gross hallucinations, not to enforce exact string equality.
    if isinstance(entities, list) and entities:
        haystack = (original_content + " " + name).lower()
        # Also strip common punctuation from haystack so underscore-separated
        # filenames match space-separated entity names.
        haystack_normalized = re.sub(r"[_\-.]+", " ", haystack)
        hits = 0
        for e in entities:
            if not isinstance(e, str):
                continue
            needle = e.strip("[]").strip().lower()
            if not needle:
                continue
            if needle in haystack or needle in haystack_normalized:
                hits += 1
                continue
            # Fallback: any word token of 4+ chars from the entity appears?
            tokens = [t for t in re.split(r"\W+", needle) if len(t) >= 4]
            if any(t in haystack_normalized for t in tokens):
                hits += 1
        if hits == 0:
            issues.append(
                f"no extracted entity appears in content or name (possible hallucination): {entities!r}"
            )

    return len(issues) == 0, issues


# ═══════════════════════════════════════════════════════════════════════
#  Public API
# ═══════════════════════════════════════════════════════════════════════

def extract_anchor(name: str, content: str, doc_type: str = "page",
                   existing_candidates: list[dict] | None = None) -> AnchorResult:
    """
    Extract a structured anchor from a memory passage.

    Tries PRIMARY_MODEL first. If the call or validator fails, falls back to
    FALLBACK_MODEL. If both fail, raises AnchorExtractionError. The caller is
    expected to catch this and fall back to a NULL anchor (best-effort — the
    write path must never block on extraction failure).

    Args:
        name: document title (used in the prompt as context)
        content: full passage text
        doc_type: "page" or "journal"
        existing_candidates: optional list of dedup candidates (Phase E).
            Each dict should have keys `id`, `name`, `anchor`. When provided,
            the extractor prompts the model with a state machine and the
            returned `decision` field may be "ADD", "UPDATE:<id>",
            "DELETE:<id>", or "NOOP" instead of always "ADD". Callers that
            don't want dedup should pass None or [].

    Returns:
        AnchorResult with anchor_hash (blake2b of content) and anchor_model
        (which model produced the accepted extraction) filled in.

    Raises:
        AnchorExtractionError: if no model could produce a valid extraction.
    """
    if not content or len(content.strip()) < 20:
        raise AnchorExtractionError(f"content too short ({len(content)} chars)")

    truncated = content[:MAX_CONTENT_CHARS]
    if len(content) > MAX_CONTENT_CHARS:
        truncated += f"\n\n[... {len(content) - MAX_CONTENT_CHARS} chars truncated for extraction ...]"

    user_msg = EXTRACTION_USER_TEMPLATE.format(
        name=name, doc_type=doc_type, content=truncated
    )

    # Build the system prompt — tack on the dedup extension only when
    # candidates are present and non-empty. Without candidates, the
    # extractor behaves exactly like Phase B (always ADD).
    system_prompt = EXTRACTION_SYSTEM_PROMPT
    if existing_candidates:
        candidates_block = _format_dedup_candidates(existing_candidates)
        system_prompt = system_prompt + DEDUP_EXTENSION_PROMPT.format(
            candidates_block=candidates_block
        )

    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_msg},
    ]

    content_hash = hashlib.blake2b(content.encode("utf-8"), digest_size=16).hexdigest()

    models_to_try = [m for m in (PRIMARY_MODEL, FALLBACK_MODEL) if m]
    if not models_to_try:
        raise AnchorExtractionError("no models configured (BRAIN_ANCHOR_MODEL_PRIMARY/FALLBACK)")

    last_error: Exception | None = None
    for idx, model in enumerate(models_to_try):
        is_last = idx == len(models_to_try) - 1
        try:
            raw, resolved_model = _call_llm(model, messages)
            obj = _parse_anchor_json(raw)
        except AnchorExtractionError as e:
            last_error = e
            log.warning("anchor extraction LLM/parse failed on %s: %s", model, e)
            if is_last:
                raise
            continue

        valid, issues = _validate_anchor(obj, content, name=name)
        if valid:
            result: AnchorResult = {
                "anchor": obj["anchor"].strip(),
                "anchor_level": obj.get("anchor_level", "atomic") or "atomic",
                "triples": obj.get("triples", []),
                "entities": obj.get("entities", []),
                "keywords": obj.get("keywords", []),
                "decision": obj.get("decision", "ADD") or "ADD",
                "anchor_hash": content_hash,
                "anchor_model": resolved_model or model,
            }
            return result

        log.warning(
            "anchor validation failed on %s: %s (will %s)",
            model,
            issues,
            "try fallback" if not is_last else "raise",
        )
        last_error = AnchorExtractionError(f"validation failed on {model}: {issues}")
        if is_last:
            raise last_error

    # Unreachable in practice — the loop raises on last model.
    assert last_error is not None
    raise last_error


def extract_anchor_safe(name: str, content: str, doc_type: str = "page",
                        existing_candidates: list[dict] | None = None) -> AnchorResult | None:
    """
    Best-effort wrapper — returns None instead of raising.

    Use from the write path (store._sync_file) so anchor failures never
    block a brain_docs write. Optional `existing_candidates` enables
    Phase E dedup; pass None/empty to skip.
    """
    try:
        return extract_anchor(name, content, doc_type, existing_candidates=existing_candidates)
    except AnchorExtractionError as e:
        log.warning("anchor extraction failed for %r (%s): %s", name, doc_type, e)
        return None
    except Exception as e:
        log.exception("unexpected error in anchor extraction for %r: %s", name, e)
        return None


# ═══════════════════════════════════════════════════════════════════════
#  CLI — for smoke testing and Phase C backfill reuse
# ═══════════════════════════════════════════════════════════════════════

def _cli() -> None:
    import argparse
    import sys

    parser = argparse.ArgumentParser(description="Extract an anchor from stdin or a file.")
    parser.add_argument("--file", type=str, help="Read content from file (default: stdin)")
    parser.add_argument("--name", type=str, default="smoke-test", help="Document name")
    parser.add_argument("--type", type=str, default="page", help="Document type (page|journal)")
    parser.add_argument("--model", type=str, help="Override primary model for this call")
    args = parser.parse_args()

    if args.model:
        global PRIMARY_MODEL
        PRIMARY_MODEL = args.model

    if args.file:
        content = open(args.file, "r", encoding="utf-8").read()
    else:
        content = sys.stdin.read()

    try:
        result = extract_anchor(args.name, content, args.type)
    except AnchorExtractionError as e:
        print(f"FAILED: {e}", file=sys.stderr)
        sys.exit(1)

    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    _cli()
