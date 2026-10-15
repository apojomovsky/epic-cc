"""Public text must not carry XC8 size or speed figures (epic-hal#450).

XC8 numbers are confidential under the XC8 licence (section 8) and live in
the private epic-benchmarks repository. The rule: a mention of XC8 within a
line or two of a measurement (a number of three or more digits with a size or
cycle unit, a ratio, or a used/total pair) is rejected in tracked docs and
manifest strings. Power-of-two figures are device capacities
(512 words, 2048 bytes), not measurements, and version numbers, error codes
and part numbers carry no unit, so none of them trip the rule."""

import pathlib
import re
import subprocess
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCANNED = (".md", ".toml")
SKIPPED_PARTS = ("third_party/", "vendor/", "fixtures/")

_XC8 = re.compile(r"xc8", re.IGNORECASE)
_NOISE = re.compile(
    r"https?://\S+|xc8[-_ /]*v?\d+(\.\d+)*|\berror \d+|\(\d{3,4}\)|\b0x[0-9a-f]+\b"
    r"|\bDS\d+\w*|\bv\d+\.\d+\b|#\d+|\b\d{4}-\d\d-\d\d\b",
    re.IGNORECASE,
)
_UNIT = r"(?:words?|bytes?|B|KB|KW|cycles?|instructions?|insns?)"
_SIZE = re.compile(r"(?<![\w.])(\d[\d,]{2,})(?:\.\d+)?\s*-?\s*" + _UNIT + r"\b")
_BARE = re.compile(r"\bxc8\W{1,3}\d{3,}", re.IGNORECASE)
_RATIO = re.compile(r"(?<![\w.])\d+\.\d+x\b")
_USED_OF_TOTAL = re.compile(r"(?<![\w.])\d{3,}\s*/\s*\d{3,}\b")
WINDOW_AFTER = 1


def _is_capacity(digits):
    n = int(digits.replace(",", ""))
    return n > 0 and n & (n - 1) == 0


def _has_figure(text):
    text = _NOISE.sub(" ", text)
    for m in _SIZE.finditer(text):
        if not _is_capacity(m.group(1)):
            return True
    return bool(
        _BARE.search(text) or _RATIO.search(text) or _USED_OF_TOTAL.search(text)
    )


def find_xc8_figures(text):
    """Return (line number, line) for every XC8 mention near a figure."""
    lines = text.split("\n")
    hits = []
    for i, line in enumerate(lines):
        if not _XC8.search(line):
            continue
        if line.lstrip().startswith("|"):
            window = [line]
        else:
            window = lines[i : i + WINDOW_AFTER + 1]
        if _has_figure(" ".join(window)):
            hits.append((i + 1, line.strip()))
    return hits


class TestDetector(unittest.TestCase):
    def test_flags_size_figures(self):
        for text in (
            "XC8 uses 777 words on the 628A",
            "the blink links 7,771 words under XC8",
            "XC8 flash 7771/16384",
            "1.31x XC8",
            "XC8 emits 7777 bytes",
            "| XC8 RAM | 777 B |",
            "XC8 takes 7777 cycles",
            "overlapped all three (XC8 7771/7772/7773, found 2026-08-11)",
            "| XC8 | PASS | 7771/16384 | 777/2048 |",
        ):
            with self.subTest(text=text):
                self.assertTrue(find_xc8_figures(text), text)

    def test_window_covers_wrapped_prose(self):
        self.assertTrue(find_xc8_figures("the XC8 build\nneeds 7774 words\nof flash"))

    def test_allows_versions_codes_capacities_and_parts(self):
        for text in (
            "XC8 v4.00 builds this",
            "/opt/microchip/xc8/v3.10/bin",
            "XC8 error 1347 on the 16F627",
            "XC8 warning (1482) in the link",
            "does not fit the 512-word flash (XC8 error 1360)",
            "XC8 bytes: the 2048 byte RAM is full",
            "XC8 builds the PIC16F877A and PIC18F4550",
            "xc8-cc -mdfp=... -mcpu=18f4550",
            "XC8 2026-09-17 measured under #450",
            "| epic-cc | PASS | 7778/16384 |\n| XC8 | PASS | - |",
        ):
            with self.subTest(text=text):
                self.assertEqual(find_xc8_figures(text), [])

    def test_no_xc8_no_hit(self):
        self.assertEqual(find_xc8_figures("blink uses 777 words"), [])


def _tracked_files():
    out = subprocess.run(
        ["git", "ls-files"], cwd=REPO_ROOT, capture_output=True, text=True, check=True
    ).stdout
    for rel in out.splitlines():
        if rel.endswith(SCANNED) and not any(s in rel for s in SKIPPED_PARTS):
            yield rel


class TestTrackedFiles(unittest.TestCase):
    def test_no_xc8_figures_in_tracked_text(self):
        found = []
        try:
            files = list(_tracked_files())
        except (subprocess.CalledProcessError, OSError):
            self.skipTest("no git checkout visible (container worktree mount)")
        for rel in files:
            path = REPO_ROOT / rel
            if not path.is_file():
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            found += [f"{rel}:{n}" for n, _ in find_xc8_figures(text)]
        self.assertEqual(
            found,
            [],
            "XC8 size or speed figures belong in the private epic-benchmarks "
            "repository, not in public text (epic-hal#450)",
        )


if __name__ == "__main__":
    unittest.main()
