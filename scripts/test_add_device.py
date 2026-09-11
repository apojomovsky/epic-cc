"""Coverage for scripts/add_device.py, the data transforms behind
scripts/add-device.sh (docs/38 D-1). No vendor file, no network, no cargo:
each helper is a pure function over a hand-authored TOML or .lkr.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
HELPER = ROOT / "scripts" / "add_device.py"

_spec = importlib.util.spec_from_file_location("add_device", HELPER)
add_device = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(add_device)

# A minimal PIC18 TOML in gen-device.py's deterministic shape, with one
# locked field, one defaulted field, and one plain field.
PIC18_TOML = """\
name = "p18syn01"
core = "pic18"
flash_words = 1024
ram_banks = [[0x0010, 0x009F]]
access_bank = [0x0000, 0x005F]
fixed_retval = [0x0000, 0x000F]
stack_depth = 16
interrupt_vectors = [0x0008, 0x0018]

[config]
base_byte_addr = 0x300000
num_bytes = 3
erased_baseline = [0xFF, 0xFF, 0xFF]

[[config.fields]]
name = "alpha"
byte_offset = 0
mask = 0x01
shift = 0
values = [{ name = "off", bits = 0 }, { name = "on", bits = 1 }]

[[config.fields]]
name = "gamma"
byte_offset = 2
mask = 0x03
shift = 0
default = "one"
values = [{ name = "zero", bits = 0 }, { name = "one", bits = 1 }, { name = "three", bits = 3 }]

[[config.fields]]
name = "xinst"
byte_offset = 6
mask = 0x40
shift = 6
default = "off"
locked = "off"
values = [{ name = "off", bits = 0 }, { name = "on", bits = 1 }]
"""

# A PIC14 TOML: banked GPR only, no access bank or fixed_retval.
PIC14_TOML = """\
name = "p14syn01"
core = "pic14"
flash_words = 1024
ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]
common_ram = [0x0070, 0x007F]
stack_depth = 8
interrupt_vectors = [0x0004]

[config]
base_byte_addr = 0x2007
num_bytes = 1
erased_baseline = [0xFF]

[[config.fields]]
name = "osc"
byte_offset = 0
mask = 0x07
shift = 0
values = [{ name = "lp", bits = 0 }, { name = "xt", bits = 1 }, { name = "hs", bits = 2 }]
"""

# A PIC18 .lkr with the DFP-understated shape: gpr0-gpr3 only, while the
# real part has gpr0-gpr7 (the p18f2550 defect, docs/32 §3).
PIC18_LKR = """\
CODEPAGE   NAME=page       START=0x0               END=0x7FFF
ACCESSBANK NAME=accessram  START=0x0               END=0x5F
DATABANK   NAME=gpr0       START=0x60              END=0xFF
DATABANK   NAME=gpr1       START=0x100             END=0x1FF
DATABANK   NAME=gpr2       START=0x200             END=0x2FF
DATABANK   NAME=gpr3       START=0x300             END=0x3FF
"""

# The same part with the full gpr0-gpr7 (what gputils actually ships).
PIC18_LKR_FULL = (
    PIC18_LKR
    + """\
