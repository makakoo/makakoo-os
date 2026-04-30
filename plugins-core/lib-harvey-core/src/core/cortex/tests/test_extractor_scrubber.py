from core.cortex.extractor import extract_memory_candidates
from core.cortex.scrubber import scrub_memory_text


def test_explicit_preference_candidate():
    c = extract_memory_candidates("Remember: I prefer owl mascots", "noted")
    assert len(c) == 1
    assert c[0].memory_type == "preference"
    assert c[0].confidence >= 0.9


def test_generic_thanks_no_candidate():
    assert extract_memory_candidates("thanks", "") == []


def test_scrubber_redacts_secrets():
    r = scrub_memory_text("api_key=sk-test-1234567890abcdef and SSN 123-45-6789")
    assert r.ok
    assert "sk-test" not in r.text
    assert "123-45-6789" not in r.text
    assert "[REDACTED]" in r.text
