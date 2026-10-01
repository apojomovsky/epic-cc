"""Coverage for size-report.py's baseline parsing.

The size ladder records `-O2` rows under the same names as the `-Os`
rows (epic-cc#839). The report renders `-Os` measurements only, so the
parser must ignore `-O2` rows: joining against them would print phantom
deltas for rows that never moved.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "size-report.py"

# Filename has a hyphen, so `import` cannot name it.
_spec = importlib.util.spec_from_file_location("size_report", SCRIPT)
sr = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sr)


class ParseBaselineProfiles(unittest.TestCase):
    def test_o2_rows_do_not_shadow_os_rows(self):
        text = """\
[[entry]]
name = "add-18f4550"
device = "18F4550"
flash_words = 19
ram_bytes = 8

[[entry]]
name = "add-18f4550"
device = "18F4550"
profile = "O2"
flash_words = 18
ram_bytes = 8
"""
        entries = sr.parse_baseline(text)
        self.assertEqual(entries["add-18f4550"]["flash_words"], 19)

    def test_explicit_os_rows_still_parse(self):
        text = """\
[[entry]]
name = "add-18f4550"
device = "18F4550"
profile = "Os"
flash_words = 19
ram_bytes = 8
"""
        entries = sr.parse_baseline(text)
        self.assertEqual(entries["add-18f4550"]["flash_words"], 19)


if __name__ == "__main__":
    unittest.main()
