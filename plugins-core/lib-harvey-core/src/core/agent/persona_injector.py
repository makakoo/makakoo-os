"""
Contextual persona injection — v0.2 Phase F.2 + F.3.

Given a base system prompt + an intent classification from the
IntelligentRouter, return a prompt augmented with:

  1. The persona's name + user mapping (F.2 — persona injection)
  2. Olibia's companion fragment (F.3 — voice unification)
  3. A mode-specific fragment for the detected intent (F.2)

The three fragments are idempotent and order-stable: repeated calls
don't re-inject, and any caller can pre-inject in a different order
without the final prompt drifting.

Intent modes recognized (case-insensitive):
  * ``code`` / ``coding`` / ``develop`` — engineering focus
  * ``research`` / ``investigate`` — multi-source synthesis
  * ``creative`` / ``write`` / ``draft`` — writing voice
  * ``emotional`` / ``personal`` — friend mode
  * ``image`` — visual generation context
  * ``archive`` — data organization / ingestion context
  * anything else → default mode (no extra fragment)

Why mode fragments live here and not in the IntelligentRouter: the
router's job is *classification*, not prompt engineering. Keeping the
two decoupled means one can change without rewriting the other.
"""

from __future__ import annotations

from typing import Optional

from core.agent.mascot import Olibia
from core.config.persona import Persona, load as load_persona


PERSONA_MARKER = "## Harvey Identity"
MODE_MARKER = "## Active Mode"

_MODE_FRAGMENTS: "dict[str, str]" = {
    "code": (
        "\n\n" + MODE_MARKER + "\n"
        "Engineering mode is active. Default to code-first answers:\n"
        "- Show the diff before explaining it.\n"
        "- Prefer editing existing files over creating new ones.\n"
        "- If adding a feature, add or extend a test alongside.\n"
        "- Never commit without explicit user approval.\n"
    ),
    "research": (
        "\n\n" + MODE_MARKER + "\n"
        "Research mode is active. Optimize for multi-source synthesis:\n"
        "- Cite specific sources, never vague `according to sources`.\n"
        "- Call out disagreements between sources explicitly.\n"
        "- Flag any claim you can't verify as `[unverified]`.\n"
        "- Offer a next action after every finding.\n"
    ),
    "creative": (
        "\n\n" + MODE_MARKER + "\n"
        "Creative mode is active. Full-prose voice — caveman compression OFF\n"
        "for this turn. Write emails, posts, and drafts as if the final\n"
        "reader will never see a Makakoo tool call. Draft first, ask for\n"
        "approval before sending, never auto-ship to an external channel.\n"
    ),
    "emotional": (
        "\n\n" + MODE_MARKER + "\n"
        "Personal / emotional mode is active. Lead with empathy before\n"
        "troubleshooting. Don't rush to fix — ask what the person needs\n"
        "first (venting vs solving vs planning). Normal tool use + Brain\n"
        "journaling stay available but recede to the background.\n"
    ),
    "image": (
        "\n\n" + MODE_MARKER + "\n"
        "Image / visual mode is active. Output concrete prompts for\n"
        "whichever image tool is being called. State dimensions, style,\n"
        "aspect ratio, and usage rights explicitly. When handing results\n"
        "back, include the file path + one-line caption.\n"
    ),
    "archive": (
        "\n\n" + MODE_MARKER + "\n"
        "Archive / ingest mode is active. Route persistent media through\n"
        "harvey_knowledge_ingest, not harvey_describe_*. A URL in the\n"
        "journal is not knowledge — it is a pointer. Only summarize after\n"
        "confirming the content was actually indexed.\n"
    ),
}


# Intent alias → canonical mode key. Kept as a simple dict so callers
# can update the router or the classifier without touching the fragments.
_ALIASES: "dict[str, str]" = {
    # code
    "coding": "code",
    "develop": "code",
    "engineering": "code",
    "fix": "code",
    "debug": "code",
    # research
    "investigate": "research",
    "search": "research",
    "lookup": "research",
    # creative
    "write": "creative",
    "draft": "creative",
    "post": "creative",
    # emotional
    "personal": "emotional",
    "feeling": "emotional",
    # image
    "visual": "image",
    "photo": "image",
    # archive
    "ingest": "archive",
    "save": "archive",
    "index": "archive",
}


def _normalize_mode(intent: Optional[str]) -> Optional[str]:
    if not intent:
        return None
    key = intent.strip().lower()
    key = _ALIASES.get(key, key)
    return key if key in _MODE_FRAGMENTS else None


def persona_fragment(persona: Optional[Persona] = None) -> str:
    """Return the identity block Harvey's system prompt injects.

    Includes the persona name + user mapping + pronouns so the LLM can
    refer to itself consistently across every CLI host. Idempotent.
    """
    p = persona or load_persona()
    return (
        "\n\n" + PERSONA_MARKER + "\n"
        f"You are {p.name}, {p.user}'s autonomous cognitive extension.\n"
        f"Pronouns: {p.pronouns}. Platform: Makakoo OS.\n"
        "Zero-sycophancy, radical competence, slightly blunt.\n"
    )


def inject(
    base_prompt: str,
    intent: Optional[str] = None,
    *,
    persona: Optional[Persona] = None,
    include_olibia: bool = True,
) -> str:
    """Compose the full system prompt.

    Args:
        base_prompt: Caller's base system prompt.
        intent: IntelligentRouter output ("code", "research", ...) or
            any freeform string. Unknown intents yield no mode fragment.
        persona: Optional persona override (tests, experiments). Defaults
            to the live persona.json.
        include_olibia: When False, skips the Olibia companion fragment —
            useful for purely internal tool flows where voice doesn't
            land in front of the user.

    Returns: the composed prompt. Every fragment is idempotent: calling
    ``inject`` repeatedly on its own output is a no-op after the first
    call. This is how the chat gateway can safely re-compose on every
    turn without bloating the prompt linearly.
    """
    result = (base_prompt or "").rstrip()

    if PERSONA_MARKER not in result:
        result += persona_fragment(persona)

    if include_olibia:
        # Olibia is the only companion fragment for now; future additions
        # (other mascots) would slot in here symmetrically.
        result = Olibia.inject_into_system_prompt(result)

    mode = _normalize_mode(intent)
    if mode and MODE_MARKER not in result:
        result += _MODE_FRAGMENTS[mode]

    return result.rstrip() + "\n"


__all__ = [
    "PERSONA_MARKER",
    "MODE_MARKER",
    "inject",
    "persona_fragment",
]
