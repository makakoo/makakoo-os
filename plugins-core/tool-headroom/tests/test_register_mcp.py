"""Unit tests for register_mcp.py across all Makakoo CLI hosts."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest


SCRIPT = Path(__file__).parents[1] / "scripts" / "register_mcp.py"


class RegisterMcpTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)

    def run_registrar(self, *extra_args: str) -> subprocess.CompletedProcess:
        cmd = [
            sys.executable,
            str(SCRIPT),
            "--home",
            str(self.home),
            "--command",
            str(self.home / ".local" / "bin" / "headroom-mcp-stdio"),
            "--proxy-url",
            "http://127.0.0.1:8787",
            *extra_args,
        ]
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        self.assertEqual(res.returncode, 0, res.stderr)
        return res

    def test_registers_all_detected_hosts_with_valid_configs_and_idempotency(self):
        # Seed detected directories and realistic pre-existing configs.
        (self.home / ".claude").mkdir()
        (self.home / ".claude.json").write_text(json.dumps({"mcpServers": {"other": {"command": "x"}}}) + "\n")
        (self.home / ".claude" / ".claude.json").write_text(json.dumps({"mcpServers": {"headroom": {"command": "old"}}}) + "\n")

        (self.home / ".gemini" / "config").mkdir(parents=True)
        (self.home / ".gemini" / "antigravity-cli").mkdir(parents=True)
        (self.home / ".gemini" / "config" / "mcp_config.json").write_text(
            json.dumps({"mcpServers": {"stitch": {"serverUrl": "https://example.invalid"}}}) + "\n"
        )
        (self.home / ".gemini" / "settings.json").write_text(json.dumps({"mcpServers": {}}) + "\n")

        (self.home / ".codex").mkdir()
        (self.home / ".codex" / "config.toml").write_text(
            'model = "gpt-5.6-sol"\n\n'
            '[model_providers.headroom]\nbase_url = "http://127.0.0.1:8787/v1"\n\n'
            '[mcp_servers.headroom]\ncommand = "/old"\nenabled = false\n\n'
            '[mcp_servers.headroom.env]\nHEADROOM_PROXY_URL = "http://old"\n\n'
            '[mcp_servers.harvey]\ncommand = "makakoo-mcp"\n'
        )

        (self.home / ".grok").mkdir()
        (self.home / ".grok" / "config.toml").write_text(
            '[cli]\ninstaller = "internal"\n\n'
            '[mcp_servers.headroom]\ncommand = "/usr/local/bin/docker"\nargs = ["exec", "headroom-default"]\nenabled = true\n\n'
            '[[marketplace.sources]]\nname = "xAI Official"\n'
        )

        (self.home / ".config" / "opencode").mkdir(parents=True)
        (self.home / ".config" / "opencode" / "opencode.json").write_text(json.dumps({"mcp": {}}) + "\n")
        (self.home / ".vibe").mkdir()
        (self.home / ".vibe" / "config.toml").write_text('permission = "always"\n')
        (self.home / ".qwen").mkdir()
        (self.home / ".qwen" / "settings.json").write_text(json.dumps({"mcpServers": {}}) + "\n")
        (self.home / ".cursor").mkdir()
        (self.home / ".cursor" / "mcp.json").write_text(json.dumps({"mcpServers": {}}) + "\n")
        (self.home / ".kimi-code").mkdir()
        (self.home / ".kimi-code" / "mcp.json").write_text(json.dumps({"mcpServers": {}}) + "\n")

        first = self.run_registrar()
        for host in ("claude", "agy", "gemini", "codex", "grok", "opencode", "vibe", "qwen", "cursor", "kimi"):
            self.assertIn(f"{host}: updated", first.stdout)

        shim = str(self.home / ".local" / "bin" / "headroom-mcp-stdio")

        # Claude primary + secondary both get alwaysLoad=True and stdio type.
        c1 = json.loads((self.home / ".claude.json").read_text())
        self.assertIn("other", c1["mcpServers"])
        self.assertTrue(c1["mcpServers"]["headroom"]["alwaysLoad"])
        self.assertEqual(c1["mcpServers"]["headroom"]["command"], shim)
        self.assertEqual(c1["mcpServers"]["headroom"]["args"], [])
        c2 = json.loads((self.home / ".claude" / ".claude.json").read_text())
        self.assertTrue(c2["mcpServers"]["headroom"]["alwaysLoad"])
        self.assertEqual(c2["mcpServers"]["headroom"]["command"], shim)

        # AGY preserves stitch and updates headroom.
        agy = json.loads((self.home / ".gemini" / "config" / "mcp_config.json").read_text())
        self.assertIn("stitch", agy["mcpServers"])
        self.assertEqual(agy["mcpServers"]["headroom"]["command"], shim)
        self.assertEqual(agy["mcpServers"]["headroom"]["args"], [])

        # Codex parses as valid TOML, re-enables headroom, preserves model_providers & harvey,
        # and omits `description` (which Codex v0.169+ flags as unrecognized).
        codex = tomllib.loads((self.home / ".codex" / "config.toml").read_text())
        self.assertEqual(codex["model_providers"]["headroom"]["base_url"], "http://127.0.0.1:8787/v1")
        self.assertEqual(codex["mcp_servers"]["harvey"]["command"], "makakoo-mcp")
        self.assertTrue(codex["mcp_servers"]["headroom"]["enabled"])
        self.assertNotIn("description", codex["mcp_servers"]["headroom"])
        for tool in ("headroom_compress", "headroom_retrieve", "headroom_stats"):
            self.assertEqual(codex["mcp_servers"]["headroom"]["tools"][tool]["approval_mode"], "approve")
        self.assertEqual(codex["mcp_servers"]["headroom"]["command"], shim)
        self.assertEqual(codex["mcp_servers"]["headroom"]["args"], [])

        # Grok parses as valid TOML, preserves [[marketplace.sources]], updates headroom.
        grok = tomllib.loads((self.home / ".grok" / "config.toml").read_text())
        self.assertEqual(grok["marketplace"]["sources"][0]["name"], "xAI Official")
        self.assertTrue(grok["mcp_servers"]["headroom"]["enabled"])
        self.assertEqual(grok["mcp_servers"]["headroom"]["startup_timeout_sec"], 120)
        self.assertEqual(grok["mcp_servers"]["headroom"]["command"], shim)

        # Vibe parses as valid TOML.
        vibe = tomllib.loads((self.home / ".vibe" / "config.toml").read_text())
        self.assertEqual(vibe["mcp_servers"][0]["name"], "headroom")
        self.assertEqual(vibe["mcp_servers"][0]["command"], shim)

        # OpenCode uses local command list.
        oc = json.loads((self.home / ".config" / "opencode" / "opencode.json").read_text())
        self.assertEqual(oc["mcp"]["headroom"]["command"], [shim])

        # Kimi preserves startupTimeoutMs.
        kimi = json.loads((self.home / ".kimi-code" / "mcp.json").read_text())
        self.assertEqual(kimi["mcpServers"]["headroom"]["startupTimeoutMs"], 120000)
        self.assertEqual(kimi["mcpServers"]["headroom"]["command"], shim)

        # Second run is 100% idempotent.
        second = self.run_registrar()
        for host in ("claude", "agy", "gemini", "codex", "grok", "opencode", "vibe", "qwen", "cursor", "kimi"):
            self.assertIn(f"{host}: unchanged", second.stdout)

    def test_skips_undetected_hosts(self):
        res = self.run_registrar()
        for host in ("claude", "agy", "gemini", "codex", "grok", "opencode", "vibe", "qwen", "cursor", "kimi"):
            self.assertIn(f"{host}: skipped", res.stdout)


if __name__ == "__main__":
    unittest.main()
