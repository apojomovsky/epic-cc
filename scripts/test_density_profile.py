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
    cfg.setdefault("family", family)
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

    def test_a_partly_readable_match_is_refused(self):
        # The dangerous case is not a body that parses as nothing, it is one
        # that parses as almost everything: a dropped MOVFF arm would halve
        # every struct copy in the profile and rerank it, silently.
        trailing_comment = (
            "fn instruction_words_pic18(line: &str) -> usize {\n"
            "    match mne {\n"
            '        "GOTO" | "CALL" => 2,\n'
            '        "MOVFF" | "LFSR" => 2, // two-word moves\n'
            "        _ => 1,\n"
            "    }\n"
            "}\n"
        )
        guard_arm = (
            "fn instruction_words_pic18(line: &str) -> usize {\n"
            "    match mne {\n"
            "        m if TWO_WORD.contains(&m) => 2,\n"
            '        "GOTO" => 2,\n'
            "        _ => 1,\n"
            "    }\n"
            "}\n"
        )
        for name, body in (
            ("trailing comment", trailing_comment),
            ("guard", guard_arm),
        ):
            with self.subTest(name):
                with self.assertRaises(dp.ProfileError):
                    dp.pic18_word_table(body)

    def test_pic14_table_is_one_word(self):
        table = dp.word_table("pic14")
        self.assertEqual(table.words("MOVLW"), 1)
        self.assertEqual(table.words("GOTO"), 1)

    def test_pic14_premise_rejects_a_size_lookup(self):
        with self.assertRaises(dp.ProfileError):
            dp.pic14_word_table(
                "fn assemble_first_pass() { org += instruction_words_x(line); }"
            )

    def test_pic14_premise_rejects_a_multi_word_step(self):
        with self.assertRaises(dp.ProfileError):
            dp.pic14_word_table("fn assemble_first_pass() { org += 1; org += 2; }")

    def test_family_detection(self):
        self.assertEqual(dp.detect_family(PIC18_HEADER), "pic18")
        self.assertEqual(dp.detect_family(PIC14_HEADER), "pic14")
        # `--target` takes all three spellings, so all three must land here.
        for spelling in ("18F4550", "p18f4550", "PIC18F4550"):
            self.assertEqual(dp.detect_family("", spelling), "pic18", spelling)
        for spelling in ("16F877A", "p16f877a", "PIC16F877A"):
            self.assertEqual(dp.detect_family("", spelling), "pic14", spelling)
        with self.assertRaises(dp.ProfileError):
            dp.detect_family("")
        with self.assertRaises(dp.ProfileError):
            dp.detect_family("", "not-a-part")


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

    def test_an_odd_db_run_rounds_the_program_up(self):
        # The assembler sizes its output `(bytes + 1) / 2`, so a trailing
        # half word still occupies a whole one. Flooring here would under-
        # report the program and inflate every percentage.
        listing = PIC18_HEADER + "tbl:\n    db 0x01, 0x02, 0x03\n"
        _, summary = profile(listing)
        self.assertEqual(summary["categories"][dp.CAT_DATA], 1.5)
        self.assertEqual(summary["total_words"], 2)

    def test_db_on_pic14_is_an_error_not_a_guess(self):
        with self.assertRaises(dp.ProfileError):
            profile(PIC14_HEADER + "tbl:\n    db 0x01, 0x02\n")

    def test_an_unknown_directive_is_an_error_not_zero_words(self):
        # A new dot-directive silently worth zero words is the same drift
        # the derived word table exists to prevent.
        with self.assertRaises(dp.ProfileError):
            profile(PIC18_HEADER + "f:\n    .newthing 4\n    RETURN\n")
        with self.assertRaises(dp.ProfileError):
            profile(PIC14_HEADER + "f:\n    .pcltbl t 4\n    RETURN\n")

    def test_pic14_align_padding_is_attributed(self):
        listing = PIC14_HEADER + (
            "top:\n    RETURN\n    .align 256\n    .table T 2\nT:\n    RETLW 0x00\n"
        )
        _, summary = profile(listing)
        self.assertEqual(summary["categories"][dp.CAT_PAD], 255)
        self.assertEqual(summary["categories"][dp.CAT_DATA], 1)

    def test_a_const_table_region_ends_at_the_next_function(self):
        # Code emitted after a table is code, not more table: a region
        # that never closes would swallow the rest of the program.
        listing = PIC14_HEADER + (
            "    .table T 2\nT:\n    RETLW 0x00\n    RETLW 0x01\n"
            "after:\n    BSF STATUS, 5\n    RETURN\n"
        )
        _, summary = profile(listing)
        self.assertEqual(summary["categories"][dp.CAT_DATA], 2)
        self.assertEqual(summary["functions"]["after"], 2)
        self.assertEqual(summary["categories"]["bank-switch"], 1)

    def test_a_sibling_block_label_opens_a_new_function(self):
        # `<fn>_L<block>` continues only its own function; the same shape
        # naming a different one is a real boundary.
        listing = PIC18_HEADER + (
            "alpha:\n    NOP\nalpha_L3:\n    NOP\nbeta_L3:\n    NOP\n"
        )
        _, summary = profile(listing)
        self.assertEqual(summary["functions"]["alpha"], 2)
        self.assertEqual(summary["functions"]["beta_L3"], 1)


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
        # The arms are `tmp<n>` labels, as the backend emits them: a
        # diamond split by two ordinary labels would be two functions.
        listing = PIC18_HEADER + (
            "f:\n    BRA tmp1\n    MOVLW 0x00\n    BRA tmp2\n"
            "tmp1:\n    MOVLW 0x01\ntmp2:\n    MOVWF 0x010,B\n    RETURN\n"
        )
        self.assertEqual(words_by_category(listing)["bool-materialization"], 4)

    def test_bool_materialization_needs_zero_and_one(self):
        listing = PIC18_HEADER + (
            "f:\n    BRA tmp1\n    MOVLW 0x05\n    BRA tmp2\n"
            "tmp1:\n    MOVLW 0x07\ntmp2:\n    MOVWF 0x010,B\n    RETURN\n"
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

    def test_a_lane_with_an_f_destination_does_not_swallow_the_next_store(self):
        # `ADDWF f,F` leaves no result in W, so the `MOVFF` after it belongs
        # to something else. Counting it inflated the second-ranked category.
        listing = PIC18_HEADER + (
            "f:\n    MOVLW 0x10\n    ADDWF 0x0E9,F,A\n"
            "    MOVLW 0x00\n    ADDWFC 0x0EA,F,A\n"
            "    MOVFF 0xFEE, 0x223\n    RETURN\n"
        )
        cats = words_by_category(listing)
        self.assertEqual(cats["wide-literal-arith"], 4)
        self.assertEqual(cats[dp.CAT_OTHER], 3)

    def test_a_run_does_not_cross_a_function_boundary(self):
        # Five one-instruction trap stubs are not a dispatch table.
        listing = PIC14_HEADER + "".join(
            f"stub{n}:\n    GOTO stub{n}\n" for n in range(5)
        )
        self.assertNotIn("switch-jump-table", words_by_category(listing))

    def test_an_sfr_movff_run_is_a_context_save_not_a_struct_copy(self):
        # The interrupt stub shuttles STATUS/BSR/FSR around its dispatch
        # call; the fix for that is a narrower live set, not a copy loop.
        listing = PIC18_HEADER + (
            "PIC18_IRQ_Handler:\n    MOVFF 0xFF6, 0x005\n    MOVFF 0xFF7, 0x006\n"
            "    CALL dispatch\n    RETFIE\n"
            "g:\n    MOVFF 0x001, 0x010\n    MOVFF 0x002, 0x011\n    RETURN\n"
        )
        cats = words_by_category(listing)
        self.assertEqual(cats["sfr-context-save"], 4)
        self.assertEqual(cats["struct-copy-movff"], 4)

    def test_pic14_status_bits_are_not_read_on_pic18(self):
        # 0x003 is STATUS on PIC14 and an ordinary ISR save slot on PIC18.
        listing = PIC18_HEADER + (
            "f:\n    BSF 0x003,5,A\n    BCF 0x003,0,A\n    RRCF 0x010,F,B\n"
            "    RRCF 0x011,F,B\n    RRCF 0x012,F,B\n    RETURN\n"
        )
        cats = words_by_category(listing)
        self.assertNotIn("bank-switch", cats)
        self.assertEqual(cats["shift-chain"], 3)
        self.assertEqual(cats[dp.CAT_OTHER], 3)

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

    def test_compare_reports_a_signed_delta_per_cell(self):
        # The point of `--compare` is showing what a fix moved, so a
        # run that always reported zero would be worse than useless.
        shrunk = PIC18_HEADER + "f:\n    MOVFF 0x001, 0x010\n    RETURN\n"
        with tempfile.TemporaryDirectory() as d:
            before = pathlib.Path(d) / "before.json"
            after = pathlib.Path(d) / "after.asm"
            before.write_text(self.run_cli("--json").stdout)
            after.write_text(shrunk)
            r = subprocess.run(
                [sys.executable, str(SCRIPT), str(after), "--compare", str(before)],
                capture_output=True,
                text=True,
            )
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("struct-copy-movff", r.stdout)
        self.assertIn("-4", r.stdout)
        self.assertIn("total 5 -> 3 words (-2)", r.stdout)

    def test_flash_words_shows_the_residual_as_its_own_row(self):
        r = self.run_cli("--flash-words", "9")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("9 flash words reported, 4 not in the listing", r.stdout)
        self.assertIn(dp.CAT_RESIDUAL, r.stdout)

    def test_device_is_the_fallback_when_the_listing_has_no_list_line(self):
        with tempfile.TemporaryDirectory() as d:
            path = pathlib.Path(d) / "bare.asm"
            path.write_text("f:\n    MOVFF 0x001, 0x010\n    RETURN\n")
            bare = [sys.executable, str(SCRIPT), str(path)]
            no_device = subprocess.run(bare, capture_output=True, text=True)
            with_device = subprocess.run(
                bare + ["--device", "PIC18F4550"], capture_output=True, text=True
            )
        self.assertEqual(no_device.returncode, 2)
        self.assertIn("--device", no_device.stderr)
        self.assertEqual(with_device.returncode, 0, with_device.stderr)
        self.assertIn("3 flash words", with_device.stdout)

    def test_compile_mode_drives_the_driver(self):
        # A stub stands in for epic-cc: the contract under test is the
        # argv this builds and that it profiles what the driver wrote.
        with tempfile.TemporaryDirectory() as d:
            stub = pathlib.Path(d) / "fake-epic-cc"
            listing = pathlib.Path(d) / "out.txt"
            stub.write_text(
                "#!/bin/sh\n"
                'while [ "$1" != "-o" ]; do shift; done\n'
                f'printf %s "$*" > {listing}\n'
                f'cat <<EOF > "$2"\n{self.LISTING}EOF\n'
            )
            stub.chmod(0o755)
            r = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--compile",
                    "--epic-cc",
                    str(stub),
                    "--device",
                    "18F4550",
                    "--cflag=-Iinc",
                    "a.c",
                ],
                capture_output=True,
                text=True,
            )
            forwarded = listing.read_text()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("struct-copy-movff", r.stdout)
        self.assertIn("-Iinc", forwarded)
        self.assertIn("a.c", forwarded)

    def test_compile_without_a_device_is_a_usage_error(self):
        r = subprocess.run(
            [sys.executable, str(SCRIPT), "--compile", "a.c"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("--device", r.stderr)

    def test_a_missing_file_exits_nonzero_with_a_message(self):
        r = subprocess.run(
            [sys.executable, str(SCRIPT), "/nonexistent.asm"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 2)
        self.assertIn("density-profile:", r.stderr)

    def test_a_malformed_compare_file_exits_like_every_other_failure(self):
        # A wrapper script branches on the exit code, so a raw traceback
        # and exit 1 here would not be distinguishable from a crash.
        with tempfile.TemporaryDirectory() as d:
            junk = pathlib.Path(d) / "junk.json"
            junk.write_text("not json")
            r = self.run_cli("--compare", str(junk))
            missing = self.run_cli("--compare", str(pathlib.Path(d) / "nope.json"))
        self.assertEqual(r.returncode, 2)
        self.assertIn("density-profile:", r.stderr)
        self.assertEqual(missing.returncode, 2)
        self.assertIn("density-profile:", missing.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
