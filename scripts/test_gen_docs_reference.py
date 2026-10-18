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
                "name": "p16f877a",
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


class GdbserverTests(unittest.TestCase):
    GDB_USAGE = "usage: epic-cc-gdbserver <hex> <sidecar.elf> --port <port>"

    def _stub(self, path, body):
        path.write_text(f"#!/bin/sh\n{body}\n")
        path.chmod(0o755)
        return str(path)

    def test_gdbserver_help_captured_from_stdout(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            binary = self._stub(
                td / "epic-cc",
                'echo "usage: epic-cc [options]"',
            )
            gdb = self._stub(
                td / "epic-cc-gdbserver",
                f'echo "{self.GDB_USAGE}"\nexit 0',
            )
            sections = gen_docs_reference.build_cli_sections(binary, gdb)
        self.assertEqual(
            sections,
            [
                ("epic-cc --help", "usage: epic-cc [options]\n"),
                ("epic-cc-gdbserver --help", f"{self.GDB_USAGE}\n"),
            ],
        )
        text = gen_docs_reference.render_cli(sections)
        self.assertIn("## `epic-cc-gdbserver --help`", text)
        self.assertIn(f"```text\n{self.GDB_USAGE}\n```", text)

    def test_gdbserver_help_failure_raises(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            gdb = self._stub(td / "epic-cc-gdbserver", "exit 2")
            with self.assertRaises(RuntimeError):
                gen_docs_reference.capture_gdbserver_help(str(gdb))


class UserPageTests(unittest.TestCase):
    ROWS = [
        {
            "name": "p16f877a",
            "part": "PIC16F877A",
            "family": "PIC16F",
            "core": "PIC14",
            "flash": 8192,
            "ram": 176,
            "status": "experimental",
        },
        {
            "name": "p16f1937",
            "part": "PIC16F1937",
            "family": "PIC16F",
            "core": "PIC14E",
            "flash": 8192,
            "ram": 512,
            "status": "supported",
        },
    ]
    FACTS = {
        "p16f877a": {
            "eeprom_bytes": 256,
            "lifecycle": "Active",
            "pdf": "https://x/a.pdf",
        },
        "p16f1937": {"lifecycle": "Active"},
    }

    def test_slug_is_url_safe(self):
        self.assertEqual(
            gen_docs_reference.family_slug("PIC12F", "PIC12 baseline"),
            "pic12f-pic12-baseline",
        )

    def test_groups_split_by_core(self):
        groups = gen_docs_reference.group_families(self.ROWS)
        self.assertEqual(list(groups), [("PIC16F", "PIC14"), ("PIC16F", "PIC14E")])

    def test_matrix_links_family_page(self):
        text = gen_docs_reference.render_user_devices(self.ROWS)
        self.assertIn("[PIC16F](families/pic16f-pic14e.md)", text)
        self.assertNotIn("\u2014", text)

    def test_family_page_uses_catalog_facts(self):
        text = gen_docs_reference.render_family(
            "PIC16F", "PIC14", self.ROWS[:1], self.FACTS
        )
        self.assertIn("# PIC16F (PIC14)", text)
        self.assertIn(
            "| PIC16F877A | 8192 | 176 | 256 | experimental | Active "
            "| [datasheet](https://x/a.pdf) |",
            text,
        )
        self.assertIn("## Notes", text)

    def test_family_page_marks_missing_facts(self):
        text = gen_docs_reference.render_family(
            "PIC16F", "PIC14E", self.ROWS[1:], self.FACTS
        )
        self.assertIn("| none | supported | Active | not listed |", text)

    def test_write_removes_stale_family_pages(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            g = gen_docs_reference
            saved = (g.USER_DEVICES_DIR, g.USER_FAMILIES_DIR, g.USER_CLI_MD)
            g.USER_DEVICES_DIR = td / "devices"
            g.USER_FAMILIES_DIR = td / "devices" / "families"
            g.USER_CLI_MD = td / "compiler" / "cli-flags.md"
            try:
                g.USER_FAMILIES_DIR.mkdir(parents=True)
                (g.USER_FAMILIES_DIR / "old.md").write_text("x")
                g.write_user_pages(self.ROWS, self.FACTS, [("epic-cc --help", "u\n")])
                names = sorted(p.name for p in g.USER_FAMILIES_DIR.glob("*.md"))
                self.assertEqual(names, ["pic16f-pic14.md", "pic16f-pic14e.md"])
                self.assertTrue((g.USER_DEVICES_DIR / "index.md").is_file())
                self.assertTrue(g.USER_CLI_MD.is_file())
            finally:
                g.USER_DEVICES_DIR, g.USER_FAMILIES_DIR, g.USER_CLI_MD = saved

    def test_cli_page_title(self):
        text = gen_docs_reference.render_user_cli([("epic-cc --help", "u\n")])
        self.assertIn("# CLI flags", text)


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

    def test_out_of_range_region_fails(self):
        with tempfile.TemporaryDirectory() as td:
            td = pathlib.Path(td)
            (td / "frag.md").write_text("one\ntwo\nthree\n")
            for region in ("100:200", "0:2", "3:2", "1:4"):
                with self.assertRaises(ValueError, msg=region):
                    gen_docs_reference.expand_includes(
                        f'--8<-- "frag.md:{region}"\n', td
                    )

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
