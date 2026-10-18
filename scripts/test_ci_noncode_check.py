#!/usr/bin/env python3
"""Behavior tests for ci_noncode_check.py allowlist (epic-cc#1362).

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import unittest
from unittest import mock

import ci_noncode_check as cnc


class AllowlistTests(unittest.TestCase):
    def test_non_code_paths(self):
        for path in (
            "README.md",
            "docs/01-target.md",
            "docs/internals/design/38-x.md",
            "LICENSE",
            "LICENSES/MIT.txt",
            ".gitignore",
            "sub/.gitattributes",
            ".clang-format",
            ".editorconfig",
            "img/board.svg",
            "img/photo.png",
            "img/photo.jpg",
            "img/photo.jpeg",
            "img/photo.gif",
            "img/icon.ico",
            "scripts/bootstrap.sh",
            "scripts/install-git-hooks.sh",
            "Makefile",
        ):
            self.assertTrue(cnc.is_non_code([path]), path)

    def test_code_paths_fail_open(self):
        for path in (
            "crates/alloc/src/lib.rs",
            "src/main.c",
            "scripts/catalog.py",
            "scripts/ci-test.sh",
            "Cargo.toml",
            "Dockerfile",
            "pyproject.toml",
            ".github/workflows/ci.yml",
            ".github/workflows/hal-fixture-drift.yml",
            "sub/Makefile",
            "docs.md.bak",
        ):
            self.assertFalse(cnc.is_non_code([path]), path)

    def test_all_must_be_non_code(self):
        self.assertTrue(cnc.is_non_code(["README.md", "docs/x.md"]))
        self.assertFalse(cnc.is_non_code(["README.md", "crates/x/lib.rs"]))
        self.assertTrue(cnc.is_non_code([]))


class MainTests(unittest.TestCase):
    def test_missing_base_ref_is_usage_error(self):
        with mock.patch("sys.argv", ["ci_noncode_check.py"]):
            self.assertEqual(cnc.main(), 2)

    def run_main(self, files):
        payload = "\n".join(files) + ("\n" if files else "")
        completed = mock.Mock(stdout=payload)
        with (
            mock.patch("sys.argv", ["ci_noncode_check.py", "origin/master"]),
            mock.patch("subprocess.run", return_value=completed) as run,
            mock.patch("builtins.print") as printed,
        ):
            rc = cnc.main()
        run.assert_called_once_with(
            ["git", "diff", "--name-only", "origin/master...HEAD"],
            capture_output=True,
            text=True,
            check=True,
        )
        return rc, printed.call_args[0][0]

    def test_docs_only_prints_true(self):
        rc, word = self.run_main(["README.md", "docs/y.md"])
        self.assertEqual((rc, word), (0, "true"))

    def test_rust_change_prints_false(self):
        rc, word = self.run_main(["README.md", "crates/alloc/src/lib.rs"])
        self.assertEqual((rc, word), (0, "false"))


if __name__ == "__main__":
    unittest.main()
