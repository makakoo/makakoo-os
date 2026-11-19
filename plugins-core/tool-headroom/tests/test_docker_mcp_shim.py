"""Offline regression tests for the installer-generated Docker MCP shim."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class DockerMcpShimTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        installer = (Path(__file__).parents[1] / "install.sh").read_text()
        template = installer.split("  cat >\"$shim\" <<'EOF'\n", 1)[1].split("\nEOF", 1)[0]
        self.shim = self.root / "headroom-mcp-stdio"
        self.shim.write_text(template + "\n")
        self.wrapper = self.root / "headroom"
        self.wrapper.write_text('HEADROOM_IMAGE_DEFAULT=headroom-local:0.31.0-bounded\n')
        docker = self.root / "docker"
        docker.write_text('#!/usr/bin/env python3\nimport json, sys\nprint(json.dumps(sys.argv[1:]))\n')
        docker.chmod(0o755)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith('HEADROOM_')}
        self.env.update(HOME=str(self.root / 'home'), PATH=str(self.root) + ':' + os.environ['PATH'])

    def run_shim(self, args=(), **env):
        result = subprocess.run(['/bin/bash', str(self.shim), *args], env={**self.env, **env},
                                cwd=self.root, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_image_cache_and_stdio(self):
        args = self.run_shim()
        self.assertEqual(args[-3:], ['headroom-local:0.31.0-bounded', 'mcp', 'serve'])
        for value in ['--no-healthcheck', '-i', '--rm',
                      'XDG_CACHE_HOME=/tmp/headroom-home/.headroom/.cache',
                      'HF_HOME=/tmp/headroom-home/.headroom/.cache/huggingface']:
            self.assertIn(value, args)
        self.assertNotIn('-t', args)

    def test_explicit_image_override(self):
        self.assertEqual(self.run_shim(HEADROOM_DOCKER_IMAGE='custom:123')[-3], 'custom:123')

    def test_literal_quoted_images(self):
        for quote in ('', "'", '"'):
            with self.subTest(quote=quote):
                self.wrapper.write_text(f'HEADROOM_IMAGE_DEFAULT={quote}custom:123{quote}\n')
                self.assertEqual(self.run_shim()[-3], 'custom:123')

    def test_absent_wrapper_fallback(self):
        self.wrapper.unlink()
        self.assertEqual(self.run_shim()[-3], 'ghcr.io/chopratejas/headroom:latest')

    def test_never_evaluates_wrapper(self):
        self.wrapper.write_text('HEADROOM_IMAGE_DEFAULT=$(touch PWNED)\nexit 123\n')
        self.assertEqual(self.run_shim()[-3], 'ghcr.io/chopratejas/headroom:latest')
        self.assertFalse((self.root / 'PWNED').exists())

    def test_default_proxy(self):
        self.assertIn('HEADROOM_PROXY_URL=http://host.docker.internal:8787', self.run_shim())

    def test_host_loopback_env(self):
        for host in ('localhost', '127.0.0.1', '[::1]'):
            with self.subTest(host=host):
                self.assertIn('HEADROOM_PROXY_URL=https://host.docker.internal:8787/path?q=1',
                              self.run_shim(HEADROOM_PROXY_URL=f'https://{host}:8787/path?q=1'))

    def test_remote_proxy_unchanged(self):
        for url in ('https://proxy.example:443/api', 'http://localhost.example:8787',
                    'http://host.docker.internal:8787'):
            with self.subTest(url=url):
                self.assertIn('HEADROOM_PROXY_URL=' + url, self.run_shim(HEADROOM_PROXY_URL=url))

    def test_cli_proxy_forms(self):
        self.assertEqual(self.run_shim(['--proxy-url', 'http://localhost:8787', '--help'])[-3:],
                         ['--proxy-url', 'http://host.docker.internal:8787', '--help'])
        self.assertEqual(self.run_shim(['--proxy-url=http://127.0.0.1:8787'])[-1],
                         '--proxy-url=http://host.docker.internal:8787')

    def test_missing_proxy_value_fails(self):
        result = subprocess.run(['/bin/bash', str(self.shim), '--proxy-url'], env=self.env,
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, '')

    def test_argument_boundaries_preserved(self):
        self.assertEqual(self.run_shim(['--example', 'a b;$(echo c)'])[-2:],
                         ['--example', 'a b;$(echo c)'])

    def install_running_container_docker_mock(self):
        docker = self.root / "docker"
        docker.write_text(
            '#!/usr/bin/env python3\n'
            'import json, sys\n'
            'args = sys.argv[1:]\n'
            'if args[:3] == ["inspect", "-f", "{{.State.Running}}"]:\n'
            '    print("true")\n'
            '    raise SystemExit(0)\n'
            'print(json.dumps(args))\n'
        )
        docker.chmod(0o755)

    def test_exec_fast_path_when_proxy_container_running(self):
        self.install_running_container_docker_mock()
        args = self.run_shim()
        self.assertEqual(
            args,
            ['exec', '-i', '-e', 'HEADROOM_PROXY_URL=http://127.0.0.1:8787',
             'headroom-default', 'headroom', 'mcp', 'serve'],
        )

    def test_exec_fast_path_maps_host_docker_internal_8787_to_loopback(self):
        self.install_running_container_docker_mock()
        args = self.run_shim(
            ['--proxy-url', 'http://host.docker.internal:8787'],
            HEADROOM_PROXY_URL='http://host.docker.internal:8787',
        )
        self.assertEqual(
            args,
            ['exec', '-i', '-e', 'HEADROOM_PROXY_URL=http://127.0.0.1:8787',
             'headroom-default', 'headroom', 'mcp', 'serve',
             '--proxy-url', 'http://127.0.0.1:8787'],
        )

    def test_exec_fast_path_preserves_custom_port_and_remote_proxy(self):
        self.install_running_container_docker_mock()
        args = self.run_shim(HEADROOM_PROXY_URL='http://127.0.0.1:9999')
        self.assertIn('HEADROOM_PROXY_URL=http://host.docker.internal:9999', args)
        remote = self.run_shim(HEADROOM_PROXY_URL='https://proxy.example:443/api')
        self.assertIn('HEADROOM_PROXY_URL=https://proxy.example:443/api', remote)

    def test_exec_fast_path_bypassed_on_explicit_image_or_opt_out(self):
        self.install_running_container_docker_mock()
        by_image = self.run_shim(HEADROOM_DOCKER_IMAGE='custom:999')
        self.assertEqual(by_image[0], 'run')
        self.assertEqual(by_image[-3], 'custom:999')
        by_flag = self.run_shim(HEADROOM_MCP_NO_EXEC='1')
        self.assertEqual(by_flag[0], 'run')


if __name__ == '__main__':
    unittest.main()
