"""device_names.py against a hand-authored EDC fixture. No vendor file is
committed. Run by scripts/ci-test.sh alongside the cargo suites."""

import pathlib
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "device_names.py"
FIXTURE = ROOT / "scripts" / "fixtures" / "synthetic_names.atdf"

TOML = """name = "psyn02"
core = "pic14"

[config]
base_byte_addr = 0x400E
num_bytes = 2
erased_baseline = [0xFF, 0x3F]

[[config.fields]]
name = "osc"
byte_offset = 0
mask = 0x03
shift = 0
values = [{ name = "lp", bits = 0 }, { name = "hs", bits = 2 }, { name = "rc", bits = 3 }]

[[config.fields]]
name = "wdt"
byte_offset = 0
mask = 0x04
shift = 2
values = [{ name = "off", bits = 0 }, { name = "on", bits = 1 }]

[[config.fields]]
name = "cpd"
byte_offset = 1
mask = 0x01
shift = 0
values = [{ name = "on", bits = 0 }, { name = "off", bits = 1 }]

[[config.fields]]
name = "ghost"
byte_offset = 1
mask = 0x20
shift = 5
values = [{ name = "on", bits = 1 }]

[provenance]
tier = "datasheet"
"""


def run(path, *extra):
    return subprocess.run(
        [
            sys.executable,
            str(SCRIPT),
            "psyn02",
            "--atdf",
            str(FIXTURE),
            "--toml",
            str(path),
            *extra,
        ],
        capture_output=True,
        text=True,
    )


class DeviceNames(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp())
        self.path = self.dir / "psyn02.toml"
        self.path.write_text(TOML)
        self.result = run(self.path)
        self.data = tomllib.loads(self.path.read_text())

    def tearDown(self):
        shutil.rmtree(self.dir)

    def sfr(self, name):
        return next(s for s in self.data["sfrs"] if s["name"] == name)

    def test_config_aliases_match_by_bit_position(self):
        osc, wdt, cpd, _ = self.data["config"]["fields"]
        self.assertEqual(osc["aliases"], ["FOSC"])
        self.assertEqual(wdt["aliases"], ["WDTE"])
        self.assertNotIn("aliases", cpd)
        rc = next(v for v in osc["values"] if v["name"] == "rc")
        self.assertEqual(rc["aliases"], ["EXTRC"])
        hs = next(v for v in osc["values"] if v["name"] == "hs")
        self.assertNotIn("aliases", hs)

    def test_an_unmatched_field_is_reported(self):
        self.assertEqual(self.result.returncode, 0)
        self.assertIn("'ghost'", self.result.stderr)

    def test_fields_walk_adjust_points_and_dedupe_modes(self):
        fields = {f["name"]: f for f in self.sfr("PORTB")["fields"]}
        self.assertEqual((fields["RB3"]["mask"], fields["RB3"]["shift"]), (0x08, 3))
        self.assertEqual((fields["HI"]["mask"], fields["HI"]["shift"]), (0x30, 4))
        self.assertEqual((fields["ALT1"]["shift"], fields["ALT1"]["mode"]), (1, 1))
        self.assertEqual(list(fields).count("RB0"), 1)

    def test_register_shapes(self):
        self.assertEqual(self.sfr("PORTB")["aliases"], ["GPIO"])
        tmr1 = self.sfr("TMR1")
        self.assertEqual((tmr1["addr"], tmr1["width"], tmr1["fields"]), (0x0E, 2, []))
        self.assertEqual(self.sfr("TMR1L")["fields"], [])
        self.assertEqual(self.sfr("SSPMSK")["addr"], self.sfr("SSPADD")["addr"])
        names = [s["name"] for s in self.data["sfrs"]]
        self.assertNotIn("SECRET", names)
        self.assertNotIn("WREG", names)

    def test_rerun_is_idempotent_and_check_passes(self):
        before = self.path.read_text()
        self.assertEqual(run(self.path).returncode, 0)
        self.assertEqual(self.path.read_text(), before)
        self.assertEqual(run(self.path, "--check").returncode, 0)

    def test_check_flags_a_stale_file(self):
        self.path.write_text(TOML)
        self.assertEqual(run(self.path, "--check").returncode, 1)


if __name__ == "__main__":
    unittest.main()
