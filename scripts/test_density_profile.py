"""Coverage for the density profiler's word table, attribution and rules.

Every fixture here is a hand-written listing small enough to count by
hand, so a failure names the rule that broke rather than a shifted total.
No compiler build is involved: the profiler reads text.

Run by scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "density-profile.py"

# Filename has a hyphen, so `import` cannot name it.
_spec = importlib.util.spec_from_file_location("density_profile", SCRIPT)
dp = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(dp)


def profile(text, device=None, **cfg):
    family = dp.detect_family(text, device)
    table = dp.word_table(family)
    items, total = dp.parse_listing(text, table)
    dp._region_carry(items)
    dp.categorize(items, dp.Config(**cfg))
    return items, dp.summarize(items, total)


def words_by_category(text, **cfg):
    return profile(text, **cfg)[1]["categories"]


PIC18_HEADER = "    list p=p18f4550\n    radix hex\n"
PIC14_HEADER = "    list p=p16f877a\n    radix hex\n"


class WordTableTest(unittest.TestCase):
    def test_pic18_table_comes_from_the_asm_crate(self):
        table = dp.word_table("pic18")
        for two_word in ("GOTO", "CALL", "LFSR", "MOVFF"):
            self.assertEqual(table.words(two_word), 2, two_word)
        for one_word in ("MOVLW", "MOVWF", "ADDWF", "BRA", "RETURN"):
            self.assertEqual(table.words(one_word), 1, one_word)
        self.assertIn("instruction_words_pic18", table.source)

    def test_pic18_table_follows_the_assembler_rather_than_a_copy(self):
        # A new two-word mnemonic in the assembler must appear here with
        # no edit to this script: that is the whole point of deriving it.
        fake = (
            "fn instruction_words_pic18(line: &str) -> usize {\n"
            "    match mne {\n"
            '        "GOTO" | "CALL" | "WIDENEW" => 2,\n'
            "        _ => 1,\n"
            "    }\n"
            "}\n"
        )
        table = dp.pic18_word_table(fake)
        self.assertEqual(table.words("WIDENEW"), 2)
        self.assertEqual(table.words("MOVLW"), 1)

    def test_pic18_table_refuses_a_shape_it_cannot_read(self):
        fake = "fn instruction_words_pic18(line: &str) -> usize { lookup(line) }\n"
        with self.assertRaises(dp.ProfileError):
            dp.pic18_word_table(fake)

    def test_missing_function_is_an_error_not_a_fallback(self):
        with self.assertRaises(dp.ProfileError):
            dp.pic18_word_table("fn something_else() {}")

    def test_pic14_table_is_one_word_and_checks_its_premise(self):
        table = dp.word_table("pic14")
        self.assertEqual(table.words("MOVLW"), 1)
        self.assertEqual(table.words("GOTO"), 1)
        with self.assertRaises(dp.ProfileError):
            dp.pic14_word_table(
                "fn assemble_first_pass() { org += instruction_words_x(line); }"
            )

    def test_family_detection(self):
        self.assertEqual(dp.detect_family(PIC18_HEADER), "pic18")
        self.assertEqual(dp.detect_family(PIC14_HEADER), "pic14")
        self.assertEqual(dp.detect_family("", "18F4550"), "pic18")
        self.assertEqual(dp.detect_family("", "16F877A"), "pic14")
        with self.assertRaises(dp.ProfileError):
            dp.detect_family("")


class AttributionTest(unittest.TestCase):
    LISTING = PIC18_HEADER + (
        "    org 0x0000\n"
        "    goto __start\n"
        "    org 0x0008\n"
        "alpha:\n"
        "    MOVLW 0x01    ; a comment\n"
        "    CALL beta\n"
        "tmp7:\n"
        "    RETURN\n"
        "alpha_L3:\n"
        "    NOP\n"
        "__far_skip0:\n"
        "    NOP\n"
        "beta:\n"
        "    MOVFF 0x001, 0x002\n"
        "    RETURN\n"
        "    end\n"
        "gamma:\n"
        "    NOP\n"
    )

    def setUp(self):
        self.items, self.summary = profile(self.LISTING)

    def test_backend_labels_do_not_open_a_function(self):
        functions = self.summary["functions"]
        self.assertNotIn("tmp7", functions)
        self.assertNotIn("alpha_L3", functions)
        self.assertNotIn("__far_skip0", functions)

    def test_every_instruction_lands_on_its_function(self):
        # alpha: MOVLW 1 + CALL 2 + RETURN 1 + NOP 1 + NOP 1 = 6
        # beta: MOVFF 2 + RETURN 1 = 3
        self.assertEqual(self.summary["functions"]["alpha"], 6)
        self.assertEqual(self.summary["functions"]["beta"], 3)

    def test_end_stops_the_walk(self):
        self.assertNotIn("gamma", self.summary["functions"])

    def test_an_org_gap_is_reported_not_silently_dropped(self):
        # `goto __start` is 2 words at byte 0; org 0x0008 leaves 2 words.
        self.assertEqual(self.summary["categories"][dp.CAT_GAP], 2)

    def test_label_and_instruction_on_one_line(self):
        items, summary = profile(PIC18_HEADER + "delta: MOVLW 0x01\n    RETURN\n")
        self.assertEqual(summary["functions"]["delta"], 2)
        self.assertEqual(items[0].mnemonic, "MOVLW")

    def test_equ_and_markers_take_no_words(self):
        listing = PIC18_HEADER + (
            "INTCON equ 0xFF2\neps:\n    .pclalign\n    .pcltbl tmp1 64\n    RETURN\n"
        )
        _, summary = profile(listing)
        self.assertEqual(summary["functions"]["eps"], 1)

    def test_pic18_db_packs_two_bytes_per_word(self):
        listing = PIC18_HEADER + "tbl:\n    db 0x01, 0x02, 0x03, 0x04\n"
        _, summary = profile(listing)
        self.assertEqual(summary["categories"][dp.CAT_DATA], 2)

    def test_pic14_align_padding_is_attributed(self):
        listing = PIC14_HEADER + (
            "top:\n    RETURN\n    .align 256\n    .table T 2\nT:\n    RETLW 0x00\n"
        )
        _, summary = profile(listing)
        self.assertEqual(summary["categories"][dp.CAT_PAD], 255)
        self.assertEqual(summary["categories"][dp.CAT_DATA], 1)


class CategorizationTest(unittest.TestCase):
    def test_movff_run_is_a_struct_copy_and_a_lone_movff_is_not(self):
        run = PIC18_HEADER + (
            "f:\n    MOVFF 0x001, 0x010\n    MOVFF 0x002, 0x011\n    RETURN\n"
        )
        self.assertEqual(words_by_category(run)["struct-copy-movff"], 4)
        lone = PIC18_HEADER + "f:\n    MOVFF 0x001, 0x010\n    RETURN\n"
        self.assertNotIn("struct-copy-movff", words_by_category(lone))

    def test_movff_run_threshold_is_tunable(self):
        run = PIC18_HEADER + (
            "f:\n    MOVFF 0x001, 0x010\n    MOVFF 0x002, 0x011\n    RETURN\n"
        )
        self.assertNotIn("struct-copy-movff", words_by_category(run, movff_run=3))

    def test_zero_init_pair(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVLW 0x00\n    MOVWF 0x010,B\n    CLRF 0x011,B\n    RETURN\n"
        )
        cats = words_by_category(listing)
        self.assertEqual(cats["zero-init-pair"], 2)
        # CLRF is already the cheap form and must not be counted as a sink.
        self.assertEqual(cats[dp.CAT_OTHER], 2)

    def test_wide_const_materialization_needs_consecutive_slots(self):
        adjacent = PIC18_HEADER + (
            "f:\n    MOVLW 0x34\n    MOVWF 0x010,B\n"
            "    MOVLW 0x12\n    MOVWF 0x011,B\n    RETURN\n"
        )
        self.assertEqual(words_by_category(adjacent)["wide-const-materialization"], 4)
        apart = PIC18_HEADER + (
            "f:\n    MOVLW 0x34\n    MOVWF 0x010,B\n"
            "    MOVLW 0x12\n    MOVWF 0x020,B\n    RETURN\n"
        )
        self.assertNotIn("wide-const-materialization", words_by_category(apart))

    def test_a_symbol_address_pair_is_a_wide_const(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVLW LOW(cb)\n    MOVWF 0x0C6,B\n"
            "    MOVLW HIGH(cb)\n    MOVWF 0x0C7,B\n    RETURN\n"
        )
        self.assertEqual(words_by_category(listing)["wide-const-materialization"], 4)

    def test_an_all_zero_pair_stays_zero_init(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVLW 0x00\n    MOVWF 0x010,B\n"
            "    MOVLW 0x00\n    MOVWF 0x011,B\n    RETURN\n"
        )
        cats = words_by_category(listing)
        self.assertEqual(cats["zero-init-pair"], 4)
        self.assertNotIn("wide-const-materialization", cats)

    def test_bank_switches_on_both_families(self):
        pic18 = PIC18_HEADER + "f:\n    MOVLB 0x2\n    RETURN\n"
        self.assertEqual(words_by_category(pic18)["bank-switch"], 1)
        pic14 = PIC14_HEADER + (
            "f:\n    BSF STATUS, 5\n    BCF STATUS, 6\n    BCF STATUS, 0\n    RETURN\n"
        )
        cats = words_by_category(pic14)
        self.assertEqual(cats["bank-switch"], 2)
        # STATUS bit 0 is carry, not a bank select.
        self.assertEqual(cats[dp.CAT_OTHER], 2)

    def test_jump_table_run(self):
        listing = PIC18_HEADER + "f:\n" + "    GOTO t\n" * 4 + "t:\n    RETURN\n"
        self.assertEqual(words_by_category(listing)["switch-jump-table"], 8)
        short = PIC18_HEADER + "f:\n" + "    GOTO t\n" * 3 + "t:\n    RETURN\n"
        self.assertNotIn("switch-jump-table", words_by_category(short))

    def test_compare_chain_needs_several_units_on_one_value(self):
        unit = "    MOVLW 0x{:02X}\n    SUBWF 0x010,W,B\n    BZ t\n"
        chain = PIC18_HEADER + "f:\n" + "".join(unit.format(k) for k in (1, 2, 3))
        chain += "t:\n    RETURN\n"
        self.assertEqual(words_by_category(chain)["switch-compare-chain"], 9)
        two = PIC18_HEADER + "f:\n" + "".join(unit.format(k) for k in (1, 2))
        two += "t:\n    RETURN\n"
        self.assertNotIn("switch-compare-chain", words_by_category(two))

    def test_compare_chain_ignores_unrelated_registers(self):
        spread = PIC18_HEADER + "f:\n"
        for k, slot in ((1, 0x10), (2, 0x20), (3, 0x30)):
            spread += f"    MOVLW 0x{k:02X}\n    SUBWF 0x{slot:03X},W,B\n    BZ t\n"
        spread += "t:\n    RETURN\n"
        self.assertNotIn("switch-compare-chain", words_by_category(spread))

    def test_bool_materialization_diamond(self):
        listing = PIC18_HEADER + (
            "f:\n    BRA a\n    MOVLW 0x00\n    BRA b\n"
            "a:\n    MOVLW 0x01\nb:\n    MOVWF 0x010,B\n    RETURN\n"
        )
        self.assertEqual(words_by_category(listing)["bool-materialization"], 4)

    def test_bool_materialization_needs_zero_and_one(self):
        listing = PIC18_HEADER + (
            "f:\n    BRA a\n    MOVLW 0x05\n    BRA b\n"
            "a:\n    MOVLW 0x07\nb:\n    MOVWF 0x010,B\n    RETURN\n"
        )
        self.assertNotIn("bool-materialization", words_by_category(listing))

    def test_shift_chain(self):
        listing = PIC18_HEADER + (
            "f:\n    BCF 0xFD8,0,A\n    RRCF 0x010,F,B\n"
            "    BCF 0xFD8,0,A\n    RRCF 0x010,F,B\n    RETURN\n"
        )
        self.assertEqual(words_by_category(listing)["shift-chain"], 4)

    def test_shift_chain_ignores_a_bare_carry_clear(self):
        listing = PIC18_HEADER + "f:\n    BCF 0xFD8,0,A\n    RETURN\n"
        self.assertNotIn("shift-chain", words_by_category(listing))

    def test_dead_store_reload(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVWF 0x015,A\n    MOVFF 0x015, 0x012\n"
            "    MOVWF 0x016,A\n    MOVF 0x016, W, A\n    RETURN\n"
        )
        self.assertEqual(words_by_category(listing)["dead-store-reload"], 5)

    def test_a_reload_of_a_different_slot_is_not_a_roundtrip(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVWF 0x015,A\n    MOVF 0x016, W, A\n    RETURN\n"
        )
        self.assertNotIn("dead-store-reload", words_by_category(listing))

    def test_wide_literal_arith(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVLW 0x6C\n    ADDWF 0x010,W,B\n    MOVWF 0x020,B\n"
            "    MOVLW 0xDC\n    ADDWFC 0x011,W,B\n    MOVWF 0x021,B\n    RETURN\n"
        )
        self.assertEqual(words_by_category(listing)["wide-literal-arith"], 6)

    def test_every_word_is_attributed_exactly_once(self):
        listing = PIC18_HEADER + (
            "f:\n    MOVLB 0x2\n    MOVFF 0x001, 0x010\n    MOVFF 0x002, 0x011\n"
            "    MOVLW 0x00\n    MOVWF 0x012,B\n    GOTO g\n"
            "g:\n    RETURN\n"
        )
        items, summary = profile(listing)
        self.assertEqual(sum(summary["categories"].values()), summary["total_words"])
        self.assertTrue(all(i.category for i in items if i.kind == "instr"))


class CliTest(unittest.TestCase):
    LISTING = PIC18_HEADER + (
        "f:\n    MOVFF 0x001, 0x010\n    MOVFF 0x002, 0x011\n    RETURN\n"
    )

    def run_cli(self, *args):
        with tempfile.TemporaryDirectory() as d:
            path = pathlib.Path(d) / "case.asm"
            path.write_text(self.LISTING)
            return subprocess.run(
                [sys.executable, str(SCRIPT), str(path), *args],
                capture_output=True,
                text=True,
            )

    def test_table_output_ranks_the_sink(self):
        r = self.run_cli("--xc8-words", "3")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("struct-copy-movff", r.stdout)
        self.assertIn("1.67x", r.stdout)

    def test_json_round_trips_into_compare(self):
        r = self.run_cli("--json")
        self.assertEqual(r.returncode, 0, r.stderr)
        with tempfile.TemporaryDirectory() as d:
            before = pathlib.Path(d) / "before.json"
            before.write_text(r.stdout)
            r2 = self.run_cli("--compare", str(before))
        self.assertEqual(r2.returncode, 0, r2.stderr)
        self.assertIn("no change", r2.stdout)

    def test_a_missing_file_exits_nonzero_with_a_message(self):
        r = subprocess.run(
            [sys.executable, str(SCRIPT), "/nonexistent.asm"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("density-profile:", r.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
