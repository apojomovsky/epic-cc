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
        self.assertNotEqual(
            r.returncode, 0, "an unresolvable pack name must not generate"
        )
        self.assertIn("pack", r.stderr)
        self.assertEqual(
            text, "", "nothing may be written when the pack name is unknown"
        )

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
                [
                    sys.executable,
                    str(GEN),
                    "synthetic",
                    "--atdf",
                    str(FIXTURE),
                    "--pack",
                    "Microchip.PIC16Fxxx_DFP",
                    "--out",
                    str(out),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(r.returncode, 0, r.stderr)
            r = subprocess.run(
                [
                    sys.executable,
                    str(GEN),
                    "synthetic",
                    "--atdf",
                    str(FIXTURE),
                    "--out",
                    str(out),
                    "--check",
                ],
                capture_output=True,
                text=True,
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

    def test_full_bank_shadow_is_a_duplicate_not_common_ram(self):
        # Confirmed on PIC16F74/PIC16F84: a bank whose *entire* GPR is one
        # shadowidref (no real GPR of its own in that bank) is bit-for-bit
        # the same silicon as the bank it shadows, not a bank-independent
        # common corner (contrast the fixture's own bank1, which mixes a
        # shadow with real GPR of its own and correctly stays common_ram).
        # A merged ram_banks or a fabricated common_ram here would mean the
        # generator mistook a full-bank duplicate for extra or shared RAM.
        xml = """<edc:PIC xmlns:edc="http://crownking/edc" edc:name="PIC14SYN02" edc:arch="16xxxx">
  <edc:ArchDef edc:name="16xxxx">
    <edc:MemTraits edc:hwstackdepth="0x8"/>
  </edc:ArchDef>
  <edc:ProgramSpace>
    <edc:CodeSector edc:beginaddr="0x0" edc:endaddr="0x400"/>
    <edc:ConfigFuseSector edc:beginaddr="0x2007" edc:endaddr="0x2008"/>
  </edc:ProgramSpace>
  <edc:DataSpace edc:endaddr="0x200">
    <edc:RegardlessOfMode>
      <edc:SFRDataSector edc:beginaddr="0x0" edc:endaddr="0x20" edc:bank="0"/>
      <edc:GPRDataSector edc:regionid="gpr0" edc:beginaddr="0x20" edc:endaddr="0x70" edc:bank="0"/>
      <edc:SFRDataSector edc:beginaddr="0x80" edc:endaddr="0xA0" edc:bank="1"/>
      <edc:GPRDataSector edc:regionid="gpr1" edc:beginaddr="0xA0" edc:endaddr="0xF0" edc:bank="1"/>
      <edc:SFRDataSector edc:beginaddr="0x100" edc:endaddr="0x120" edc:bank="2"/>
      <edc:GPRDataSector edc:regionid="gpr2" edc:shadowidref="gpr0" edc:beginaddr="0x120" edc:endaddr="0x170" edc:bank="2"/>
    </edc:RegardlessOfMode>
  </edc:DataSpace>
</edc:PIC>"""
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "full_alias.atdf"
            src.write_text(xml)
            r, text = run_generator(
                src, name="p14syn02", pack="Microchip.PIC16Fxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]", text)
        self.assertNotIn("common_ram", text)

    def test_fails_loudly_when_the_source_omits_a_field(self):
        stripped = "\n".join(
            line
            for line in FIXTURE.read_text().splitlines()
            if "GPRDataSector" not in line
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "no_ram.atdf"
            src.write_text(stripped)
            r, text = run_generator(src, pack="Microchip.PIC16Fxxx_DFP")
        self.assertNotEqual(
            r.returncode, 0, "a source with no RAM map must not generate"
        )
        self.assertIn("ram_banks", r.stderr)
        self.assertEqual(text, "", "nothing may be written when a field is missing")


class GenDevicePic18Test(unittest.TestCase):
    """PIC18's EDC shape (byte-addressed DCRDef config bytes, an access
    bank split from banked GPR, an Extended Instruction Set mirror this
    compiler does not target) is structurally different from PIC14's, and
    was never exercised by a real second device before epic-cc#230 found
    it silently mis-generating both the config region and the RAM map."""

    def generate(self):
        r, text = run_generator(
            PIC18_FIXTURE, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
        )
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

    def test_bare_accessram_under_regardless_of_mode_is_still_the_access_bank(self):
        # Confirmed on PIC18F252/258/452: silicon that predates the Extended
        # Instruction Set entirely states `accessram` directly under
        # RegardlessOfMode, with empty TraditionalModeOnly/ExtendedModeOnly
        # stubs, instead of wrapping it like every other shipped PIC18
        # device. Without recognizing the bare form, `access_bank` goes
        # missing and `accessram` is misread as an ordinary banked GPR.
        xml = """<edc:PIC xmlns:edc="http://crownking/edc" edc:name="PIC18SYN02" edc:arch="18xxxx">
  <edc:ArchDef edc:name="18xxxx">
    <edc:MemTraits edc:hwstackdepth="0x10"/>
  </edc:ArchDef>
  <edc:ProgramSpace>
    <edc:CodeSector edc:beginaddr="0x0" edc:endaddr="0x800"/>
    <edc:ConfigFuseSector edc:beginaddr="0x300000" edc:endaddr="0x300001"/>
  </edc:ProgramSpace>
  <edc:DataSpace edc:endaddr="0x100">
    <edc:RegardlessOfMode>
      <edc:GPRDataSector edc:regionid="accessram" edc:beginaddr="0x0" edc:endaddr="0x60" edc:bank="0x0"/>
      <edc:GPRDataSector edc:regionid="gpr0" edc:beginaddr="0x60" edc:endaddr="0x100" edc:bank="0x0"/>
    </edc:RegardlessOfMode>
    <edc:TraditionalModeOnly/>
    <edc:ExtendedModeOnly/>
  </edc:DataSpace>
</edc:PIC>"""
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "bare_accessram.atdf"
            src.write_text(xml)
            r, text = run_generator(
                src, name="p18syn02", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("access_bank = [0x0000, 0x005F]", text)

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
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertNotEqual(r.returncode, 0, "an impl mismatch must not generate")
        self.assertIn("does not match", r.stderr)
        self.assertEqual(text, "")

    def test_scattered_field_mask_generates(self):
        # A mask like 0x5 (bits 0 and 2) is scattered within its span, the
        # PIC16F628A FOSC shape (mask 0x13 over a 5-bit window). The cursor
        # advances by span and the absolute bits are mask << cursor, so this
        # generates with the scattered mask intact instead of being
        # re-encoded as a wrong contiguous run.
        tampered = PIC18_FIXTURE.read_text().replace(
            'edc:name="ALPHA" edc:mask="0x1"', 'edc:name="ALPHA" edc:mask="0x5"'
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "scattered_mask.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("mask = 0x05", text)

    def test_mask_outside_nzwidth_span_is_a_hard_failure(self):
        # A mask that does not fit its nzwidth span is inconsistent source
        # data, not a scattered field: refusing beats placing wrong bits.
        tampered = PIC18_FIXTURE.read_text().replace(
            'edc:name="ALPHA" edc:mask="0x1"',
            'edc:name="ALPHA" edc:mask="0x5" edc:nzwidth="0x1"',
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "bad_span.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertNotEqual(
            r.returncode, 0, "a mask outside its span must not generate"
        )
        self.assertIn("does not fit", r.stderr)
        self.assertEqual(text, "")

    def test_unhandled_relational_when_is_a_hard_failure(self):
        # A `when` using a relational form (>=, <, !=) instead of `==` must
        # not be silently skipped: dropping every semantic for GAMMA would
        # otherwise drop the whole field from the output with exit 0.
        tampered = (
            PIC18_FIXTURE.read_text()
            .replace(
                'edc:cname="THREE" edc:when="(field &amp; 0x3) == 0x3"',
                'edc:cname="THREE" edc:when="(field &amp; 0x3) &gt;= 0x3"',
            )
            .replace(
                'edc:cname="ONE" edc:when="(field &amp; 0x3) == 0x1"',
                'edc:cname="ONE" edc:when="(field &amp; 0x3) &gt;= 0x1"',
            )
            .replace(
                'edc:cname="ZERO" edc:when="(field &amp; 0x3) == 0x0"',
                'edc:cname="ZERO" edc:when="(field &amp; 0x3) &gt;= 0x0"',
            )
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "relational_when.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertNotEqual(
            r.returncode, 0, "an unhandled `when` form must not generate"
        )
        self.assertIn("unhandled", r.stderr)
        self.assertEqual(text, "")

    def test_individually_hidden_semantic_without_cname_is_skipped(self):
        # A semantic can be hidden on its own (an overlapping "don't care"
        # bit pattern) even when its field is not, and carries no cname
        # since it names no real value (Microchip.PIC18Fxxxx_DFP 1.8.178's
        # CONFIG1H OSC field does exactly this for its 11XX/101X entries).
        # Collecting it as a real value crashed downstream aliasing on the
        # missing name; it must be dropped like a hidden field's values are.
        tampered = PIC18_FIXTURE.read_text().replace(
            '<edc:DCRFieldSemantic edc:cname="OFF" edc:when="(field &amp; 0x1) == 0x0"/>\n'
            "            </edc:DCRFieldDef>\n"
            "            <edc:AdjustPoint",
            '<edc:DCRFieldSemantic edc:cname="OFF" edc:when="(field &amp; 0x1) == 0x0"/>\n'
            '              <edc:DCRFieldSemantic edc:when="(field &amp; 0x1) == 0x1" '
            'edc:islanghidden="true"/>\n'
            "            </edc:DCRFieldDef>\n"
            "            <edc:AdjustPoint",
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "hidden_semantic.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('name = "alpha"', text)
        self.assertIn('{ name = "on", bits = 1 }', text)
        self.assertIn('{ name = "off", bits = 0 }', text)

    def test_hidden_field_beyond_impl_is_padding_not_a_mismatch(self):
        # A trailing hidden RESERVED field can describe bits past the
        # byte's own impl mask ("maintain as 1s" documentation, confirmed
        # against Microchip.PIC18Fxxxx_DFP 1.8.178's CONFIG5L on parts like
        # p18lf4420). Those bits are not real config bits, so they must not
        # be forced to match impl the way a real field's bits are.
        tampered = PIC18_FIXTURE.read_text().replace(
            '<edc:DCRFieldDef edc:name="GAMMA" edc:mask="0x3">\n'
            '              <edc:DCRFieldSemantic edc:cname="THREE" edc:when="(field &amp; 0x3) == 0x3"/>\n'
            '              <edc:DCRFieldSemantic edc:cname="ONE" edc:when="(field &amp; 0x3) == 0x1"/>\n'
            '              <edc:DCRFieldSemantic edc:cname="ZERO" edc:when="(field &amp; 0x3) == 0x0"/>\n'
            "            </edc:DCRFieldDef>",
            '<edc:DCRFieldDef edc:name="GAMMA" edc:mask="0x3">\n'
            '              <edc:DCRFieldSemantic edc:cname="THREE" edc:when="(field &amp; 0x3) == 0x3"/>\n'
            '              <edc:DCRFieldSemantic edc:cname="ONE" edc:when="(field &amp; 0x3) == 0x1"/>\n'
            '              <edc:DCRFieldSemantic edc:cname="ZERO" edc:when="(field &amp; 0x3) == 0x0"/>\n'
            "            </edc:DCRFieldDef>\n"
            '            <edc:DCRFieldDef edc:name="RESERVED" edc:mask="0x3" '
            'edc:ishidden="true" edc:islanghidden="true">\n'
            '              <edc:DCRFieldSemantic edc:when="(field &amp; 0x3) == 0x3"/>\n'
            "            </edc:DCRFieldDef>",
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "reserved_beyond_impl.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('name = "gamma"', text)
        self.assertNotIn('name = "reserved"', text)

    def test_hidden_field_straddling_impl_only_counts_the_overlap(self):
        # RES (hidden, bit 2, inside impl 0x05) widened to two bits (2-3):
        # bit 2 is real, implemented and still counts; bit 3 is padding
        # past impl and must not be. Counting the whole widened field
        # would make covered 0x0d against impl 0x05, a false mismatch for
        # a field that is genuinely part real bit, part padding.
        tampered = PIC18_FIXTURE.read_text().replace(
            '<edc:DCRFieldDef edc:name="RES" edc:mask="0x1" '
            'edc:ishidden="true" edc:islanghidden="true">',
            '<edc:DCRFieldDef edc:name="RES" edc:mask="0x3" '
            'edc:ishidden="true" edc:islanghidden="true">',
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "straddling_res.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('name = "alpha"', text)
        self.assertNotIn('name = "res"', text)

    def test_field_with_only_hidden_semantics_is_a_hard_failure(self):
        # ALPHA is a real, visible field; if every one of its semantics
        # turns out to be individually hidden, the generator has no legal
        # value left to name it with. Dropping it from the output with
        # exit 0 would silently omit a real config field, the same
        # failure mode the unhandled-`when` check above already refuses.
        tampered = PIC18_FIXTURE.read_text().replace(
            '<edc:DCRFieldSemantic edc:cname="ON" edc:when="(field &amp; 0x1) == 0x1"/>\n'
            '              <edc:DCRFieldSemantic edc:cname="OFF" edc:when="(field &amp; 0x1) == 0x0"/>',
            '<edc:DCRFieldSemantic edc:when="(field &amp; 0x1) == 0x1" edc:islanghidden="true"/>\n'
            '              <edc:DCRFieldSemantic edc:when="(field &amp; 0x1) == 0x0" edc:islanghidden="true"/>',
        )
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "all_hidden_semantics.atdf"
            src.write_text(tampered)
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
        self.assertNotEqual(
            r.returncode, 0, "a field with no non-hidden semantic must not generate"
        )
        self.assertIn("no non-hidden semantic", r.stderr)
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
            r, text = run_generator(
                src, name="p18syn01", pack="Microchip.PIC18Fxxxx_DFP"
            )
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
            "[18CFGTEST]\nARCH=PIC16\nROMSIZE=800\nRAMBANK=60-FF\nSTACKDEPTH=0x1F\n"
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

    def test_ini_romsize_is_halved_for_pic18(self):
        # epic-cc#351: the ini's ROMSIZE states PIC18 program memory in
        # bytes, same as EDC's CodeSector. ROMSIZE=800 (hex) is 2048 bytes,
        # 1024 words; the undivided bug would emit flash_words = 2048.
        with tempfile.TemporaryDirectory() as d:
            ini = self.write_ini(d)
            cfg = self.write_cfgdata(d)
            text = gen_device.generate_toml("p18cfgtest", ini, cfg, None)
        self.assertIn("flash_words = 1024", text)


class GenDeviceBaselineArchTest(unittest.TestCase):
    """Baseline parts name their architecture PIC12 in ini and 16c5x in
    EDC, neither of which the core maps knew. The PIC16Fxxx mid-range
    pack ships baseline PIC16F5x EDC next to the mid-range parts, so
    its sweep tripped over them (epic-cc#337). Baseline output also
    carries fsr_bank_bits (derived from the EDC GPR bank numbers,
    epic-cc#338) and no interrupt vector."""

    def write_ini(self, d):
        ini = pathlib.Path(d) / "p12syntest.ini"
        ini.write_text(
            "[12SYNTEST]\nARCH=PIC12\nROMSIZE=400\nRAMBANK=10-1F\nSTACKDEPTH=0x2\n"
        )
        return ini

    def write_cfgdata(self, d):
        cfg = pathlib.Path(d) / "p12syntest.cfgdata"
        cfg.write_text(
            "CWORD:FFF:1F:FFF:CONFIG\nCSETTING:4:WDTE\nCVALUE:4:ON\nCVALUE:0:OFF\n"
        )
        return cfg

    def write_edc(self, d, banks=(0, 1)):
        gprs = "\n".join(
            f'      <edc:GPRDataSector edc:regionid="gpr{b}" '
            f' edc:beginaddr="0x{b * 16:02X}" edc:endaddr="0x{b * 16 + 16:02X}"'
            f' edc:bank="0x{b}"/>'
            for b in banks
        )
        src = pathlib.Path(d) / "PICBASESYN.PIC"
        src.write_text(
            '<edc:PIC xmlns:edc="http://crownking/edc"'
            ' edc:name="PICBASESYN" edc:arch="16c5x">\n'
            '  <edc:ArchDef edc:name="16c5x">'
            '<edc:MemTraits edc:hwstackdepth="0x2"/></edc:ArchDef>\n'
            "  <edc:ProgramSpace>\n"
            '    <edc:CodeSector edc:beginaddr="0x0" edc:endaddr="0x400"/>\n'
            "  </edc:ProgramSpace>\n"
            '  <edc:DataSpace edc:endaddr="0x100">\n'
            "    <edc:RegardlessOfMode>\n"
            f"{gprs}\n"
            "    </edc:RegardlessOfMode>\n"
            "  </edc:DataSpace>\n"
            "</edc:PIC>\n"
        )
        return src

    def test_ini_arch_pic12_maps_to_pic_baseline(self):
        with tempfile.TemporaryDirectory() as d:
            ini = self.write_ini(d)
            cfg = self.write_cfgdata(d)
            edc = self.write_edc(d)
            text = gen_device.generate_toml(
                "p12syntest", ini, cfg, edc, pack="Microchip.PIC10-12Fxxx_DFP"
            )
        self.assertIn('core = "pic-baseline"', text)
        self.assertIn("flash_words = 1024", text)
        self.assertIn("fsr_bank_bits = 1", text)
        self.assertIn("interrupt_vectors = []", text)

    def test_edc_arch_16c5x_maps_to_pic_baseline(self):
        text = FIXTURE.read_text().replace("16xxxx", "16c5x")
        text = text.replace("PIC14SYN01", "PICBASESYN01")
        with tempfile.TemporaryDirectory() as d:
            src = pathlib.Path(d) / "PICBASESYN01.PIC"
            src.write_text(text)
            r, out = run_generator(
                src, name="picbasesyn01", pack="Microchip.PIC10-12Fxxx_DFP"
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn('core = "pic-baseline"', out)

    def test_eight_bank_baseline_part_is_missing_facts_not_capped(self):
        # Eight GPR banks need three FSR bank bits; the device schema
        # allows at most two for pic-baseline. Refusing names the
        # core-code decision instead of emitting a TOML the tree
        # cannot validate (epic-cc#338: p16f59, p16f570, p12f529t39a,
        # p12f529t48a).
        with tempfile.TemporaryDirectory() as d:
            ini = self.write_ini(d)
            cfg = self.write_cfgdata(d)
            edc = self.write_edc(d, banks=range(8))
            with self.assertRaises(gen_device.MissingFacts) as ctx:
                gen_device.generate_toml(
                    "p12syntest", ini, cfg, edc, pack="Microchip.PIC10-12Fxxx_DFP"
                )
        self.assertIn("fsr_bank_bits", str(ctx.exception.args[0]))

    def test_dcr_impl_mismatch_ignored_off_pic18(self):
        # The DCR covered==impl check verifies cursor math for its only
        # consumer, pic18. Baseline PIC10F EDC states impl 0x1c against
        # fields agreeing on 0x1d; parsing it anyway turned discarded
        # data into a hard failure (epic-cc#338).
        dcr = (
            '<edc:DCRDef edc:name="CONFIG" edc:_addr="0xFFF" edc:impl="0x1c">'
            "<edc:DCRModeList><edc:DCRMode>"
            '<edc:DCRFieldDef edc:name="OSC" edc:mask="0x1" edc:nzwidth="0x1">'
            '<edc:DCRFieldSemantic edc:cname="IntRC"'
            ' edc:when="(field &amp; 0x1) == 0x1"/>'
            "</edc:DCRFieldDef></edc:DCRMode></edc:DCRModeList></edc:DCRDef>"
        )

        def edc_with_arch(arch):
            return (
                '<edc:PIC xmlns:edc="http://crownking/edc"'
                f' edc:name="SYN" edc:arch="{arch}">'
                "  <edc:ProgramSpace>"
                '<edc:ConfigFuseSector edc:beginaddr="0xFFF"'
                f' edc:endaddr="0x1000">{dcr}</edc:ConfigFuseSector>'
                "  </edc:ProgramSpace></edc:PIC>"
            )

        with tempfile.TemporaryDirectory() as d:
            other = pathlib.Path(d) / "other.PIC"
            other.write_text(edc_with_arch("16c5x"))
            parsed = gen_device.parse_edc(other)
            self.assertEqual(parsed["config_sectors"], [(0xFFF, 0x1000)])
            self.assertNotIn("config_dcr", parsed)
            pic18 = pathlib.Path(d) / "pic18.PIC"
            pic18.write_text(edc_with_arch("18xxxx"))
            with self.assertRaises(gen_device.MissingFacts):
                gen_device.parse_edc(pic18)


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
                "\n".join(
                    line
                    for line in FIXTURE.read_text().splitlines()
                    if "GPRDataSector" not in line
                )
            )
            (edc / "BROKEN.PIC").write_text(
                '<edc:PIC xmlns:edc="http://crownking/edc">'
            )
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(pathlib.Path(d))],
                capture_output=True,
                text=True,
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
        self.assertIn("ok  pbasesyn01", r.stdout)
        self.assertIn("3/3 parts generated, 0 failed", r.stdout)

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
        self.assertEqual(written, ["p14syn01.toml", "p18syn01.toml", "pbasesyn01.toml"])

    def test_sweep_rejects_a_non_directory(self):
        r = subprocess.run(
            [sys.executable, str(GEN), "--sweep", str(GEN)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("is not a directory", r.stderr)

    def test_sweep_rejects_out_dir_pointing_at_a_file(self):
        with tempfile.TemporaryDirectory() as d:
            f = pathlib.Path(d) / "out"
            f.write_text("x")
            r = subprocess.run(
                [
                    sys.executable,
                    str(GEN),
                    "--sweep",
                    str(SWEEP_PACK),
                    "--out-dir",
                    str(f),
                ],
                capture_output=True,
                text=True,
            )
        self.assertEqual(r.returncode, 2)
        self.assertIn("is not a directory", r.stderr)

    def test_sweep_exits_1_when_no_parts_found(self):
        with tempfile.TemporaryDirectory() as d:
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(d)],
                capture_output=True,
                text=True,
            )
        self.assertEqual(r.returncode, 1)
        self.assertIn("no *.PIC files found", r.stderr)

    def test_sweep_rejects_single_part_flags(self):
        r = subprocess.run(
            [sys.executable, str(GEN), "--sweep", str(SWEEP_PACK), "--check"],
            capture_output=True,
            text=True,
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
                "[14SYN01]\nARCH=16xxxx\nROMSIZE=200\nRAMBANK=20-6F\nSTACKDEPTH=0x8\n"
            )
            out = pathlib.Path(d) / "out"
            r = subprocess.run(
                [sys.executable, str(GEN), "--sweep", str(pack), "--out-dir", str(out)],
                capture_output=True,
                text=True,
            )
            toml = (out / "p14syn01.toml").read_text()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("flash_words = 512", toml)


if __name__ == "__main__":
    unittest.main()