DATABANK   NAME=gpr4       START=0x400             END=0x4FF
DATABANK   NAME=gpr5       START=0x500             END=0x5FF
DATABANK   NAME=gpr6       START=0x600             END=0x6FF
DATABANK   NAME=gpr7       START=0x700             END=0x7FF
"""
)

# A PIC14 .lkr: banked gpr plus a shared window.
PIC14_LKR = """\
DATABANK   NAME=sfr0       START=0x0               END=0x1F           PROTECTED
DATABANK   NAME=gpr0       START=0x20              END=0x6F
DATABANK   NAME=gpr1       START=0xA0              END=0xEF
SHAREBANK  NAME=gprnobnk   START=0x70            END=0x7F
"""

# PIC12F675's shape: gputils names the device's whole (and only) GPR
# window a SHAREBANK ("gprnobank"), not a DATABANK gpr*, because bank 1's
# window is a full mirror of bank 0. `banks` here is empty even though
# the device has real, already-correct RAM.
PIC14_LKR_ALL_SHARED = """\
DATABANK   NAME=sfr0       START=0x0               END=0x1F           PROTECTED
DATABANK   NAME=sfr1       START=0x80              END=0x9F           PROTECTED
SHAREBANK  NAME=gprnobank  START=0x20              END=0x5F
SHAREBANK  NAME=gprnobank  START=0xA0              END=0xDF           PROTECTED
"""

# PIC10F320/322's shape: a true single bank, no aliasing at all, so
# gputils calls the whole thing a plain DATABANK, never a SHAREBANK
# (there is no second bank to protect against). gen-device.py's D-1 carve
# still splits it into ram_banks + common_ram (docs/39).
PIC14_LKR_SINGLE_BANK = """\
DATABANK   NAME=sfr0       START=0x0               END=0x3F           PROTECTED
DATABANK   NAME=gpr0       START=0x40              END=0x7F
"""

# A TOML already carved the D-1 way for a single-bank device: ram_banks is
# the DATABANK minus the top 16 bytes, common_ram is that top 16.
PIC14_SINGLE_BANK_TOML = PIC14_TOML.replace(
    "ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]\ncommon_ram = [0x0070, 0x007F]\n",
    "ram_banks = [[0x0040, 0x006F]]\ncommon_ram = [0x0070, 0x007F]\n",
)


def write(path, text):
    path.write_text(text)
    return path


class NormalizeStemTest(unittest.TestCase):
    def test_normalizes_any_spelling(self):
        self.assertEqual(add_device.normalize_stem("PIC16F887"), "p16f887")
        self.assertEqual(add_device.normalize_stem("p16f887"), "p16f887")
        self.assertEqual(add_device.normalize_stem("16f887"), "p16f887")


class SynthesizeConfigTest(unittest.TestCase):
    def test_uses_locked_then_default_then_first_value(self):
        with tempfile.TemporaryDirectory() as d:
            p = write(pathlib.Path(d) / "p18syn01.toml", PIC18_TOML)
            spec = add_device.synthesize_config(p)
        # alpha has no default: first enumerated value (off).
        self.assertIn("alpha=off", spec)
        # gamma has a default: one.
        self.assertIn("gamma=one", spec)
        # xinst is locked to off: the locked value wins over its default.
        self.assertIn("xinst=off", spec)
        # Every declared field is covered.
        for name in ("alpha", "gamma", "xinst"):
            self.assertIn(f"{name}=", spec)

    def test_locked_without_default_does_not_false_fail(self):
        # A locked field with no default must still synthesize (docs/38
        # D-1 step 4: handle locked explicitly, not assume a default).
        toml = PIC18_TOML.replace('default = "off"\nlocked = "off"', 'locked = "off"')
        with tempfile.TemporaryDirectory() as d:
            p = write(pathlib.Path(d) / "p18syn01.toml", toml)
            spec = add_device.synthesize_config(p)
        self.assertIn("xinst=off", spec)


class SynthesizeXtalTest(unittest.TestCase):
    def test_crystal_mode_uses_4mhz(self):
        with tempfile.TemporaryDirectory() as d:
            p = write(pathlib.Path(d) / "p18syn01.toml", PIC18_TOML)
            self.assertEqual(add_device.synthesize_xtal_hz(p), "4000000")

    def test_pll_mode_scales_by_plldiv(self):
        # osc=hspll with plldiv=div4 needs xtal = 4 MHz * 4 = 16 MHz.
        toml = PIC18_TOML.replace(
            'name = "alpha"',
            'name = "osc"\nbyte_offset = 1\nmask = 0x0F\nshift = 0\n'
            'values = [{ name = "hspll", bits = 14 }]\n\n[[config.fields]]\nname = "plldiv"\n'
            'byte_offset = 0\nmask = 0x07\nshift = 0\nvalues = [{ name = "div4", bits = 3 }]\n\n'
            '[[config.fields]]\nname = "alpha"',
        )
        with tempfile.TemporaryDirectory() as d:
            p = write(pathlib.Path(d) / "p18syn01.toml", toml)
            self.assertEqual(add_device.synthesize_xtal_hz(p), "16000000")


class CorrectRamTest(unittest.TestCase):
    def test_pic18_widens_understated_ram(self):
        with tempfile.TemporaryDirectory() as d:
            toml = write(pathlib.Path(d) / "p18syn01.toml", PIC18_TOML)
            lkr = write(pathlib.Path(d) / "18syn01_g.lkr", PIC18_LKR_FULL)
            changed = add_device.correct_ram(toml, lkr)
            text = toml.read_text()
        self.assertTrue(changed)
        self.assertIn("ram_banks = [[0x0010, 0x07FF]]", text)

    def test_pic18_noop_when_ram_already_matches(self):
        with tempfile.TemporaryDirectory() as d:
            # TOML already widened to the lkr's gpr0-gpr3 top (0x3FF).
            toml = write(
                pathlib.Path(d) / "p18syn01.toml",
                PIC18_TOML.replace(
                    "ram_banks = [[0x0010, 0x009F]]", "ram_banks = [[0x0010, 0x03FF]]"
                ),
            )
            lkr = write(pathlib.Path(d) / "18syn01_g.lkr", PIC18_LKR)
            changed = add_device.correct_ram(toml, lkr)
        self.assertFalse(changed)

    def test_pic14_uses_banked_gpr_only(self):
        with tempfile.TemporaryDirectory() as d:
            toml = write(pathlib.Path(d) / "p14syn01.toml", PIC14_TOML)
            lkr = write(pathlib.Path(d) / "14syn01_g.lkr", PIC14_LKR)
            changed = add_device.correct_ram(toml, lkr)
            text = toml.read_text()
        self.assertFalse(changed)
        self.assertIn("ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]", text)

    def test_pic14_widens_understated_ram(self):
        # The DFP understates a PIC14 part's banked GPR; gputils wins and
        # the TOML's ram_banks must widen to the lkr's banks (docs/32 §3).
        with tempfile.TemporaryDirectory() as d:
            toml = write(
                pathlib.Path(d) / "p14syn01.toml",
                PIC14_TOML.replace(
                    "ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]",
                    "ram_banks = [[0x0020, 0x006F]]",
                ),
            )
            lkr = write(pathlib.Path(d) / "14syn01_g.lkr", PIC14_LKR)
            changed = add_device.correct_ram(toml, lkr)
            text = toml.read_text()
        self.assertTrue(changed)
        self.assertIn("ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]", text)

    def test_pic14_keeps_ram_when_lkr_has_no_databank_gpr(self):
        # PIC12F675: gputils calls the device's only GPR window a
        # SHAREBANK, not a DATABANK gpr*, so `banks` parses empty. That
        # must not overwrite gen-device.py's already-correct ram_banks
        # with nothing (this compiler's common_ram is a narrow,
        # fixed-scratch-only concept, never a stand-in for "gputils called
        # it SHAREBANK").
        with tempfile.TemporaryDirectory() as d:
            toml = write(
                pathlib.Path(d) / "p14syn01.toml",
                PIC14_TOML.replace(
                    "ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]\n"
                    "common_ram = [0x0070, 0x007F]\n",
                    "ram_banks = [[0x0020, 0x005F]]\n",
                ),
            )
            lkr = write(pathlib.Path(d) / "14syn01_g.lkr", PIC14_LKR_ALL_SHARED)
            changed = add_device.correct_ram(toml, lkr)
            text = toml.read_text()
        self.assertFalse(changed)
        self.assertIn("ram_banks = [[0x0020, 0x005F]]", text)

    def test_pic14_single_bank_noop_when_already_split_correctly(self):
        # PIC10F320/322: gputils' one DATABANK (0x40-0x7F) exactly equals
        # the union of an already-carved ram_banks (0x40-0x6F) and
        # common_ram (0x70-0x7F). Widening to the full DATABANK span would
        # overlap common_ram; confirmed this used to happen before the
        # common_ram-aware clip (docs/39 D-1, the p10f320 add-device.sh
        # failure that found this).
        with tempfile.TemporaryDirectory() as d:
            toml = write(pathlib.Path(d) / "p14syn01.toml", PIC14_SINGLE_BANK_TOML)
            lkr = write(pathlib.Path(d) / "14syn01_g.lkr", PIC14_LKR_SINGLE_BANK)
            changed = add_device.correct_ram(toml, lkr)
            text = toml.read_text()
        self.assertFalse(changed)
        self.assertIn("ram_banks = [[0x0040, 0x006F]]", text)
        self.assertIn("common_ram = [0x0070, 0x007F]", text)

    def test_pic14_single_bank_widens_without_touching_common_ram(self):
        # A genuinely understated ram_banks (DFP said less GPR than
        # gputils) on the same single-bank shape must still widen, but
        # never past the carved common_ram window.
        with tempfile.TemporaryDirectory() as d:
            toml = write(
                pathlib.Path(d) / "p14syn01.toml",
                PIC14_SINGLE_BANK_TOML.replace(
                    "ram_banks = [[0x0040, 0x006F]]", "ram_banks = [[0x0050, 0x006F]]"
                ),
            )
            lkr = write(pathlib.Path(d) / "14syn01_g.lkr", PIC14_LKR_SINGLE_BANK)
            changed = add_device.correct_ram(toml, lkr)
            text = toml.read_text()
        self.assertTrue(changed)
        self.assertIn("ram_banks = [[0x0040, 0x006F]]", text)
        self.assertIn("common_ram = [0x0070, 0x007F]", text)


class FieldDiffTest(unittest.TestCase):
    def test_names_new_absent_and_renamed_fields(self):
        with tempfile.TemporaryDirectory() as d:
            gen = write(pathlib.Path(d) / "p18syn01.toml", PIC18_TOML)
            # Sibling: same alpha/gamma, no xinst, plus a field at a
            # position the generated TOML does not use.
            sib = write(
                pathlib.Path(d) / "p18syn02.toml",
                PIC18_TOML.replace('name = "xinst"', 'name = "icprt"').replace(
                    "byte_offset = 6\nmask = 0x40\nshift = 6",
                    "byte_offset = 6\nmask = 0x20\nshift = 5",
                ),
            )
            lines = add_device.field_diff(gen, sib)
        text = "\n".join(lines)
        self.assertIn("+ xinst", text)
        self.assertIn("- icprt", text)


class SiblingTest(unittest.TestCase):
    def test_picks_closest_same_core(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            write(d / "p18syn01.toml", PIC18_TOML)
            write(
                d / "p18syn02.toml",
                PIC18_TOML.replace('name = "p18syn01"', 'name = "p18syn02"'),
            )
            write(d / "p14syn01.toml", PIC14_TOML)
            sib = add_device.sibling(d / "p18syn01.toml", d)
        self.assertEqual(sib.name, "p18syn02.toml")

    def test_returns_none_when_no_same_core_sibling(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            write(d / "p18syn01.toml", PIC18_TOML)
            write(d / "p14syn01.toml", PIC14_TOML)
            sib = add_device.sibling(d / "p14syn01.toml", d)
        self.assertIsNone(sib)


if __name__ == "__main__":
    unittest.main()
