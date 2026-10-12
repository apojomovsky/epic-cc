"""Stub-harness coverage for scripts/warm-toolchain-cache.sh.

The script retries only transient registry errors. A 429 counts only on
the final build log line (the ERROR BuildKit prints last), so an
earlier tolerated 429 warning followed by an unrelated failure fails at
once instead of retrying for about 52 minutes.

The harness stubs `docker` (canned output and exit code per call) and
`sleep` (recorded, never waited) on PATH, then runs the real script.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "warm-toolchain-cache.sh"

DOCKER_STUB = """\
#!/usr/bin/env bash
# Canned docker: call n prints call-n output, exits call-n.code.
set -u
n=$(cat "$STUB_DIR/calls")
cat "$STUB_DIR/call-$n"
code=$(cat "$STUB_DIR/call-$n.code")
echo "$((n + 1))" > "$STUB_DIR/calls"
exit "$code"
"""

SLEEP_STUB = """\
#!/usr/bin/env bash
echo "$*" >> "$STUB_DIR/sleeps"
"""

FINAL_429 = """\
#1 [internal] load build definition from Dockerfile
ERROR: failed to solve: unexpected status from HEAD request to \\
  https://registry-1.docker.io/v2/library/ubuntu/manifests/sha256:abc: \\
  429 Too Many Requests
"""

EARLY_429_THEN_UNRELATED = """\
#4 exporting cache to ghcr.io/example/toolchain: \
  retrying in 5s (429 Too Many Requests, tolerated)
#5 [stage-0 3/5] RUN apt-get install -y foo
ERROR: failed to solve: process "/bin/sh -c apt-get install -y foo" did \\
  not complete successfully: exit code: 100
"""

FINAL_BARE_502 = """\
#1 [internal] load build definition from Dockerfile
ERROR: failed to solve: unexpected status from GET request to \
  https://registry-1.docker.io/v2/library/ubuntu/manifests/sha256:abc: \
  502
"""

BARE_502_WITHOUT_REGISTRY = """\
#5 [stage-0 3/5] RUN apt-get install -y foo
ERROR: failed to solve: unexpected status: 502 from mirror-proxy.internal
"""


class WarmToolchainCache(unittest.TestCase):
    def run_script(self, calls):
        """Run the script against canned (output, code) docker calls.

        Returns (returncode, docker_calls, sleeps).
        """
        with tempfile.TemporaryDirectory() as tmp:
            stub_dir = pathlib.Path(tmp)
            bin_dir = stub_dir / "bin"
            bin_dir.mkdir()
            for name, body in (("docker", DOCKER_STUB), ("sleep", SLEEP_STUB)):
                path = bin_dir / name
                path.write_text(body)
                path.chmod(0o755)
            (stub_dir / "calls").write_text("0")
            (stub_dir / "sleeps").write_text("")
            for n, (output, code) in enumerate(calls):
                (stub_dir / f"call-{n}").write_text(output)
                (stub_dir / f"call-{n}.code").write_text(str(code))
            env = {
                **os.environ,
                "CACHE_REF": "ghcr.io/example/toolchain",
                "STUB_DIR": str(stub_dir),
                "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
            }
            proc = subprocess.run(
                ["bash", str(SCRIPT), "--target", "ci"],
                env=env,
                capture_output=True,
                text=True,
                timeout=60,
            )
            docker_calls = int((stub_dir / "calls").read_text().strip())
            sleeps = (stub_dir / "sleeps").read_text().split()
            return proc.returncode, docker_calls, sleeps

    def test_early_429_then_unrelated_failure_fails_at_once(self):
        rc, docker_calls, sleeps = self.run_script([(EARLY_429_THEN_UNRELATED, 1)])
        self.assertNotEqual(rc, 0)
        self.assertEqual(docker_calls, 1)
        self.assertEqual(sleeps, [])

    def test_final_429_then_success_retries_once(self):
        rc, docker_calls, sleeps = self.run_script([(FINAL_429, 1), ("", 0)])
        self.assertEqual(rc, 0)
        self.assertEqual(docker_calls, 2)
        self.assertEqual(len(sleeps), 1)

    def test_persistent_final_429_exhausts_retries(self):
        rc, docker_calls, sleeps = self.run_script([(FINAL_429, 1)] * 6)
        self.assertNotEqual(rc, 0)
        self.assertEqual(docker_calls, 6)
        self.assertEqual(len(sleeps), 5)

    def test_bare_code_registry_5xx_retries_once(self):
        rc, docker_calls, sleeps = self.run_script([(FINAL_BARE_502, 1), ("", 0)])
        self.assertEqual(rc, 0)
        self.assertEqual(docker_calls, 2)
        self.assertEqual(len(sleeps), 1)

    def test_bare_code_without_registry_fails_at_once(self):
        rc, docker_calls, sleeps = self.run_script([(BARE_502_WITHOUT_REGISTRY, 1)])
        self.assertNotEqual(rc, 0)
        self.assertEqual(docker_calls, 1)
        self.assertEqual(sleeps, [])


if __name__ == "__main__":
    unittest.main()
