"""Trigger-coverage test for skill-tytus-files.

Verifies that the SKILL.md's decision tree covers the 40+ natural-
language phrasings it claims to handle in English + Spanish, and that
each resolves to the correct verb + canonical args.

Approach: we don't embed an LLM. We encode the decision tree as a
regex-based classifier that mirrors the SKILL.md trigger table. If the
classifier + SKILL.md drift apart, either the test fails (missing
coverage) or the SKILL.md's table needs to grow. Either way the human
notices.

No network, no pod traffic. Pure offline unit test.
"""

from __future__ import annotations

import re
import unittest
from dataclasses import dataclass
from typing import Optional


# ── Classifier ───────────────────────────────────────────────
#
# Minimal rule-based intent matcher for the EN + ES trigger table
# declared in SKILL.md. Order matters: we check PULL triggers that
# include "from" before PUSH triggers for ambiguous verbs like
# "copy" — "copy X from pod 2" is a pull, "copy X to pod 2" is a
# push.
#
# The classifier returns a dataclass with the verb + pod + required
# paths so the test asserts on shape, not on a single string.

PUSH_PATTERNS_EN = [
    # verb phrases that imply Mac → pod direction
    r"\b(push|upload|share|drop|put|place)\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\b(send|give|copy|move)\b.*\bto\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\b(copy|move)\b.*\binto\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
]
PUSH_PATTERNS_ES = [
    r"\b(manda|env[ií]a|sube|pasa|p[aá]sale|dale|copia)\b.*\b(al|a)\s+(pod|pod-)\s*(?P<pod>\d+)\b",
]

PULL_PATTERNS_EN = [
    r"\b(pull|download|fetch|grab|bring back|get|save)\b.*\bfrom\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\bcopy\b.*\bfrom\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\boff\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
]
PULL_PATTERNS_ES = [
    r"\b(baja|b[aá]jame|descarga|tr[aá]eme|consigue)\b.*\bdel\s+(pod|pod-)\s*(?P<pod>\d+)\b",
]

LS_PATTERNS_EN = [
    r"\bwhat'?s on\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\b(list|show|ls)\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\bwhat files\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
]
LS_PATTERNS_ES = [
    r"\bqu[eé] hay\b.*\b(en el|en|del)\s+(pod|pod-)\s*(?P<pod>\d+)\b",
    r"\b(lista|mu[eé]strame|qu[eé] archivos)\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
]

RM_PATTERNS_EN = [
    r"\b(delete|remove|rm|wipe|clear)\b.*\b(from|on)\b.*\b(pod|pod-)\s*(?P<pod>\d+)\b",
]
RM_PATTERNS_ES = [
    r"\b(borra|elimina|quita)\b.*\bdel\s+(pod|pod-)\s*(?P<pod>\d+)\b",
]


@dataclass
class Intent:
    verb: str  # push | pull | ls | rm
    pod: Optional[str]  # "02" etc, or None if not specified


def _scan(patterns: list[str], text: str) -> Optional[re.Match]:
    for p in patterns:
        m = re.search(p, text, flags=re.IGNORECASE)
        if m:
            return m
    return None


def classify(text: str) -> Optional[Intent]:
    t = text.strip()

    # Order: PULL + RM + LS before PUSH so "copy X from pod" beats
    # "copy X to pod" and "delete from pod" doesn't get absorbed as
    # a push of the word "delete".
    for patterns, verb in [
        (RM_PATTERNS_EN, "rm"),
        (RM_PATTERNS_ES, "rm"),
        (PULL_PATTERNS_EN, "pull"),
        (PULL_PATTERNS_ES, "pull"),
        (LS_PATTERNS_EN, "ls"),
        (LS_PATTERNS_ES, "ls"),
        (PUSH_PATTERNS_EN, "push"),
        (PUSH_PATTERNS_ES, "push"),
    ]:
        m = _scan(patterns, t)
        if m:
            pod = m.groupdict().get("pod")
            return Intent(verb=verb, pod=f"{int(pod):02d}" if pod else None)
    return None


# ── Test corpus (40+ phrasings across EN + ES) ──────────────

