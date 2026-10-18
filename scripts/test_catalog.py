#!/usr/bin/env python3
"""Behavior tests for catalog.py merge precedence (epic-cc#1362).

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import argparse
import contextlib
import io
import json
import pathlib
import tempfile
import unittest

import catalog


def build(gp_rows, dfp_rows, scores=None, mcp=None):
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        gp = root / "gputils.json"
        dfp = root / "dfp.json"
        gp.write_text(json.dumps(gp_rows))
        dfp.write_text(json.dumps(dfp_rows))
        score_path = root / "scores.json"
        mcp_path = root / "mcp.json"
        if scores is None:
            score_path = root / "no-scores.json"
        else:
            score_path.write_text(json.dumps(scores))
        if mcp is None:
            mcp_path = root / "no-mcp.json"
        else:
            mcp_path.write_text(json.dumps(mcp))
        out = root / "catalog.toml"
        args = argparse.Namespace(
            gputils=str(gp),
            dfp=str(dfp),
            scores=str(score_path),
            mcp=str(mcp_path),
            page_template="https://example.invalid/{display}",
            date="2026-01-01",
            out=str(out),
        )
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = catalog.cmd_build(args)
        return rc, out.read_text(), err.getvalue()


def gp(display, cost_core="pic14", **kw):
    row = {"display": display, "name": "p" + display[3:].lower()}
    if cost_core is not None:
        row["core"] = cost_core
    row.update(kw)
    return row


def dfp(display, pack="Microchip.PIC16Fxxx_DFP", core="pic14", **kw):
    row = {"display": display, "name": "p" + display[3:].lower(), "pack": pack}
    if core is not None:
        row["core"] = core
    row.update(kw)
    return row


class ClassifyCoreTests(unittest.TestCase):
    def test_pic18_by_name(self):
        self.assertEqual(catalog.classify_core("p18f4550", ""), "pic18")

    def test_bsr_means_enhanced(self):
        self.assertEqual(catalog.classify_core("p16f1786", "BSR EQU 0x10\n"), "pic14e")

    def test_pclath_means_midrange(self):
        self.assertEqual(
            catalog.classify_core("p16f877a", "PCLATH EQU 0x0A\n"), "pic14"
        )

    def test_neither_means_baseline(self):
        self.assertEqual(catalog.classify_core("p16f54", "W EQU 0\n"), "pic-baseline")


class ParseLkrTests(unittest.TestCase):
    LKR = (
        "CODEPAGE NAME=page START=0x0000 END=0x07FF\n"
        "CODEPAGE NAME=page1 START=0x0800 END=0x0FFF PROTECTED\n"
        "CODEPAGE NAME=config START=0x2000 END=0x2007\n"
        "CODEPAGE NAME=eedata START=0x2100 END=0x21FF\n"
        "DATABANK NAME=gpr0 START=0x0020 END=0x007F\n"
        "DATABANK NAME=gpr1 START=0x00A0 END=0x00FF\n"
        "SHAREBANK NAME=shared START=0x0070 END=0x007F\n"
        "DATABANK NAME=bad START=0x0120 END=0x013F PROTECTED\n"
    )

    def test_pic14_words_ram_eeprom(self):
        facts = catalog.parse_lkr(self.LKR, "pic14")
        self.assertEqual(facts["flash_words"], 0x800)
        self.assertEqual(facts["eeprom_bytes"], 0x100)
        self.assertEqual(facts["ram_bytes"], 0x60 + 0x60)

    def test_pic18_codepages_are_byte_addressed(self):
        facts = catalog.parse_lkr(
            "CODEPAGE NAME=page START=0x000000 END=0x000FFF\n", "pic18"
        )
        self.assertEqual(facts["flash_words"], 0x800)

    def test_oscval_reserved_word_counts_as_flash(self):
        facts = catalog.parse_lkr(
            "CODEPAGE NAME=page START=0x0000 END=0x03FE\n"
            "CODEPAGE NAME=.oscval START=0x03FF END=0x03FF PROTECTED\n",
            "pic14",
        )
        self.assertEqual(facts["flash_words"], 0x400)


class CoalesceTests(unittest.TestCase):
    def test_overlapping_and_adjacent_merge(self):
        self.assertEqual(
            catalog.coalesce([(0x20, 0x7F), (0x70, 0xFF), (0x200, 0x20F)]),
            [[0x20, 0xFF], [0x200, 0x20F]],
        )

    def test_unsorted_and_empty(self):
        self.assertEqual(catalog.coalesce([(5, 6), (1, 2)]), [[1, 2], [5, 6]])
        self.assertEqual(catalog.coalesce([]), [])


class TierTests(unittest.TestCase):
    def test_thresholds(self):
        self.assertEqual(catalog.tier_of(None), "unscored")
        self.assertEqual(catalog.tier_of(0), "low")
        self.assertEqual(catalog.tier_of(499), "low")
        self.assertEqual(catalog.tier_of(500), "mid")
        self.assertEqual(catalog.tier_of(4999), "mid")
        self.assertEqual(catalog.tier_of(5000), "high")


class RegexTests(unittest.TestCase):
    def test_part_classifiers(self):
        for good in ("p16f877a", "p18lf4550", "p10f200", "p12hv615"):
            self.assertTrue(catalog.F_PART.match(good), good)
        self.assertFalse(catalog.F_PART.match("p24fj64"))
        for good in ("PIC16F877A", "PIC16LF1786", "PIC12HV615", "PIC18F4550"):
            self.assertTrue(catalog.PIC8_PART.match(good), good)
        for bad in ("PIC16C54", "PIC24FJ64", "dsPIC33"):
            self.assertFalse(catalog.PIC8_PART.match(bad), bad)
        self.assertTrue(catalog.PIC8_PACK.match("Microchip.PIC18F_DFP"))
        for bad in ("Microchip.PIC32_DFP", "Atmel.AVR_DFP"):
            self.assertFalse(catalog.PIC8_PACK.match(bad), bad)

    def test_display_name(self):
        self.assertEqual(catalog.display_name("p16f877a"), "PIC16F877A")


class ParseIniTests(unittest.TestCase):
    def test_sections_uppercased_comments_skipped(self):
        with tempfile.TemporaryDirectory() as tmp:
            ini = pathlib.Path(tmp) / "x.ini"
            ini.write_text(
                "; semi\n# hash\n[16F877A]\nROMSIZE=2000\nARCH=PIC14\n"
                "ROMSIZE=9999\n[16f84a]\nARCH = PIC14\n"
            )
            sections = catalog.parse_ini_facts(ini)
        self.assertEqual(sections["16F877A"]["ROMSIZE"], "2000")
        self.assertEqual(sections["16F877A"]["ARCH"], "PIC14")
        self.assertEqual(sections["16F84A"]["ARCH"], "PIC14")


class SelectPacksTests(unittest.TestCase):
    def test_latest_version_wins_non8bit_excluded(self):
        with tempfile.TemporaryDirectory() as tmp:
            idx = pathlib.Path(tmp) / "index.idx"
            idx.write_text(
                "<index>"
                '<pdsc name="Microchip.PIC16F1xxxx_DFP.pdsc" version="1.2.3"/>'
                '<pdsc name="Microchip.PIC16F1xxxx_DFP.pdsc" version="1.2.4"/>'
                '<pdsc name="Microchip.PIC32CX_DFP.pdsc" version="2.0.0"/>'
                '<pdsc name="Atmel.AVR_DFP.pdsc" version="1.0.0"/>'
                "</index>"
            )
            self.assertEqual(
                catalog.select_packs(idx), {"Microchip.PIC16F1xxxx_DFP": "1.2.4"}
            )


class BuildMergeTests(unittest.TestCase):
    def test_flash_from_dfp_ram_eeprom_from_gputils(self):
        rc, toml, err = build(
            [gp("PIC16F877A", flash_words=4096, ram_bytes=512, eeprom_bytes=256)],
            [
                dfp(
                    "PIC16F877A",
                    flash_words=8192,
                    ram_bytes=256,
                    eeprom_bytes=128,
                )
            ],
        )
        self.assertEqual(rc, 0)
        self.assertIn("flash_words = 8192", toml)
        self.assertIn("ram_bytes = 512", toml)
        self.assertIn("eeprom_bytes = 256", toml)
        self.assertIn("flash mismatch PIC16F877A", err)
        self.assertIn("ram mismatch PIC16F877A", err)
        self.assertIn("eeprom mismatch PIC16F877A", err)

    def test_dfp_core_wins_agreement_is_quiet(self):
        rc, toml, err = build(
            [gp("PIC16F1786", "pic14", flash_words=8192)],
            [dfp("PIC16F1786", core="pic14e", flash_words=8192)],
        )
        self.assertEqual(rc, 0)
        self.assertIn('core = "pic14e"', toml)
        self.assertNotIn("mismatch", err)

    def test_family_prefix_core_fallback(self):
        rc, toml, _ = build(
            [gp("PIC10F200", None), gp("PIC18F4550", None), gp("PIC16F84A", None)],
            [],
        )
        self.assertEqual(rc, 0)
        for display, core in (
            ("PIC10F200", "pic-baseline"),
            ("PIC18F4550", "pic18"),
            ("PIC16F84A", "pic14"),
        ):
            self.assertIn(f'display = "{display}"', toml)
            self.assertIn(f'core = "{core}"', toml)

    def test_scores_tiers_and_mcp_enrichment(self):
        rc, toml, _ = build(
            [gp("PIC16F877A", flash_words=8192)],
            [dfp("PIC16F877A", flash_words=8192)],
            scores={"PIC16F877A": 6000},
            mcp={"PIC16F877A": {"lifecycle": "Active", "pdf": "http://x/y.pdf"}},
        )
        self.assertEqual(rc, 0)
        self.assertIn("gh_refs = 6000", toml)
        self.assertIn('tier = "high"', toml)
        self.assertIn('lifecycle = "Active"', toml)
        self.assertIn('pdf = "http://x/y.pdf"', toml)
        self.assertIn('tier = "unscored"', build([gp("PIC16F84A")], [])[1])


if __name__ == "__main__":
    unittest.main()
