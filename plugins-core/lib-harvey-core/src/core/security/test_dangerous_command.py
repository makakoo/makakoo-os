"""Tests for dangerous_command module.

RED phase - tests for Dangerous Command Detection (Plan 02-05).
"""

import sys
import os
import unittest

# Walk up from src/core/security/ to the plugin root (lib-harvey-core/)
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))

# Import using hyphen-to-underscore workaround
import importlib.util

spec = importlib.util.spec_from_file_location(
    "dangerous_command", os.path.join(os.path.dirname(__file__), "dangerous_command.py")
)
dc_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dc_module)

detect_dangerous_command = dc_module.detect_dangerous_command
is_approved = dc_module.is_approved
approve_session = dc_module.approve_session
approve_permanent = dc_module.approve_permanent
clear_session = dc_module.clear_session
DANGEROUS_PATTERNS = dc_module.DANGEROUS_PATTERNS
_normalize_command_for_detection = dc_module._normalize_command_for_detection
_approval_key_aliases = dc_module._approval_key_aliases


class TestDetectDangerousCommand(unittest.TestCase):
    """Test detect_dangerous_command function."""

    def test_detect_rm_rf_root(self):
        """Test 1: detect_dangerous_command("rm -rf /") returns dangerous."""
        is_dang, key, desc = detect_dangerous_command("rm -rf /")
        self.assertTrue(is_dang)
        self.assertEqual(key, "delete in root path")

    def test_detect_safe_rm(self):
        """Test 2: detect_dangerous_command("rm file.txt") returns safe."""
        is_dang, key, desc = detect_dangerous_command("rm file.txt")
        self.assertFalse(is_dang)
        self.assertIsNone(key)
        self.assertIsNone(desc)

    def test_detect_chmod_777(self):
        """Test 3: detect_dangerous_command("chmod 777 file") returns dangerous."""
        is_dang, key, desc = detect_dangerous_command("chmod 777 file")
        self.assertTrue(is_dang)
        self.assertEqual(key, "world/other-writable permissions")

    def test_detect_sql_drop(self):
        """Test 4: detect_dangerous_command("DROP TABLE users") returns dangerous."""
        is_dang, key, desc = detect_dangerous_command("DROP TABLE users")
        self.assertTrue(is_dang)
        self.assertEqual(key, "SQL DROP")

    def test_detect_fullwidth_unicode_obfuscation(self):
        """Test 5: Fullwidth unicode obfuscation is normalized and detected."""
        # Fullwidth 'r' and 'm' (U+FF52, U+FF4D)
        fullwidth_rm = "\uff52\uff4d\u200b-rf /"
        is_dang, key, desc = detect_dangerous_command(fullwidth_rm)
        self.assertTrue(is_dang, "NFKC normalization should detect fullwidth rm -rf")

    def test_detect_ansi_escape_codes(self):
        """Test 6: ANSI escape codes are stripped before detection."""
        # ANSI escape sequence for bold
        ansi_rm = "\x1b[1mrm\x1b[0m -rf /"
        is_dang, key, desc = detect_dangerous_command(ansi_rm)
        self.assertTrue(is_dang, "ANSI codes should be stripped before detection")

    def test_detect_sql_delete_no_where(self):
        """SQL DELETE without WHERE is dangerous."""
        is_dang, key, desc = detect_dangerous_command("DELETE FROM users")
        self.assertTrue(is_dang)
        self.assertEqual(key, "SQL DELETE without WHERE")

    def test_detect_fork_bomb(self):
        """Fork bomb pattern is detected."""
        is_dang, key, desc = detect_dangerous_command(":(){ :|:& };:")
        self.assertTrue(is_dang)
        self.assertEqual(key, "fork bomb")

    def test_detect_kill_hermes(self):
        """pkill hermes (self-termination) is detected."""
        is_dang, key, desc = detect_dangerous_command("pkill hermes")
        self.assertTrue(is_dang)
        self.assertEqual(key, "kill hermes/gateway process (self-termination)")

    def test_safe_git_operation(self):
        """Safe git operations are not flagged."""
        is_dang, key, desc = detect_dangerous_command("git status")
        self.assertFalse(is_dang)
        is_dang, key, desc = detect_dangerous_command("git add .")
        self.assertFalse(is_dang)
        is_dang, key, desc = detect_dangerous_command("git commit -m 'fix: bug'")
        self.assertFalse(is_dang)
        assert key is None
        assert desc is None

    def test_detect_chmod_777(self):
        """Test 3: detect_dangerous_command("chmod 777 file") returns dangerous."""
        is_dang, key, desc = detect_dangerous_command("chmod 777 file")
        assert is_dang is True
        assert key == "world/other-writable permissions"

    def test_detect_sql_drop(self):
        """Test 4: detect_dangerous_command("DROP TABLE users") returns dangerous."""
        is_dang, key, desc = detect_dangerous_command("DROP TABLE users")
        assert is_dang is True
        assert key == "SQL DROP"

    def test_detect_fullwidth_unicode_obfuscation(self):
        """Test 5: Fullwidth unicode obfuscation is normalized and detected."""
        # Fullwidth 'r' and 'm' (U+FF52, U+FF4D)
        fullwidth_rm = "\uff52\uff4d\u200b-rf /"
        is_dang, key, desc = detect_dangerous_command(fullwidth_rm)
        assert is_dang is True, "NFKC normalization should detect fullwidth rm -rf"

    def test_detect_ansi_escape_codes(self):
        """Test 6: ANSI escape codes are stripped before detection."""
        # ANSI escape sequence for bold
        ansi_rm = "\x1b[1mrm\x1b[0m -rf /"
        is_dang, key, desc = detect_dangerous_command(ansi_rm)
        assert is_dang is True, "ANSI codes should be stripped before detection"

    def test_detect_sql_delete_no_where(self):
        """SQL DELETE without WHERE is dangerous."""
        is_dang, key, desc = detect_dangerous_command("DELETE FROM users")
        assert is_dang is True
        assert key == "SQL DELETE without WHERE"

    def test_detect_fork_bomb(self):
        """Fork bomb pattern is detected."""
        is_dang, key, desc = detect_dangerous_command(":(){ :|:& };:")
        assert is_dang is True
        assert key == "fork bomb"

    def test_detect_kill_hermes(self):
        """pkill hermes (self-termination) is detected."""
        is_dang, key, desc = detect_dangerous_command("pkill hermes")
        assert is_dang is True
        assert key == "kill hermes/gateway process (self-termination)"

    def test_safe_git_operation(self):
        """Safe git operations are not flagged."""
        is_dang, key, desc = detect_dangerous_command("git status")
        assert is_dang is False
        is_dang, key, desc = detect_dangerous_command("git add .")
        assert is_dang is False
        is_dang, key, desc = detect_dangerous_command("git commit -m 'fix: bug'")
        assert is_dang is False


