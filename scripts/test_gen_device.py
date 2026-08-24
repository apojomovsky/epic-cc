"""Generator coverage using a hand-authored ATDF. No vendor file is committed.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
GEN = ROOT / "scripts" / "gen-device.py"
FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic.atdf"


def run_generator(source, name="synthetic"):
    with tempfile.TemporaryDirectory() as d:
        out = pathlib.Path(d) / "synthetic.toml"
        r = subprocess.run(
            [sys.executable, str(GEN), name, "--atdf", str(source), "--out", str(out)],
            capture_output=True, text=True,
        )
        return r, out.read_text() if out.exists() else ""


class GenDeviceTest(unittest.TestCase):
    def generate(self):
        r, text = run_generator(FIXTURE)
        self.assertEqual(r.returncode, 0, r.stderr)
        return text

    def test_emits_a_provenance_stanza(self):
        text = self.generate()
        self.assertIn("[provenance]", text)
        self.assertIn('tier = "atdf"', text)
        self.assertIn("sha256 = ", text)

    def test_is_deterministic(self):
        self.assertEqual(self.generate(), self.generate())

    def test_transcribes_the_fixture_not_a_builtin_default(self):
        # The fixture's own numbers, none of them shared with a real part:
        # 8192 words or four 0x20-0x6F style banks here would mean the
        # generator fell back to p16f877a's map for an imaginary device.
        text = self.generate()
        self.assertIn("flash_words = 1024", text)
        self.assertIn("ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]", text)
        self.assertIn("common_ram = [0x0070, 0x007F]", text)
        self.assertIn("stack_depth = 8", text)
        self.assertIn('core = "pic14"', text)

    def test_fails_loudly_when_the_source_omits_a_field(self):
        stripped = "\n".join(
            l for l in FIXTURE.read_text().splitlines() if "GPRDataSector" not in l
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "no_ram.atdf"
            src.write_text(stripped)
            r, text = run_generator(src)
        self.assertNotEqual(r.returncode, 0, "a source with no RAM map must not generate")
        self.assertIn("ram_banks", r.stderr)
        self.assertEqual(text, "", "nothing may be written when a field is missing")


if __name__ == "__main__":
    unittest.main()
