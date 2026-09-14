"""Coverage for scripts/gen_devices_manifest.py (epic-cc#416). No cargo, no
network: build_manifest is a pure function over hand-authored TOML fixtures.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
HELPER = ROOT / "scripts" / "gen_devices_manifest.py"

_spec = importlib.util.spec_from_file_location("gen_devices_manifest", HELPER)
gen_devices_manifest = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gen_devices_manifest)

WITH_PACK = """\
name = "p16fsyn01"
core = "pic14"
flash_words = 8192

[provenance]
tier = "atdf"
source = "PIC16FSYN01.PIC"
pack = "Microchip.PIC16Fxxx_DFP"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
"""

NO_PROVENANCE = """\
name = "p16fsyn02"
core = "pic14"
flash_words = 8192
"""


class BuildManifestTests(unittest.TestCase):
    def test_extracts_name_core_and_pack(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "p16fsyn01.toml").write_text(WITH_PACK)
            manifest = gen_devices_manifest.build_manifest(td)
        self.assertEqual(
            manifest,
            [
                {
                    "name": "p16fsyn01",
                    "core": "pic14",
                    "pack": "Microchip.PIC16Fxxx_DFP",
                }
            ],
        )

    def test_missing_provenance_yields_none_pack_not_a_crash(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "p16fsyn02.toml").write_text(NO_PROVENANCE)
            manifest = gen_devices_manifest.build_manifest(td)
        self.assertEqual(
            manifest, [{"name": "p16fsyn02", "core": "pic14", "pack": None}]
        )

    def test_sorted_by_filename_deterministic_order(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "p18fsyn.toml").write_text(
                NO_PROVENANCE.replace("p16fsyn02", "p18fsyn").replace("pic14", "pic18")
            )
            (td / "p10fsyn.toml").write_text(
                NO_PROVENANCE.replace("p16fsyn02", "p10fsyn")
            )
            manifest = gen_devices_manifest.build_manifest(td)
        self.assertEqual([d["name"] for d in manifest], ["p10fsyn", "p18fsyn"])

    def test_real_device_registry_has_no_duplicates_and_every_core_is_known(self):
        manifest = gen_devices_manifest.build_manifest(gen_devices_manifest.DEVICES_DIR)
        names = [d["name"] for d in manifest]
        self.assertEqual(len(names), len(set(names)), "duplicate device name")
        known_cores = {"pic14", "pic14e", "pic18", "pic-baseline"}
        for d in manifest:
            self.assertIn(d["core"], known_cores, d)

    def test_real_registry_null_pack_always_has_a_documented_reason(self):
        # Pins the invariant this module's docstring states, against the
        # real registry, rather than a hardcoded device-name snapshot:
        # pack is None only for a TOML with no [provenance] at all, or a
        # tier="datasheet" entry with no pack key of its own.
        for path in sorted(gen_devices_manifest.DEVICES_DIR.glob("*.toml")):
            doc = gen_devices_manifest.add_device.parse_toml(path.read_text())
            if doc.get("provenance", {}).get("pack") is not None:
                continue
            provenance = doc.get("provenance")
            reason_ok = provenance is None or provenance.get("tier") == "datasheet"
            self.assertTrue(reason_ok, f"{path.name}: null pack, undocumented reason")


if __name__ == "__main__":
    unittest.main()