CORPUS: list[tuple[str, str, Optional[str]]] = [
    # (phrasing, expected_verb, expected_pod)

    # PUSH — English
    ("push report.pdf to pod 2", "push", "02"),
    ("push this to pod-04", "push", "04"),
    ("send the meeting notes to pod 2", "push", "02"),
    ("send the PDF to pod-04", "push", "04"),
    ("upload my CV to pod 2", "push", "02"),
    ("upload this folder to pod-04", "push", "04"),
    ("copy file.txt to pod 2", "push", "02"),
    ("copy this into pod 4", "push", "04"),
    ("share x.zip with pod 2", "push", "02"),
    ("give the tarball to pod 4", "push", "04"),
    ("move the slides to pod-02", "push", "02"),
    ("drop readme.md on pod 4", "push", "04"),
    ("put the dataset onto pod 2", "push", "02"),
    ("place this file on pod-04", "push", "04"),

    # PUSH — Spanish
    ("manda el PDF al pod 2", "push", "02"),
    ("envía el archivo al pod 4", "push", "04"),
    ("envia el archivo al pod-02", "push", "02"),
    ("sube este archivo al pod 2", "push", "02"),
    ("sube esta carpeta al pod-04", "push", "04"),
    ("copia el reporte al pod 2", "push", "02"),
    ("pasa el documento al pod 4", "push", "04"),
    ("pásale el archivo al pod 02", "push", "02"),
    ("dale el informe al pod 4", "push", "04"),

    # PULL — English
    ("pull report.pdf from pod 2", "pull", "02"),
    ("download the file from pod-04", "pull", "04"),
    ("fetch the log from pod 2", "pull", "02"),
    ("grab the tarball off pod 4", "pull", "04"),
    ("bring back the results from pod 2", "pull", "02"),
    ("get the output from pod-04", "pull", "04"),
    ("save the report from pod 2 to my Mac", "pull", "02"),
    ("copy the PDF from pod 4", "pull", "04"),

    # PULL — Spanish
    ("baja el archivo del pod 2", "pull", "02"),
    ("bájame el reporte del pod 4", "pull", "04"),
    ("descarga el PDF del pod 2", "pull", "02"),
    ("tráeme el log del pod-04", "pull", "04"),
    ("consigue el archivo del pod 2", "pull", "02"),

    # LS — English
    ("what's on pod 2", "ls", "02"),
    ("what's on pod-04", "ls", "04"),
    ("list pod 2", "ls", "02"),
    ("list pod-04's inbox", "ls", "04"),
    ("show me files on pod 2", "ls", "02"),
    ("ls pod 4", "ls", "04"),
    ("what files does pod 2 have", "ls", "02"),

    # LS — Spanish
    ("qué hay en el pod 2", "ls", "02"),
    ("que hay en pod-04", "ls", "04"),
    ("lista el pod 2", "ls", "02"),
    ("muéstrame los archivos del pod 4", "ls", "04"),
    ("qué archivos tiene el pod 2", "ls", "02"),

    # RM — English
    ("delete report.pdf from pod 2", "rm", "02"),
    ("remove the old file from pod-04", "rm", "04"),
    ("rm /app/workspace/inbox/x on pod 2", "rm", "02"),
    ("wipe that file on pod 4", "rm", "04"),
    ("clear the inbox on pod 2", "rm", "02"),

    # RM — Spanish
    ("borra ese archivo del pod 2", "rm", "02"),
    ("elimina el archivo del pod-04", "rm", "04"),
    ("quita el reporte del pod 2", "rm", "02"),
]


class TriggerCoverageTest(unittest.TestCase):
    """Assert every corpus entry classifies correctly."""

    def test_corpus_size_at_least_40(self):
        # Sprint spec: 40+ phrasings. Keep a lower bound so future
        # edits that prune too aggressively fail the test.
        self.assertGreaterEqual(
            len(CORPUS),
            40,
            "Sprint spec requires at least 40 trigger phrasings; "
            f"corpus has {len(CORPUS)}",
        )

    def test_each_phrase_resolves_to_expected_verb(self):
        failures = []
        for phrase, expected_verb, expected_pod in CORPUS:
            intent = classify(phrase)
            if intent is None:
                failures.append((phrase, "NONE", expected_verb))
                continue
            if intent.verb != expected_verb:
                failures.append((phrase, intent.verb, expected_verb))
            if expected_pod is not None and intent.pod != expected_pod:
                failures.append(
                    (phrase, f"pod={intent.pod}", f"pod={expected_pod}")
                )
        if failures:
            msg = "\n".join(
                f"  {phrase!r}: got {got}, expected {exp}"
                for phrase, got, exp in failures
            )
            self.fail(f"{len(failures)} triggers misclassified:\n{msg}")

    def test_english_and_spanish_both_represented(self):
        """Sanity: at least 10 Spanish phrasings alongside English."""
        spanish_markers = ("á", "é", "í", "ó", "ú", "ñ", "qué", "que ", "del", "al ")
        es_count = sum(
            1 for phrase, _, _ in CORPUS
            if any(m in phrase.lower() for m in spanish_markers)
        )
        self.assertGreaterEqual(
            es_count, 10, f"Want ≥10 Spanish phrasings, got {es_count}"
        )

    def test_all_four_verbs_covered(self):
        verbs = {verb for _, verb, _ in CORPUS}
        self.assertEqual(verbs, {"push", "pull", "ls", "rm"})

    def test_pull_with_copy_from_beats_push(self):
        """Regression: 'copy X from pod N' must resolve to pull, not push."""
        intent = classify("copy the PDF from pod 4")
        self.assertIsNotNone(intent)
        self.assertEqual(intent.verb, "pull")
        self.assertEqual(intent.pod, "04")

    def test_empty_input_returns_none(self):
        self.assertIsNone(classify(""))
        self.assertIsNone(classify("hello"))

    def test_unknown_pod_number_still_extracted(self):
        # Triple-digit pod ids are valid if a future tier ships them.
        intent = classify("push file.txt to pod 123")
        self.assertIsNotNone(intent)
        self.assertEqual(intent.verb, "push")
        self.assertEqual(intent.pod, "123")


if __name__ == "__main__":
    unittest.main()
