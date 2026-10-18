#!/usr/bin/env python3
"""Behavior tests for check-hal-fixture-drift.py (epic-cc#1362).

Run by scripts/ci-test.sh alongside the cargo suites. The live check
needs a real epic-hal clone, so these tests pin the exit-code contract
(0 clean, 1 drift, 2 stale pin or unusable checkout) with stubbed git
and stubbed file lists instead.
"""

import importlib.util
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).resolve().parent / "check-hal-fixture-drift.py"
_spec = importlib.util.spec_from_file_location("hal_fixture_drift", SCRIPT)
hfd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(hfd)


def completed(returncode=0, stdout=""):
    return mock.Mock(returncode=returncode, stdout=stdout, stderr="")


class ConfigLineTests(unittest.TestCase):
    def test_first_epic_config_line(self):
        blob = b"// header\nEPIC_CONFIG(a)\nEPIC_CONFIG(b)\n"
        self.assertEqual(hfd.config_line(blob), "EPIC_CONFIG(a)")

    def test_missing_line_is_empty(self):
        self.assertEqual(hfd.config_line(b"int x;\n"), "")


class PinTests(unittest.TestCase):
    def test_pin_is_40_hex(self):
        self.assertRegex(hfd.HAL_PIN, r"^[0-9a-f]{40}$")


class CheckFilesTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.vendor = pathlib.Path(tmp.name)
        self.files_patch = mock.patch.object(
            hfd, "FILES", [("vendored/a.h", "upstream/a.h")]
        )
        self.vendor_patch = mock.patch.object(hfd, "VENDOR", self.vendor)
        self.files_patch.start()
        self.vendor_patch.start()
        self.addCleanup(self.files_patch.stop)
        self.addCleanup(self.vendor_patch.stop)

    def write_vendored(self, rel, data=b"x"):
        target = self.vendor / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)

    def test_matching_bytes_pass(self):
        self.write_vendored("vendored/a.h", b"same")
        with mock.patch.object(hfd, "run", return_value=completed(0, "same")):
            self.assertEqual(hfd.check_files("hal-root", "pin"), [])

    def test_drifted_bytes_fail(self):
        self.write_vendored("vendored/a.h", b"old")
        with mock.patch.object(hfd, "run", return_value=completed(0, "new")):
            (failure,) = hfd.check_files("hal-root", "pin")
        self.assertIn("drifted", failure)

    def test_missing_vendored_file_fails(self):
        with mock.patch.object(hfd, "run", return_value=completed(0, "new")):
            (failure,) = hfd.check_files("hal-root", "pin")
        self.assertIn("missing vendored file", failure)

    def test_missing_upstream_blob_fails(self):
        self.write_vendored("vendored/a.h", b"old")
        with mock.patch.object(hfd, "run", return_value=completed(1, "")):
            (failure,) = hfd.check_files("hal-root", "pin")
        self.assertIn("missing upstream blob", failure)

    def test_unmapped_file_fails_provenance_and_config_skipped(self):
        self.write_vendored("vendored/a.h", b"same")
        self.write_vendored("stray.h", b"s")
        self.write_vendored("PROVENANCE.md", b"p")
        self.write_vendored("demo/config.c", b"c")
        self.write_vendored("demo/config_18F4550.c", b"c")
        with mock.patch.object(hfd, "run", return_value=completed(0, "same")):
            failures = hfd.check_files("hal-root", "pin")
        self.assertEqual(len(failures), 1)
        self.assertIn("unmapped vendored file: stray.h", failures[0])


class GeneratedConfigTests(unittest.TestCase):
    def test_failed_regen_reports_last_stderr_line(self):
        with mock.patch.object(
            hfd,
            "run",
            return_value=completed(
                1,
                "",
            ),
        ) as run:
            run.return_value.stderr = "boom\nlast line"
            blob, err = hfd.generated_config("hal-root", "m", "mcu", "v")
        self.assertIsNone(blob)
        self.assertEqual(err, ["last line"])


class MainExitCodeTests(unittest.TestCase):
    def hal_root(self, tmp, with_build_script=True):
        root = pathlib.Path(tmp) / "hal"
        scripts = root / "scripts"
        scripts.mkdir(parents=True)
        if with_build_script:
            (scripts / "epic_build.py").write_text("x")
        return str(root)

    def run_main(self, hal_root, run_stub, files_failures=(), config_failures=()):
        argv = ["check-hal-fixture-drift.py", "--hal-root", hal_root]
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(hfd, "run", side_effect=run_stub),
            mock.patch.object(hfd, "check_files", return_value=list(files_failures)),
            mock.patch.object(hfd, "check_configs", return_value=list(config_failures)),
        ):
            return hfd.main()

    def test_not_a_hal_checkout_is_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(
                self.run_main(self.hal_root(tmp, False), lambda *a, **k: None), 2
            )

    def test_unknown_pin_is_2(self):
        def run(*args, **kwargs):
            return completed(1, "")

        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self.run_main(self.hal_root(tmp), run), 2)

    def test_drift_is_1(self):
        def run(*args, **kwargs):
            if "cat-file" in args:
                return completed(0, "")
            raise AssertionError(f"unexpected git call: {args}")

        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(
                self.run_main(self.hal_root(tmp), run, files_failures=["drifted: x"]),
                1,
            )

    def test_clean_matching_tip_is_0(self):
        def run(*args, **kwargs):
            if "cat-file" in args:
                return completed(0, "")
            if "rev-parse" in args:
                return completed(0, hfd.HAL_PIN + "\n")
            raise AssertionError(f"unexpected git call: {args}")

        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self.run_main(self.hal_root(tmp), run), 0)

    def test_clean_stale_pin_is_2(self):
        def run(*args, **kwargs):
            if "cat-file" in args:
                return completed(0, "")
            if "rev-parse" in args:
                return completed(0, "0" * 40 + "\n")
            raise AssertionError(f"unexpected git call: {args}")

        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self.run_main(self.hal_root(tmp), run), 2)


if __name__ == "__main__":
    unittest.main()
