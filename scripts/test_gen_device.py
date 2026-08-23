"""Generator coverage using a hand-authored ATDF. No vendor file is committed."""

import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
GEN = ROOT / "scripts" / "gen-device.py"
FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic.atdf"


class GenDeviceTest(unittest.TestCase):
    def generate(self):
        with tempfile.TemporaryDirectory() as d:
            out = pathlib.Path(d) / "synthetic.toml"
            r = subprocess.run(
                [sys.executable, str(GEN), "synthetic",
                 "--atdf", str(FIXTURE), "--out", str(out)],
                capture_output=True, text=True,
            )
            self.assertEqual(r.returncode, 0, r.stderr)
            return out.read_text()

    def test_emits_a_provenance_stanza(self):
        text = self.generate()
        self.assertIn("[provenance]", text)
        self.assertIn('tier = "atdf"', text)
        self.assertIn("sha256 = ", text)

    def test_is_deterministic(self):
        self.assertEqual(self.generate(), self.generate())

    def test_emits_the_required_scalars(self):
        text = self.generate()
        for key in ("name", "core", "flash_words", "ram_banks"):
            self.assertIn(key, text)


if __name__ == "__main__":
    unittest.main()
