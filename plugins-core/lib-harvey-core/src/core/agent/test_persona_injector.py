"""F.2+F.3 persona injection tests."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

_SRC = Path(__file__).resolve().parents[3]
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

from core.agent.persona_injector import (  # noqa: E402
    MODE_MARKER,
    PERSONA_MARKER,
    inject,
    persona_fragment,
)
from core.config.persona import Persona  # noqa: E402


class PersonaInjectorTests(unittest.TestCase):
    def setUp(self):
        self.fake = Persona(
            name="TestPersona",
            user="Pilot",
            home="/tmp/fake",
            pronouns="they/them",
            version=1,
        )

    def test_persona_fragment_contains_name_and_user(self):
        frag = persona_fragment(self.fake)
        self.assertIn("TestPersona", frag)
        self.assertIn("Pilot", frag)
        self.assertIn("they/them", frag)
        self.assertIn(PERSONA_MARKER, frag)

    def test_inject_adds_all_three_blocks_for_known_intent(self):
        out = inject("You are a helper.", intent="code", persona=self.fake)
        self.assertIn(PERSONA_MARKER, out)
        self.assertIn(MODE_MARKER, out)
        self.assertIn("Engineering mode", out)
        self.assertIn("Olibia", out)

    def test_unknown_intent_skips_mode_fragment(self):
        out = inject("base.", intent="sandwich", persona=self.fake)
        self.assertIn(PERSONA_MARKER, out)
        self.assertIn("Olibia", out)
        self.assertNotIn(MODE_MARKER, out)

    def test_intent_aliases_resolve(self):
        for alias in ["coding", "develop", "fix", "debug"]:
            out = inject("base.", intent=alias, persona=self.fake)
            self.assertIn("Engineering mode", out, f"alias {alias!r} must map to code mode")

    def test_injection_is_idempotent(self):
        once = inject("base.", intent="research", persona=self.fake)
        twice = inject(once, intent="research", persona=self.fake)
        self.assertEqual(once, twice, "second inject must be a no-op on its own output")
        self.assertEqual(once.count(PERSONA_MARKER), 1)
        self.assertEqual(once.count(MODE_MARKER), 1)
        # Olibia's fragment is idempotent via its own guard.
        self.assertEqual(once.count("## Your Companion: Olibia"), 1)

    def test_include_olibia_false_omits_mascot(self):
        out = inject("base.", intent="code", persona=self.fake, include_olibia=False)
        self.assertNotIn("Olibia", out)
        self.assertIn(PERSONA_MARKER, out)

    def test_creative_mode_turns_off_caveman(self):
        out = inject("base.", intent="creative", persona=self.fake)
        self.assertIn("caveman compression OFF", out)
        # Approval guard must survive — never auto-ship.
        self.assertIn("approval", out.lower())

    def test_archive_mode_routes_to_knowledge_ingest(self):
        """F.2 regression guard — the archive fragment must reflect the
        describe-vs-ingest rule Sebastian burned on 2026-04-20."""
        out = inject("base.", intent="archive", persona=self.fake)
        self.assertIn("harvey_knowledge_ingest", out)
        self.assertIn("not harvey_describe_*", out)

    def test_emotional_mode_leads_with_empathy(self):
        out = inject("base.", intent="emotional", persona=self.fake)
        self.assertIn("empathy", out.lower())

    def test_every_mode_adds_mode_marker_exactly_once(self):
        for intent in ["code", "research", "creative", "emotional", "image", "archive"]:
            out = inject("base.", intent=intent, persona=self.fake)
            self.assertEqual(
                out.count(MODE_MARKER),
                1,
                f"intent {intent!r} should yield exactly one mode marker",
            )

    def test_none_intent_skips_mode_but_keeps_identity_and_olibia(self):
        out = inject("base.", intent=None, persona=self.fake)
        self.assertIn(PERSONA_MARKER, out)
        self.assertIn("Olibia", out)
        self.assertNotIn(MODE_MARKER, out)


if __name__ == "__main__":
    unittest.main()
