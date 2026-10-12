#!/usr/bin/env python3
"""Behavior tests for baseline_delta.judge: the epic-tasks#306 rules."""

import importlib.util
import pathlib
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parent / "baseline_delta.py"
_spec = importlib.util.spec_from_file_location("baseline_delta", SCRIPT)
bd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bd)


def size_row(name, flash, ram=10, profile="Os"):
    return {
        "name": name,
        "device": "p16f877a",
        "profile": profile,
        "flash_words": flash,
        "ram_bytes": ram,
    }


def cycle_row(name, cycles, flash=100, ram=10, profile="Os"):
    return {
        "name": name,
        "device": "p16f877a",
        "profile": profile,
        "cycles": cycles,
        "flash_words": flash,
        "ram_bytes": ram,
    }


def run(base_size=(), head_size=(), base_cycle=(), head_cycle=(), body=""):
    base = {"size": list(base_size), "cycle": list(base_cycle)}
    head = {"size": list(head_size), "cycle": list(head_cycle)}
    return bd.judge(base, head, body)


class JudgeTests(unittest.TestCase):
    def test_unchanged_tree_passes(self):
        r = run([size_row("add", 50)], [size_row("add", 50)])
        self.assertEqual(r["verdict"], "pass")
        self.assertEqual(r["rows"], [])

    def test_shrink_only_passes_without_explanation(self):
        r = run([size_row("add", 50)], [size_row("add", 40)])
        self.assertEqual(r["verdict"], "pass")
        self.assertTrue(r["net_win"])

    def test_flash_regression_within_eight_words_needs_marker(self):
        base = [size_row("add", 50), size_row("mul", 100)]
        head = [size_row("add", 58), size_row("mul", 80)]
        self.assertEqual(run(base, head)["verdict"], "hold")
        self.assertEqual(run(base, head, body="delta-ok: add")["verdict"], "pass")

    def test_flash_regression_over_eight_words_holds_even_if_marked(self):
        base = [size_row("add", 50), size_row("mul", 100)]
        head = [size_row("add", 59), size_row("mul", 40)]
        self.assertEqual(run(base, head, body="delta-ok: add")["verdict"], "hold")

    def test_ram_growth_holds_even_if_marked(self):
        r = run(
            [size_row("add", 50, ram=10)],
            [size_row("add", 40, ram=11)],
            body="delta-ok: add",
        )
        self.assertEqual(r["verdict"], "hold")

    def test_cycle_regression_at_one_percent_passes_and_above_holds(self):
        base = [cycle_row("k", 1000), cycle_row("j", 500)]
        at_limit = [cycle_row("k", 1010), cycle_row("j", 400)]
        over = [cycle_row("k", 1011), cycle_row("j", 400)]
        self.assertEqual(
            run(base_cycle=base, head_cycle=at_limit, body="delta-ok: k")["verdict"],
            "pass",
        )
        self.assertEqual(
            run(base_cycle=base, head_cycle=over, body="delta-ok: k")["verdict"], "hold"
        )

    def test_net_loss_holds_even_when_every_row_is_within_limits(self):
        base = [size_row("add", 50), size_row("mul", 100)]
        head = [size_row("add", 58), size_row("mul", 101)]
        r = run(base, head, body="delta-ok: add\ndelta-ok: mul")
        self.assertEqual(r["verdict"], "hold")
        self.assertFalse(r["net_win"])

    def test_new_or_removed_row_holds(self):
        self.assertEqual(
            run([size_row("add", 50)], [size_row("add", 50), size_row("mul", 9)])[
                "verdict"
            ],
            "hold",
        )
        self.assertEqual(
            run([size_row("add", 50), size_row("mul", 9)], [size_row("add", 50)])[
                "verdict"
            ],
            "hold",
        )

    def test_o2_row_is_keyed_apart_from_os_row(self):
        base = [size_row("add", 50), size_row("add", 40, profile="O2")]
        head = [size_row("add", 50), size_row("add", 38, profile="O2")]
        r = run(base, head, body="delta-ok: add@O2")
        self.assertEqual(r["verdict"], "pass")
        self.assertEqual([row["key"] for row in r["rows"]], ["add@O2"])

    def test_marker_must_be_a_whole_line_for_the_exact_key(self):
        self.assertFalse(bd.names_row("the address table grows", "add"))
        self.assertFalse(bd.names_row("add grows by 2", "add"))
        self.assertFalse(bd.names_row("delta-ok: add@O2", "add"))
        self.assertTrue(bd.names_row("delta-ok: add", "add"))

    def test_isr_latency_and_roundtrip_count_one_binary(self):
        # Both rows come from one ISR binary: its flash shrink counts once, so
        # a second binary's growth of 1 word leaves a net of 0, not a win.
        base = [
            cycle_row("speed-isr-a-latency", 80, flash=100),
            cycle_row("speed-isr-a-roundtrip", 21, flash=100),
            cycle_row("k", 50, flash=100),
        ]
        head = [
            cycle_row("speed-isr-a-latency", 80, flash=99),
            cycle_row("speed-isr-a-roundtrip", 21, flash=99),
            cycle_row("k", 50, flash=101),
        ]
        r = run(base_cycle=base, head_cycle=head, body="delta-ok: k")
        self.assertEqual(r["totals"]["flash_words"], 0)
        self.assertFalse(r["net_win"])
        self.assertEqual(r["verdict"], "hold")

    def test_marker_does_not_match_across_a_line_break(self):
        self.assertFalse(bd.names_row("delta-ok:\nadd", "add"))

    def test_marker_accepts_crlf_line_endings(self):
        self.assertTrue(bd.names_row("delta-ok: add\r\n", "add"))

    def test_duplicate_ladder_row_is_an_error(self):
        with self.assertRaises(ValueError):
            run([size_row("add", 50), size_row("add", 40)], [size_row("add", 40)])


if __name__ == "__main__":
    unittest.main()
