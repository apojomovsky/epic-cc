"""Generator coverage using a hand-authored ATDF. No vendor file is committed.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
GEN = ROOT / "scripts" / "gen-device.py"
FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic.atdf"
PIC18_FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic_pic18.atdf"
SWEEP_PACK = ROOT / "scripts" / "fixtures" / "sweep-pack"

# `gen-device.py`'s CLI only accepts one `--atdf` path, so it cannot express
# "a real ini and a real .cfgdata, no EDC" in one invocation (the ini/cfgdata
# pair is otherwise only discoverable under a local XC8 install, unavailable
# here). Importing `generate_toml` directly is the only way to exercise that
# specific input combination: filename has a hyphen, so `import` cannot name it.
_spec = importlib.util.spec_from_file_location("gen_device", GEN)
gen_device = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gen_device)


def run_generator(source, name="synthetic", pack=None):
    with tempfile.TemporaryDirectory() as d:
        out = pathlib.Path(d) / "synthetic.toml"
        cmd = [sys.executable, str(GEN), name, "--atdf", str(source), "--out", str(out)]
        if pack:
            cmd += ["--pack", pack]
        r = subprocess.run(cmd, capture_output=True, text=True)
        return r, out.read_text() if out.exists() else ""


class GenDeviceTest(unittest.TestCase):
    def generate(self):
        r, text = run_generator(FIXTURE, pack="Microchip.PIC16Fxxx_DFP")
        self.assertEqual(r.returncode, 0, r.stderr)
        return text

    def test_emits_a_provenance_stanza(self):
        text = self.generate()
        self.assertIn("[provenance]", text)
        self.assertIn('tier = "atdf"', text)
        self.assertIn('pack = "Microchip.PIC16Fxxx_DFP"', text)
        self.assertIn("sha256 = ", text)

    def test_refuses_when_pack_name_cannot_be_resolved(self):
        # The fixture path has no *_DFP ancestor directory, so without
        # --pack the pack name is unknowable. Writing pack = "unknown"
        # would fabricate provenance (ADR-021): refuse instead.
        r, text = run_generator(FIXTURE)
        self.assertNotEqual(r.returncode, 0, "an unresolvable pack name must not generate")
        self.assertIn("pack", r.stderr)
        self.assertEqual(text, "", "nothing may be written when the pack name is unknown")

    def test_pack_name_derived_from_dfp_ancestor_directory(self):
        # A file still inside its pack directory needs no --pack: the
        # *_DFP ancestor names the pack, matching how a .atpack unzips.
        with tempfile.TemporaryDirectory() as d:
            pack_dir = pathlib.Path(d) / "Microchip.PIC16Fxxx_DFP.1.7.162" / "edc"
            pack_dir.mkdir(parents=True)
            src = pack_dir / "synthetic.atdf"
            src.write_text(FIXTURE.read_text())
            r, text = run_generator(src)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('pack = "Microchip.PIC16Fxxx_DFP.1.7.162"', text)

    def test_explicit_pack_wins_over_ancestor_directory(self):
        # --pack is the caller's deliberate choice; it must not be
        # overridden by whatever directory the file happens to sit in.
        with tempfile.TemporaryDirectory() as d:
            pack_dir = pathlib.Path(d) / "Microchip.PIC16Fxxx_DFP" / "edc"
            pack_dir.mkdir(parents=True)
            src = pack_dir / "synthetic.atdf"
            src.write_text(FIXTURE.read_text())
            r, text = run_generator(src, pack="Microchip.PIC18Fxxxx_DFP")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('pack = "Microchip.PIC18Fxxxx_DFP"', text)

    def test_check_does_not_require_a_pack_name(self):
        # --check strips the provenance stanza before comparing, so the
        # pack name is irrelevant to it. A file outside its pack
        # directory must still be checkable without --pack.
        with tempfile.TemporaryDirectory() as d:
            out = pathlib.Path(d) / "synthetic.toml"
            r = subprocess.run(
                [sys.executable, str(GEN), "synthetic", "--atdf", str(FIXTURE),
                 "--pack", "Microchip.PIC16Fxxx_DFP", "--out", str(out)],
                capture_output=True, text=True,
            )
            self.assertEqual(r.returncode, 0, r.stderr)
            r = subprocess.run(
                [sys.executable, str(GEN), "synthetic", "--atdf", str(FIXTURE),
                 "--out", str(out), "--check"],
                capture_output=True, text=True,
            )
        self.assertEqual(r.returncode, 0, r.stderr)

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
            r, text = run_generator(src, pack="Microchip.PIC16Fxxx_DFP")
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
        r, text = run_generator(PIC18_FIXTURE, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP")
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
        # gpr0 (0x60-0x7F) and gpr1 (0x80-0x9F) are back-to-back, and the
        # schema's `ram_banks` also folds in whatever the access bank
        # (0x00-0x5F) leaves over after `fixed_retval` (0x00-0x0F) reserves
        # its slice: matching the hand-transcribed p18f4550.toml, whose
        # `ram_banks` starts right after `fixed_retval`, not at the banked
        # region's own start address.
        text = self.generate()
        self.assertIn("ram_banks = [[0x0010, 0x009F]]", text)

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
            r, text = run_generator(src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP")
        self.assertNotEqual(r.returncode, 0, "an impl mismatch must not generate")
        self.assertIn("does not match", r.stderr)
        self.assertEqual(text, "")

    def test_noncontiguous_field_mask_is_a_hard_failure(self):
        # ALPHA's mask (0x1) is a contiguous 1-bit run, the only shape the
        # width/cursor reconstruction handles. A mask like 0x5 (bits 0 and 2)
        # would silently be re-encoded as a wrong, contiguous 0x3 without
        # this check.
        tampered = PIC18_FIXTURE.read_text().replace(
            'edc:name="ALPHA" edc:mask="0x1"', 'edc:name="ALPHA" edc:mask="0x5"'
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "bad_mask.atdf"
            src.write_text(tampered)
            r, text = run_generator(src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP")
        self.assertNotEqual(r.returncode, 0, "a non-contiguous field mask must not generate")
        self.assertIn("not a contiguous run", r.stderr)
        self.assertEqual(text, "")

    def test_unhandled_relational_when_is_a_hard_failure(self):
        # A `when` using a relational form (>=, <, !=) instead of `==` must
        # not be silently skipped: dropping every semantic for GAMMA would
        # otherwise drop the whole field from the output with exit 0.
        tampered = PIC18_FIXTURE.read_text().replace(
            'edc:cname="THREE" edc:when="(field &amp; 0x3) == 0x3"',
            'edc:cname="THREE" edc:when="(field &amp; 0x3) &gt;= 0x3"',
        ).replace(
            'edc:cname="ONE" edc:when="(field &amp; 0x3) == 0x1"',
            'edc:cname="ONE" edc:when="(field &amp; 0x3) &gt;= 0x1"',
        ).replace(
            'edc:cname="ZERO" edc:when="(field &amp; 0x3) == 0x0"',
            'edc:cname="ZERO" edc:when="(field &amp; 0x3) &gt;= 0x0"',
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "relational_when.atdf"
            src.write_text(tampered)
            r, text = run_generator(src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP")
        self.assertNotEqual(r.returncode, 0, "an unhandled `when` form must not generate")
        self.assertIn("unhandled", r.stderr)
        self.assertEqual(text, "")

    def test_split_access_bank_sectors_merge(self):
        # Two adjacent TraditionalModeOnly sectors describing one physical
        # access bank must merge, not silently lose the second half by only
        # keeping the lowest-address sector.
        split = PIC18_FIXTURE.read_text().replace(
            '<edc:GPRDataSector edc:regionid="accessram" edc:beginaddr="0x0" edc:endaddr="0x60" edc:bank="0"/>',
            '<edc:GPRDataSector edc:regionid="accessram_lo" edc:beginaddr="0x0" edc:endaddr="0x30" edc:bank="0"/>\n'
            '      <edc:GPRDataSector edc:regionid="accessram_hi" edc:beginaddr="0x30" edc:endaddr="0x60" edc:bank="0"/>',
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "split_access.atdf"
            src.write_text(split)
            r, text = run_generator(src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("access_bank = [0x0000, 0x005F]", text)


class GenDevicePic18CfgdataTest(unittest.TestCase):
    """`.cfgdata` is XC8's own repackaging of the same DFP data gen-device.py
    reads from EDC directly; its PIC18 CWORD addresses are byte-native the
    same way, and the word-shift pipeline that consumes it once doubled
    them exactly like the address-range-only EDC fallback did (epic-cc#230).
    Only reachable when no --atdf is given and a local XC8 install supplies
    ini+cfgdata but no matching EDC .PIC, so it needs generate_toml called
    directly (see the importlib note above)."""

    def write_ini(self, d):
        ini = pathlib.Path(d) / "p18cfgtest.ini"
        ini.write_text(
            "[18CFGTEST]\n"
            "ARCH=PIC16\n"
            "ROMSIZE=800\n"
            "RAMBANK=60-FF\n"
            "STACKDEPTH=0x1F\n"
        )
        return ini

    def write_cfgdata(self, d):
        cfg = pathlib.Path(d) / "p18cfgtest.cfgdata"
        cfg.write_text(
            "CWORD:300000:1:0:CONFIG1L\n"
            "CSETTING:1:ALPHA\n"
            "CVALUE:1:ON\n"
            "CVALUE:0:OFF\n"
            "CWORD:300001:1:0:CONFIG1H\n"
            "CSETTING:1:BETA\n"
            "CVALUE:1:ON\n"
            "CVALUE:0:OFF\n"
        )
        return cfg

    def test_cfgdata_sourced_pic18_config_is_not_doubled(self):
        with tempfile.TemporaryDirectory() as d:
            ini = self.write_ini(d)
            cfg = self.write_cfgdata(d)
            text = gen_device.generate_toml("p18cfgtest", ini, cfg, None)
        self.assertIn('core = "pic18"', text)
        self.assertIn("base_byte_addr = 0x300000", text)
        self.assertIn("num_bytes = 2", text)
        self.assertIn('name = "alpha"\nbyte_offset = 0', text)
        # beta is CONFIG1H, the byte right after CONFIG1L: offset 1, not the
        # doubled-address bug's offset 2.
        self.assertIn('name = "beta"\nbyte_offset = 1', text)


class GenDeviceSweepTest(unittest.TestCase):
    """`--sweep` breadth proofing (docs/38 D-2): generate for every part in
    an unpacked DFP directory, triage failures instead of stopping at the
    first one. The fixture pack mirrors a real .atpack layout: a
    `*_DFP`-suffixed directory holding `edc/*.PIC` files, so the pack name
    resolves from the ancestor and the sweep needs no `--pack`."""

    def run_sweep(self, *extra):
        cmd = [sys.executable, str(GEN), "--sweep", str(SWEEP_PACK), *extra]
        return subprocess.run(cmd, capture_output=True, text=True)

    def test_sweep_reports_every_part_and_exits_1_on_failures(self):
        with tempfile.TemporaryDirectory() as d:
            edc = pathlib.Path(d) / "Microchip.PIC16Fxxx_DFP" / "edc"
            edc.mkdir(parents=True)
            (edc / "PIC14SYN01.PIC").write_text(FIXTURE.read_text())
            (edc / "PIC18SYN01.PIC").write_text(PIC18_FIXTURE.read_text())
            (edc / "PIC14SYN02.PIC").write_text(
                "\n".join(l for l in FIXTURE.read_text().splitlines() if "GPRDataSector" not in l)
            )
            (edc / "BROKEN.PIC").write_text('<edc:PIC xmlns:edc="http://crownking/edc">')
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(pathlib.Path(d))],
                capture_output=True, text=True,
            )
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("ok  p14syn01", r.stdout)
        self.assertIn("ok  p18syn01", r.stdout)
        self.assertIn("missing-facts  p14syn02: ram_banks", r.stdout)
        self.assertIn("error", r.stdout)
        self.assertIn("pbroken", r.stdout)
        self.assertIn("2/4 parts generated, 2 failed", r.stdout)

    def test_sweep_exits_0_when_every_part_generates(self):
        r = self.run_sweep()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("ok  p14syn01", r.stdout)
        self.assertIn("ok  p18syn01", r.stdout)
        self.assertIn("2/2 parts generated, 0 failed", r.stdout)

    def test_sweep_derives_pack_name_from_dfp_ancestor(self):
        with tempfile.TemporaryDirectory() as d:
            r = self.run_sweep("--out-dir", d)
            self.assertEqual(r.returncode, 0, r.stderr)
            toml = (pathlib.Path(d) / "p14syn01.toml").read_text()
        self.assertIn('pack = "Microchip.PIC16Fxxx_DFP"', toml)

    def test_sweep_out_dir_holds_only_successful_tomls(self):
        with tempfile.TemporaryDirectory() as d:
            r = self.run_sweep("--out-dir", d)
            self.assertEqual(r.returncode, 0, r.stderr)
            written = sorted(p.name for p in pathlib.Path(d).iterdir())
        self.assertEqual(written, ["p14syn01.toml", "p18syn01.toml"])

    def test_sweep_rejects_a_non_directory(self):
        r = subprocess.run(
            [sys.executable, str(GEN), "--sweep", str(GEN)],
            capture_output=True, text=True,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("is not a directory", r.stderr)

    def test_sweep_rejects_out_dir_pointing_at_a_file(self):
        with tempfile.TemporaryDirectory() as d:
            f = pathlib.Path(d) / "out"
            f.write_text("x")
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(SWEEP_PACK), "--out-dir", str(f)],
                capture_output=True, text=True,
            )
        self.assertEqual(r.returncode, 2)
        self.assertIn("is not a directory", r.stderr)

    def test_sweep_exits_1_when_no_parts_found(self):
        with tempfile.TemporaryDirectory() as d:
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(d)],
                capture_output=True, text=True,
            )
        self.assertEqual(r.returncode, 1)
        self.assertIn("no *.PIC files found", r.stderr)

    def test_sweep_rejects_single_part_flags(self):
        r = subprocess.run(
            [sys.executable, str(GEN), "--sweep", str(SWEEP_PACK), "--check"],
            capture_output=True, text=True,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("--check cannot be combined with --sweep", r.stderr)

    def test_sweep_uses_pack_local_ini_and_cfgdata(self):
        # A real .atpack carries its own xc8/pic/dat (ADR-020); the sweep
        # must read ini/cfgdata from the swept pack, not depend on a global
        # XC8 install. The ini's ROMSIZE (512) overrides the EDC's code_end
        # (1024), proving the pack-local ini was the source.
        with tempfile.TemporaryDirectory() as d:
            pack = pathlib.Path(d) / "Microchip.PIC16Fxxx_DFP"
            edc = pack / "edc"
            edc.mkdir(parents=True)
            (edc / "PIC14SYN01.PIC").write_text(FIXTURE.read_text())
            ini = pack / "ini"
            ini.mkdir()
            (ini / "14syn01.ini").write_text(
                "[14SYN01]\n"
                "ARCH=16xxxx\n"
                "ROMSIZE=200\n"
                "RAMBANK=20-6F\n"
                "STACKDEPTH=0x8\n"
            )
            out = pathlib.Path(d) / "out"
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(pack), "--out-dir", str(out)],
                capture_output=True, text=True,
            )
            toml = (out / "p14syn01.toml").read_text()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("flash_words = 512", toml)


if __name__ == "__main__":
    unittest.main()
