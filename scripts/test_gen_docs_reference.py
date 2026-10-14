"""Coverage for scripts/gen_docs_reference.py (epic-cc#1280). No cargo, no
network: every helper under test is pure over hand-authored fixtures,
except capture_help, which is exercised by the docs-gen CI job running
the real binary.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
HELPER = ROOT / "scripts" / "gen_docs_reference.py"

_spec = importlib.util.spec_from_file_location("gen_docs_reference", HELPER)
gen_docs_reference = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gen_docs_reference)

PIC14_TOML = """\
name = "p16f877a"
core = "pic14"
flash_words = 8192
ram_banks = [[0x0020, 0x006F], [0x00A0, 0x00EF]]
common_ram = [0x0070, 0x007F]
"""

PIC18_TOML = """\
name = "p18f4550"
core = "pic18"
flash_words = 16384
ram_banks = [[0x0010, 0x07FF]]
access_bank = [0x0000, 0x005F]
"""


def write_devices(td: pathlib.Path) -> None:
    (td / "p16f877a.toml").write_text(PIC14_TOML)
    (td / "p18f4550.toml").write_text(PIC18_TOML)


CATALOG = {
    "p16f877a": ("PIC16F877A", "pic14", 8192),
    "p18f4550": ("PIC18F4550", "pic18", 16384),
}


class FamilyTests(unittest.TestCase):
    def test_stems_map_to_marketing_families(self):
        cases = {
            "p10f320": "PIC10F",
            "p12f509": "PIC12F",
            "p16f877a": "PIC16F",
            "p16lf873a": "PIC16LF",
            "p18f4550": "PIC18F",
            "p18lf46k22": "PIC18LF",
        }
        for stem, want in cases.items():
            self.assertEqual(gen_docs_reference.family_of(stem), want)

    def test_bare_stem_has_no_family(self):
        with self.assertRaises(ValueError):
            gen_docs_reference.family_of("weird")

    def test_display_inserts_pic(self):
        self.assertEqual(gen_docs_reference.display_of("p16f877a"), "PIC16F877A")


class RamTests(unittest.TestCase):
    def test_banks_plus_common(self):
        doc = {"ram_banks": [[0x20, 0x6F], [0xA0, 0xEF]], "common_ram": [0x70, 0x7F]}
        self.assertEqual(gen_docs_reference.ram_bytes(doc), 176)

    def test_banks_only(self):
        doc = {"ram_banks": [[0x10, 0x7FF]]}
        self.assertEqual(gen_docs_reference.ram_bytes(doc), 2032)


class StatusTests(unittest.TestCase):
    def test_empty_file_means_all_experimental(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            write_devices(td)
            rows = gen_docs_reference.build_device_rows(td, CATALOG, {})
        self.assertEqual([r["status"] for r in rows], ["experimental"] * 2)

    def test_listed_parts_use_their_status(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            write_devices(td)
            status = {"p18f4550": "supported"}
            rows = gen_docs_reference.build_device_rows(td, CATALOG, status)
        by_part = {r["part"]: r["status"] for r in rows}
        self.assertEqual(by_part["PIC18F4550"], "supported")
        self.assertEqual(by_part["PIC16F877A"], "experimental")

    def test_unknown_status_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td) / "device-status.toml"
            path.write_text('p16f877a = "beta"\n')
            with self.assertRaises(ValueError):
                gen_docs_reference.load_status(path)

    def test_status_naming_an_unknown_device_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            write_devices(td)
            with self.assertRaises(ValueError):
                gen_docs_reference.build_device_rows(
                    td, CATALOG, {"p99f9999": "supported"}
                )

    def test_status_key_shadowed_by_a_status_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td) / "device-status.toml"
            path.write_text('supported = "experimental"\n')
            with self.assertRaises(ValueError):
                gen_docs_reference.load_status(path)


class CatalogCheckTests(unittest.TestCase):
    def test_missing_catalog_row_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            write_devices(td)
            catalog = dict(CATALOG)
            del catalog["p18f4550"]
            with self.assertRaises(ValueError):
                gen_docs_reference.build_device_rows(td, catalog, {})

    def test_core_or_flash_drift_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            write_devices(td)
            catalog = dict(CATALOG)
            catalog["p16f877a"] = ("PIC16F877A", "pic14e", 8192)
            with self.assertRaises(ValueError):
                gen_docs_reference.build_device_rows(td, catalog, {})
            catalog["p16f877a"] = ("PIC16F877A", "pic14", 4096)
            with self.assertRaises(ValueError):
                gen_docs_reference.build_device_rows(td, catalog, {})

    def test_unknown_core_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "px.toml").write_text(
                'name = "px"\ncore = "pic99"\nflash_words = 64\n'
            )
            catalog = {"px": ("PX", "pic99", 64)}
            with self.assertRaises(ValueError):
                gen_docs_reference.build_device_rows(td, catalog, {})

    def test_row_facts(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            write_devices(td)
            rows = gen_docs_reference.build_device_rows(td, CATALOG, {})
        self.assertEqual(
            rows[0],
            {
                "part": "PIC16F877A",
                "family": "PIC16F",
                "core": "PIC14",
                "flash": 8192,
                "ram": 176,
                "status": "experimental",
            },
        )
        self.assertEqual(rows[1]["core"], "PIC18")


class RenderTests(unittest.TestCase):
    def test_devices_page_shape(self):
        text = gen_docs_reference.render_devices(
            [
                {
                    "part": "PIC16F877A",
                    "family": "PIC16F",
                    "core": "PIC14",
                    "flash": 8192,
                    "ram": 176,
                    "status": "experimental",
                }
            ]
        )
        lines = text.split("\n")
        self.assertTrue(lines[0].startswith("<!-- generated by "))
        self.assertTrue(lines[0].endswith("do not edit -->"))
        self.assertIn(
            "| PIC16F877A | PIC16F | PIC14 | 8192 | 176 | experimental |", lines
        )
        self.assertTrue(text.endswith("\n") and not text.endswith("\n\n"))
        self.assertNotIn("\u2014", text)

    def test_cli_page_embeds_help_verbatim(self):
        text = gen_docs_reference.render_cli([("epic-cc --help", "usage: epic-cc\n")])
        self.assertTrue(text.split("\n")[0].startswith("<!-- generated by "))
        self.assertIn("## `epic-cc --help`", text)
        self.assertIn("```text\nusage: epic-cc\n```", text)


class SubcommandTests(unittest.TestCase):
    def test_no_commands_section_finds_none(self):
        self.assertEqual(
            gen_docs_reference.discover_subcommands("usage: epic-cc [options]\n"), []
        )

    def test_commands_section_lists_names(self):
        help_text = (
            "usage: epic-cc <command> [options]\n\nCommands:\n"
            "  build    compile the project\n"
            "  clean    remove artifacts\n"
        )
        self.assertEqual(
            gen_docs_reference.discover_subcommands(help_text), ["build", "clean"]
        )


class IncludeTests(unittest.TestCase):
    def test_include_splices_file_content(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "frag.md").write_text("line one\nline two\n")
            out = gen_docs_reference.expand_includes(
                'before\n--8<-- "frag.md"\nafter\n', td
            )
        self.assertEqual(out, "before\nline one\nline two\nafter\n")

    def test_line_region_selects_rows(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "frag.md").write_text("one\ntwo\nthree\n")
            out = gen_docs_reference.expand_includes('--8<-- "frag.md:2:3"\n', td)
        self.assertEqual(out, "two\nthree\n")

    def test_missing_include_fails(self):
        with tempfile.TemporaryDirectory() as td:
            with self.assertRaises(ValueError):
                gen_docs_reference.expand_includes(
                    '--8<-- "gone.md"\n', pathlib.Path(td)
                )

    def test_cycle_fails(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "a.md").write_text('--8<-- "b.md"\n')
            (td / "b.md").write_text('--8<-- "a.md"\n')
            with self.assertRaises(ValueError):
                gen_docs_reference.expand_includes(
                    '--8<-- "a.md"\n', td, (td.resolve(),)
                )


if __name__ == "__main__":
    unittest.main()
