"""Coverage for the size probes' normalization and acceptance counts.

epic-cc#827. Each probe counts a checked-in listing excerpt whose
count is the ticket number: 45 pairs (#674), 97 stores plus 104
loads (#738), 7 idiom sites (#767), 17 CLRF runs (#789). A probe
that cannot reproduce its number fails here, not in a triage
comment.
"""

import importlib.util
import json
import pathlib
import subprocess
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "size-probe.py"
FIXDIR = ROOT / "scripts" / "fixtures" / "size-probe"

# Filename has a hyphen, so `import` cannot name it.
_spec = importlib.util.spec_from_file_location("size_probe", SCRIPT)
sp = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sp)


def items(name):
    return sp.parse_listing((FIXDIR / name).read_text())


class ListingParseTest(unittest.TestCase):
    def test_label_sharing_a_directive_prefix_is_not_swallowed(self):
        text = "listen:\n    MOVWF 0x010,A\n    MOVFF 0x010, 0xFCA\n"
        found = sp.w_sfr_pairs(sp.parse_listing(text))
        self.assertEqual(len(found), 1)

    def test_pair_across_such_a_label_is_not_same_function(self):
        text = "f:\n    MOVWF 0x010,A\nlisten:\n    MOVFF 0x010, 0xFCA\n"
        self.assertEqual(sp.w_sfr_pairs(sp.parse_listing(text)), [])


class NormalizationTest(unittest.TestCase):
    def test_access_suffix_stripped_case_insensitive(self):
        self.assertEqual(sp.normalize_operands("0x010,A"), "0x010")
        self.assertEqual(sp.normalize_operands("0x120,b"), "0x120")
        self.assertEqual(sp.normalize_operands("0xFD8,0,A"), "0xFD8,0")

    def test_destination_bits_kept(self):
        self.assertEqual(sp.normalize_operands("0x010,W,A"), "0x010,W")
        self.assertEqual(sp.normalize_operands("0x010,F"), "0x010,F")


class SfrPairsTest(unittest.TestCase):
    def test_acceptance_count(self):
        counts, _ = sp.probe_summary("w-sfr-pairs", items("674-sfr-pairs.asm"))
        self.assertEqual(counts, {"pairs": 45})

    def test_sfr_segment_boundary(self):
        text = "f:\n    MOVWF 0x010,A\n    MOVFF 0x010, 0xF80\n"
        self.assertEqual(len(sp.w_sfr_pairs(sp.parse_listing(text))), 1)
        text = "f:\n    MOVWF 0x010,A\n    MOVFF 0x010, 0xF7F\n"
        self.assertEqual(sp.w_sfr_pairs(sp.parse_listing(text)), [])

    def test_mismatched_slot_and_label_break_are_not_pairs(self):
        text = (
            "f:\n    MOVWF 0x040,A\n    MOVFF 0x041, 0xFCA\n"
            "    MOVWF 0x053,A\ng:\n    MOVFF 0x053, 0xFCA\n"
        )
        self.assertEqual(sp.w_sfr_pairs(sp.parse_listing(text)), [])


class RetvalMovesTest(unittest.TestCase):
    def test_acceptance_count(self):
        counts, _ = sp.probe_summary("retval-moves", items("738-retval-moves.asm"))
        self.assertEqual(counts, {"stores": 97, "loads": 104})

    def test_save_area_past_retval_is_out(self):
        text = "f:\n    MOVWF 0x004,A\n    MOVF 0x008,W,A\n    MOVFF 0x00C, 0x010\n"
        self.assertEqual(sp.retval_moves(sp.parse_listing(text)), ([], []))

    def test_symbols_are_not_addresses(self):
        text = "f:\n    MOVWF retval_lo,A\n    MOVFF retval_lo, 0x010\n"
        self.assertEqual(sp.retval_moves(sp.parse_listing(text)), ([], []))


class IncCarryTest(unittest.TestCase):
    def test_acceptance_count(self):
        counts, _ = sp.probe_summary("inc-carry", items("767-inc-carry.asm"))
        self.assertEqual(counts, {"sites": 7})

    def test_plus_two_and_bank_select_break_are_not_sites(self):
        text = (
            "f:\n    MOVF 0x090,W,A\n    ADDLW 2\n    MOVWF 0x090,A\n"
            "    MOVF 0x091,W,A\n    BTFSC STATUS,C,A\n    ADDLW 1\n"
            "    MOVWF 0x091,A\n"
        )
        self.assertEqual(sp.inc_carry_sites(sp.parse_listing(text)), [])


class ClrfRunsTest(unittest.TestCase):
    def test_acceptance_count(self):
        counts, _ = sp.probe_summary("clrf-runs", items("789-clrf-runs.asm"))
        self.assertEqual(counts["runs"], 17)
        self.assertEqual(counts["words"], 86)

    def test_isolated_clrf_is_not_a_run(self):
        text = "f:\n    CLRF 0x100,A\n    MOVWF 0x100,A\n    CLRF 0x101,A\n"
        self.assertEqual(sp.clrf_runs(sp.parse_listing(text)), [])


class CliTest(unittest.TestCase):
    def test_json_reports_the_acceptance_number(self):
        proc = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "w-sfr-pairs",
                "--json",
                str(FIXDIR / "674-sfr-pairs.asm"),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(json.loads(proc.stdout)["counts"], {"pairs": 45})


if __name__ == "__main__":
    unittest.main(verbosity=2)