class TestApprovalState(unittest.TestCase):
    """Test approval state management."""

    def setUp(self):
        """Clear state before each test."""
        clear_session("test_session")

    def test_unapproved_returns_false(self):
        """Test 7: is_approved() returns False for unapproved patterns."""
        result = is_approved("test_session", "delete in root path")
        self.assertFalse(result)

    def test_approve_session(self):
        """Test 8: approve_session() allows subsequent is_approved() to return True."""
        approve_session("test_session", "delete in root path")
        result = is_approved("test_session", "delete in root path")
        self.assertTrue(result)

    def test_clear_session(self):
        """Clearing session removes approvals."""
        approve_session("test_session", "delete in root path")
        clear_session("test_session")
        result = is_approved("test_session", "delete in root path")
        self.assertFalse(result)

    def test_permanent_approval(self):
        """Permanent approvals persist across sessions."""
        clear_session("session1")
        clear_session("session2")
        approve_permanent("delete in root path")
        self.assertTrue(is_approved("session1", "delete in root path"))
        self.assertTrue(is_approved("session2", "delete in root path"))

    def test_pattern_key_aliases(self):
        """Test 9: Pattern key aliases work for legacy migrations."""
        # Canonical key should match alias
        aliases = _approval_key_aliases("delete in root path")
        self.assertIn("delete in root path", aliases)


class TestNormalization(unittest.TestCase):
    """Test command normalization for detection bypass prevention."""

    def test_null_byte_removal(self):
        """Null bytes are removed before detection."""
        cmd = "rm\x00 -rf /"
        is_dang, key, desc = detect_dangerous_command(cmd)
        self.assertTrue(is_dang)

    def test_nfkc_normalization_fullwidth(self):
        """Fullwidth Latin characters are NFKC normalized."""
        # Fullwidth 'rm' (U+FF52 U+FF4D)
        fullwidth = "\uff52\uff4d -rf /"
        is_dang, key, desc = detect_dangerous_command(fullwidth)
        self.assertTrue(is_dang)

    def test_nfkc_normalization_halfwidth_katakana(self):
        """Halfwidth Katakana is NFKC normalized."""
        # Halfwidth 'ka' (U+FF76)
        cmd = "rm\uff76 -rf /"
        is_dang, key, desc = detect_dangerous_command(cmd)
        self.assertTrue(is_dang)


class TestDangerousPatterns(unittest.TestCase):
    """Test that DANGEROUS_PATTERNS is comprehensive."""

    def test_patterns_compile(self):
        """All DANGEROUS_PATTERNS compile without error."""
        import re

        for pattern, description in DANGEROUS_PATTERNS:
            try:
                re.compile(pattern)
            except re.error as e:
                self.fail(f"Pattern '{pattern}' failed to compile: {e}")

    def test_has_40_plus_patterns(self):
        """DANGEROUS_PATTERNS has 40+ entries."""
        self.assertGreaterEqual(
            len(DANGEROUS_PATTERNS),
            40,
            f"Expected 40+ patterns, got {len(DANGEROUS_PATTERNS)}",
        )

    def test_covers_key_categories(self):
        """Key danger categories are covered."""
        categories = {
            "rm_rf": False,
            "chmod_777": False,
            "sql_drop": False,
            "fork_bomb": False,
            "kill_hermes": False,
        }
        for pattern, desc in DANGEROUS_PATTERNS:
            if "rm" in desc.lower() and "root" in desc.lower():
                categories["rm_rf"] = True
            if "777" in pattern or "666" in pattern:
                categories["chmod_777"] = True
            if "DROP" in pattern:
                categories["sql_drop"] = True
            if "fork bomb" in desc.lower():
                categories["fork_bomb"] = True
            if "pkill" in pattern and "hermes" in pattern:
                categories["kill_hermes"] = True

        for cat, found in categories.items():
            self.assertTrue(found, f"Category '{cat}' not found in DANGEROUS_PATTERNS")


if __name__ == "__main__":
    unittest.main([__file__, "-v"])
