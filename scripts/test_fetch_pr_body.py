#!/usr/bin/env python3
"""Behavior tests for fetch_pr_body.sh (epic-cc#1440).

The delta judge lifts the HOLD for rows the PR names with `delta-ok:` on
their own line, and must read the body as it stands when the job runs: a
re-run replays the triggering event, whose body predates any edit. The
tests stub `gh` on PATH: the fetch must write that body verbatim (quotes
and $(...) reach the file, never a shell) and fail loudly, judging
nothing, when the read itself fails.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import json
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "fetch_pr_body.sh"

GH_STUB = """\
#!/usr/bin/env python3
# Emulate `gh pr view --json body --jq <expr>` for one canned payload:
# a raw body under --jq, the JSON envelope without it, and the gojq
# rule that a null body prints empty under `// ""` and "null" without.
import json
import os
import sys

if os.environ.get("STUB_FAIL") == "1":
    sys.stderr.write("HTTP 403: Resource not accessible by integration\\n")
    sys.exit(1)
payload = json.loads(os.environ["STUB_BODY"])
args = sys.argv[1:]
if "--jq" in args:
    expr = args[args.index("--jq") + 1]
    body = payload.get("body")
    if body is None and "//" in expr:
        body = ""
    sys.stdout.write("null" if body is None else body + "\\n")
else:
    sys.stdout.write(json.dumps(payload) + "\\n")
"""

BODY = (
    "Closes #1440\n"
    "\n"
    'A `delta-ok:` line names the row, "nothing else" does.\n'
    "\n"
    "delta-ok: hal-pic18-menu-demo-18f4550@O1@O1\n"
    "Cost is $(echo not-a-command) dollars.\n"
)


class FetchPrBodyTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.bindir = pathlib.Path(self.tmp.name) / "bin"
        self.bindir.mkdir()
        self.gh = self.bindir / "gh"
        self.gh.write_text(GH_STUB)
        self.gh.chmod(self.gh.stat().st_mode | stat.S_IXUSR)
        self.out = pathlib.Path(self.tmp.name) / "pr-body.md"

    def run_script(self, *args, stub_body="null", stub_fail=False):
        env = dict(
            os.environ,
            PATH=f"{self.bindir}{os.pathsep}{os.environ['PATH']}",
            STUB_BODY=stub_body,
        )
        if stub_fail:
            env["STUB_FAIL"] = "1"
        argv = ["bash", str(SCRIPT), *args] if args else ["bash", str(SCRIPT)]
        if args:
            argv += [str(self.out), "o/r"]
        return subprocess.run(argv, capture_output=True, text=True, env=env)

    def test_live_body_lands_verbatim(self):
        proc = self.run_script("1440", stub_body=json.dumps({"body": BODY}))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.out.read_bytes(), (BODY + "\n").encode())

    def test_null_body_writes_empty_not_null(self):
        proc = self.run_script("1440", stub_body='{"body": null}')
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.out.read_bytes(), b"\n")

    def test_failed_read_fails_loudly(self):
        proc = self.run_script("1440", stub_body='{"body": "stale"}', stub_fail=True)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("1440", proc.stderr)

    def test_usage_error_without_args(self):
        proc = self.run_script()
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("usage", proc.stderr)


if __name__ == "__main__":
    unittest.main()
