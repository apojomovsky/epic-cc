"""Stub-harness coverage for the .githooks/pre-push suite stamp.

`epic-tasks push` retries a dropped connection once; the retry re-runs
this hook, so the hook stamps the tree `make ci-local` already passed
and skips the suite on a stamp hit (epic-cc#1296, same scheme as
epic-tasks#754). The stamp covers only the expensive suite: the force
guard and the cheap prose gate run on every push.

The harness runs the real hook in a scratch repo with `git` and `make`
stubs on PATH (canned tree and hook hashes, recorded suite calls) and a
stub scripts/prose-diff.sh under the scratch repo (the hook invokes it
by relative path, so the scratch copy is what runs).

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
HOOK = ROOT / ".githooks" / "pre-push"

TREE = "1" * 40
HOOK_SUM = "2" * 40

GIT_STUB = """\
#!/usr/bin/env bash
set -u
echo "$*" >> "$STUB_DIR/git_calls"
if [ "${1:-}" = "rev-parse" ] && [ "${2:-}" = "--absolute-git-dir" ]; then
    cat "$STUB_DIR/gitdir"
elif [ "${1:-}" = "rev-parse" ]; then
    cat "$STUB_DIR/tree"
elif [ "${1:-}" = "hash-object" ]; then
    cat "$STUB_DIR/hook_sum"
elif [ "${1:-}" = "merge-base" ]; then
    exit "$(cat "$STUB_DIR/merge_base_code")"
else
    echo "unexpected git call: $*" >&2
    exit 99
fi
"""

MAKE_STUB = """\
#!/usr/bin/env bash
set -u
echo "$*" >> "$STUB_DIR/make_calls"
exit "$(cat "$STUB_DIR/make_code")"
"""

PROSE_STUB = """\
#!/usr/bin/env bash
set -u
echo "$*" >> "$STUB_DIR/prose_calls"
exit "$(cat "$STUB_DIR/prose_code")"
"""


class PrePushStamp(unittest.TestCase):
    def run_hook(
        self,
        *,
        stamp=None,
        tree=TREE,
        hook_sum=HOOK_SUM,
        make_code=0,
        prose_code=0,
        merge_base_code=0,
        push_lines="",
    ):
        """Run the real hook against stubbed git/make/prose-diff.

        Returns (returncode, stdout, make_calls, prose_calls, stamp_text).
        stamp_text is the stamp file content after the run, or None when
        the hook wrote nothing.
        """
        with tempfile.TemporaryDirectory() as tmp:
            stub_dir = pathlib.Path(tmp)
            bin_dir = stub_dir / "bin"
            bin_dir.mkdir()
            for name, body in (("git", GIT_STUB), ("make", MAKE_STUB)):
                path = bin_dir / name
                path.write_text(body)
                path.chmod(0o755)
            repo = stub_dir / "repo"
            scripts = repo / "scripts"
            scripts.mkdir(parents=True)
            prose = scripts / "prose-diff.sh"
            prose.write_text(PROSE_STUB)
            prose.chmod(0o755)
            fake_git = stub_dir / "fake-git"
            fake_git.mkdir()
            (stub_dir / "gitdir").write_text(str(fake_git))
            (stub_dir / "tree").write_text(tree)
            (stub_dir / "hook_sum").write_text(hook_sum)
            (stub_dir / "merge_base_code").write_text(str(merge_base_code))
            (stub_dir / "make_code").write_text(str(make_code))
            (stub_dir / "prose_code").write_text(str(prose_code))
            for name in ("git_calls", "make_calls", "prose_calls"):
                (stub_dir / name).write_text("")
            stamp_file = fake_git / "epic-pre-push-stamp"
            if stamp is not None:
                stamp_file.write_text(stamp)
            env = {
                **os.environ,
                "STUB_DIR": str(stub_dir),
                "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
            }
            proc = subprocess.run(
                ["bash", str(HOOK)],
                input=push_lines,
                cwd=str(repo),
                env=env,
                capture_output=True,
                text=True,
                timeout=60,
            )
            make_calls = (stub_dir / "make_calls").read_text().splitlines()
            prose_calls = (stub_dir / "prose_calls").read_text().splitlines()
            stamp_text = stamp_file.read_text() if stamp_file.exists() else None
            outcome = (
                proc.returncode,
                proc.stdout,
                make_calls,
                prose_calls,
                stamp_text,
            )
        return outcome

    def test_miss_runs_suite_and_stamps(self):
        rc, out, make_calls, prose_calls, stamp_text = self.run_hook()
        self.assertEqual(rc, 0)
        self.assertEqual(make_calls, ["ci-local"])
        self.assertEqual(prose_calls, ["--verify"])
        self.assertEqual(stamp_text, f"{TREE} {HOOK_SUM}\n")
        self.assertIn("make ci-local", out)

    def test_hit_skips_suite_but_runs_prose(self):
        rc, out, make_calls, prose_calls, stamp_text = self.run_hook(
            stamp=f"{TREE} {HOOK_SUM}\n"
        )
        self.assertEqual(rc, 0)
        self.assertEqual(make_calls, [])
        self.assertEqual(prose_calls, ["--verify"])
        self.assertIn("already passed", out)

    def test_tree_change_reruns_suite(self):
        rc, _, make_calls, _, stamp_text = self.run_hook(
            stamp=f"{'3' * 40} {HOOK_SUM}\n"
        )
        self.assertEqual(rc, 0)
        self.assertEqual(make_calls, ["ci-local"])
        self.assertEqual(stamp_text, f"{TREE} {HOOK_SUM}\n")

    def test_hook_change_reruns_suite(self):
        rc, _, make_calls, _, stamp_text = self.run_hook(stamp=f"{TREE} {'4' * 40}\n")
        self.assertEqual(rc, 0)
        self.assertEqual(make_calls, ["ci-local"])
        self.assertEqual(stamp_text, f"{TREE} {HOOK_SUM}\n")

    def test_suite_failure_refuses_and_leaves_no_stamp(self):
        rc, _, make_calls, prose_calls, stamp_text = self.run_hook(make_code=1)
        self.assertEqual(rc, 1)
        self.assertEqual(make_calls, ["ci-local"])
        self.assertEqual(prose_calls, [])
        self.assertIsNone(stamp_text)

    def test_prose_failure_refuses_despite_stamp_hit(self):
        rc, _, make_calls, prose_calls, _ = self.run_hook(
            stamp=f"{TREE} {HOOK_SUM}\n", prose_code=1
        )
        self.assertEqual(rc, 1)
        self.assertEqual(make_calls, [])
        self.assertEqual(prose_calls, ["--verify"])

    def test_suite_pass_then_prose_failure_still_stamps(self):
        rc, _, make_calls, prose_calls, stamp_text = self.run_hook(prose_code=1)
        self.assertEqual(rc, 1)
        self.assertEqual(make_calls, ["ci-local"])
        self.assertEqual(prose_calls, ["--verify"])
        self.assertEqual(stamp_text, f"{TREE} {HOOK_SUM}\n")

    def test_force_push_refused_on_stamp_hit(self):
        rc, _, make_calls, prose_calls, _ = self.run_hook(
            stamp=f"{TREE} {HOOK_SUM}\n",
            merge_base_code=1,
            push_lines="refs/heads/x aaaa refs/heads/x bbbb\n",
        )
        self.assertEqual(rc, 1)
        self.assertEqual(make_calls, [])
        self.assertEqual(prose_calls, [])


if __name__ == "__main__":
    unittest.main()
