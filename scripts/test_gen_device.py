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
PIC18_FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic_pic18.atdf"


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


class GenDevicePic18Test(unittest.TestCase):
    """PIC18's EDC shape (byte-addressed DCRDef config bytes, an access
    bank split from banked GPR, an Extended Instruction Set mirror this
    compiler does not target) is structurally different from PIC14's, and
    was never exercised by a real second device before epic-cc#230 found
    it silently mis-generating both the config region and the RAM map."""

    def generate(self):
        r, text = run_generator(PIC18_FIXTURE, name="p18syn01")
        self.assertEqual(r.returncode, 0, r.stderr)
        return text

    def test_transcribes_the_fixture_not_a_pic4550_default(self):
        # None of these numbers are shared with p18f4550: base_byte_addr
        # 0x600000 or num_bytes 28 here would mean the old word-doubling
        # bug (or a regression of its fix) is back.
        text = self.generate()
        self.assertIn('core = "pic18"', text)
        self.assertIn("flash_words = 1024", text)
        self.assertIn("stack_depth = 16", text)
        self.assertIn("interrupt_vectors = [0x0008, 0x0018]", text)

    def test_config_byte_range_is_not_doubled(self):
        # Real bug: PIC18's ConfigFuseSector is already byte-addressed;
        # the generator once multiplied it by two as if it were PIC14's
        # word-addressed convention (epic-cc#230).
        text = self.generate()
        self.assertIn("base_byte_addr = 0x300000", text)
        self.assertIn("num_bytes = 3", text)
        self.assertIn("erased_baseline = [0xFF, 0xFF, 0xFF]", text)

    def test_config_gap_byte_has_no_fields(self):
        # CONFIGX (0x300000) and CONFIGY (0x300002) are non-adjacent, like
        # PIC18F2550's real missing CONFIG3L. The gap byte (offset 1) must
        # still be counted in num_bytes but own no [[config.fields]].
        text = self.generate()
        self.assertNotIn("byte_offset = 1\n", text)

    def test_adjust_point_and_hidden_field_are_handled(self):
        # ALPHA is bit 0; AdjustPoint skips the reserved bit 1; the hidden
        # RES field at bit 2 must be excluded from the output even though
        # its width still had to be walked correctly to get here.
        text = self.generate()
        self.assertIn('name = "alpha"', text)
        self.assertIn("mask = 0x01", text)
        self.assertIn("shift = 0", text)
        self.assertNotIn('name = "res"', text)

    def test_multi_bit_field_values_are_field_local(self):
        # GAMMA is a 2-bit field; DCRFieldSemantic's `when` values are
        # already local to the field (0-3), not word-shifted.
        text = self.generate()
        self.assertIn('name = "gamma"', text)
        self.assertIn('{ name = "zero", bits = 0 }', text)
        self.assertIn('{ name = "one", bits = 1 }', text)
        self.assertIn('{ name = "three", bits = 3 }', text)

    def test_access_bank_is_the_traditional_mode_view_not_extended(self):
        # accessram (TraditionalModeOnly, 0x00-0x5F) and gpre
        # (ExtendedModeOnly, deliberately given 0x00-0x8F in the fixture)
        # describe the same physical bytes under the two PIC18 instruction
        # set modes. This compiler never sets `xinst`, so only the
        # Traditional view may surface as `access_bank`.
        text = self.generate()
        self.assertIn("access_bank = [0x0000, 0x005F]", text)
        self.assertIn("fixed_retval = [0x0000, 0x000F]", text)

    def test_contiguous_gpr_banks_merge(self):
        # gpr0 (0x60-0x7F) and gpr1 (0x80-0x9F) are back-to-back; the
        # schema wants one merged range, matching the hand-transcribed
        # p18f4550.toml's single-entry `ram_banks`.
        text = self.generate()
        self.assertIn("ram_banks = [[0x0060, 0x009F]]", text)

    def test_bit_layout_mismatch_against_impl_is_a_hard_failure(self):
        # If CONFIGX's declared `impl` no longer matches the union of its
        # own fields' bit positions, that means the AdjustPoint/DCRFieldDef
        # walk computed the wrong layout for this byte. Guessing anyway
        # would silently mis-place every field after the mismatch.
        tampered = PIC18_FIXTURE.read_text().replace(
            'edc:impl="0x05"', 'edc:impl="0x07"'
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "bad_impl.atdf"
            src.write_text(tampered)
            r, text = run_generator(src, name="p18syn01")
        self.assertNotEqual(r.returncode, 0, "an impl mismatch must not generate")
        self.assertIn("does not match", r.stderr)
        self.assertEqual(text, "")


if __name__ == "__main__":
    unittest.main()
