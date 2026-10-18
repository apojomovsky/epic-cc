"""Coverage for scripts/check_release_bundle.py (epic-cc#1336).

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import pathlib
import tempfile
import unittest
import zipfile

import check_release_bundle as crb


def make_zip(directory, name, members):
    path = pathlib.Path(directory) / name
    with zipfile.ZipFile(path, "w") as zf:
        for m in members:
            zf.writestr(m, "x")
    return str(path)


class CheckReleaseBundleTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = tmp.name

    def test_complete_linux_bundle_passes(self):
        z = make_zip(
            self.dir,
            "epic-cc-1.0.0-x86_64-linux.zip",
            [
                "epic-cc-1.0.0-x86_64-linux/epic-cc",
                "epic-cc-1.0.0-x86_64-linux/epic-cc-gdbserver",
            ],
        )
        self.assertEqual(crb.missing_members(z), [])

    def test_windows_bundle_without_gdbserver_fails(self):
        z = make_zip(
            self.dir,
            "epic-cc-1.0.0-x86_64-windows.zip",
            ["epic-cc-1.0.0-x86_64-windows/epic-cc.exe"],
        )
        self.assertEqual(
            crb.missing_members(z),
            ["epic-cc-1.0.0-x86_64-windows/epic-cc-gdbserver.exe"],
        )
        self.assertEqual(crb.main([z]), 1)

    def test_windows_bundle_needs_exe_suffix(self):
        z = make_zip(
            self.dir,
            "epic-cc-1.0.0-x86_64-windows.zip",
            [
                "epic-cc-1.0.0-x86_64-windows/epic-cc",
                "epic-cc-1.0.0-x86_64-windows/epic-cc-gdbserver",
            ],
        )
        self.assertEqual(len(crb.missing_members(z)), 2)

    def test_backslash_separators_accepted(self):
        z = make_zip(
            self.dir,
            "epic-cc-1.0.0-x86_64-windows.zip",
            [
                "epic-cc-1.0.0-x86_64-windows\\epic-cc.exe",
                "epic-cc-1.0.0-x86_64-windows\\epic-cc-gdbserver.exe",
            ],
        )
        self.assertEqual(crb.missing_members(z), [])

    def test_any_failing_zip_fails_the_run(self):
        good = make_zip(
            self.dir,
            "epic-cc-1.0.0-x86_64-linux.zip",
            [
                "epic-cc-1.0.0-x86_64-linux/epic-cc",
                "epic-cc-1.0.0-x86_64-linux/epic-cc-gdbserver",
            ],
        )
        bad = make_zip(self.dir, "epic-cc-1.0.0-x86_64-windows.zip", [])
        self.assertEqual(crb.main([good, bad]), 1)
        self.assertEqual(crb.main([good]), 0)


if __name__ == "__main__":
    unittest.main()
