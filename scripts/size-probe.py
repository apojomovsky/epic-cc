#!/usr/bin/env python3
"""size-probe.py -- reproducible counters for PIC18 density triage.

epic-cc#827. Density triage on raw listings kept producing ad-hoc
regex counts that later closed tickets falsely (#674, #738). These
probes replace the greps with one parsing rule each, so a recount
reproduces the ticket number exactly:

- w-sfr-pairs: adjacent MOVWF f / MOVFF f,SFR sharing the slot f,
  the #674 W-preserving store shape. Operands normalize first, so a
  `,A` access suffix cannot hide a pair from the match.
- retval-moves: data moves touching the ADR-013 retval region
  0x000-0x003, split into stores and loads, the #738 call-result
  traffic. Address range, never the `retval_lo` symbol.
- inc-carry: the 7-word 16-bit increment carry chain of #767.
- clrf-runs: maximal consecutive CLRF runs of length 2 or more,
  the #789 zero-fill shape. Singles are inline stores, not runs.

Usage: python3 scripts/size-probe.py <probe> <listing> [--json]
Probes only; density fixes live in the backend, never here.
"""

import argparse
import dataclasses
import json
import pathlib
import re
import sys

PIC18_SFR_BASE = 0xF80
RETVAL_LO = 0x000
RETVAL_HI = 0x003

_LABEL = re.compile(r"^(?P<label>[A-Za-z_.$][A-Za-z0-9_.$]*)\s*:\s*(?P<rest>.*)$")
_BACKEND_LABEL = re.compile(r"^(tmp\d+|__far_skip\d+)$")
_NUMBER = re.compile(r"^(0[xX][0-9a-fA-F]+|\d+)$")


@dataclasses.dataclass
class Instr:
    """One instruction line: its function, mnemonic, raw operands, source line."""

    function: str
    mnemonic: str
    operands: str
    line_no: int


def _is_internal_label(label, current):
    if _BACKEND_LABEL.match(label):
        return True
    return bool(current) and label.startswith(current + "_L")


def parse_listing(text):
    """Instruction lines of an `--emit asm` listing, comments stripped.

    Directives, `db` bytes and gap markers carry no probe shape, so
    only real instructions survive, attributed to the nearest
    preceding function label. A label change ends adjacency: probes
    never match across it.
    """
    items = []
    function = "<prologue>"
    for line_no, raw in enumerate(text.splitlines(), start=1):
        line = raw.split(";", 1)[0].strip()
        if not line:
            continue
        low = line.lower()
        # Word boundary: a label like `listen:` starts with a directive
        # name but is not one, so bare-prefix matching would eat it.
        if low in ("list", "radix") or low.startswith(("list ", "radix ")):
            continue
        if low == "end" or low.startswith("end "):
            break
        if " equ " in line and not line.endswith(":"):
            continue
        m = _LABEL.match(line)
        if m:
            if not _is_internal_label(m.group("label"), function):
                function = m.group("label")
            line = m.group("rest").strip()
            if not line:
                continue
            low = line.lower()
        if low.startswith(("org ", ".")) or low.startswith("db "):
            continue
        parts = line.split(None, 1)
        mnemonic = parts[0].upper()
        operands = parts[1].strip() if len(parts) > 1 else ""
        items.append(Instr(function, mnemonic, operands, line_no))
    return items


def normalize_operands(operands):
    """Operands with the `,A`/`,B` access suffix removed.

    The PIC18 backend spells the suffix on every direct access
    (`MOVWF 0x010,A`), while `MOVFF` carries full 12-bit addresses
    with none, so an unnormalized compare never sees the pair.
    `,W` and `,F` are destination bits and stay.
    """
    tokens = [t.strip() for t in operands.split(",")]
    if len(tokens) >= 2 and tokens[-1].upper() in ("A", "B"):
        tokens = tokens[:-1]
    return ",".join(tokens)


def tokens_of(item):
    """Normalized comma-separated operand tokens of one instruction."""
    if not item.operands:
        return []
    return [t.strip() for t in normalize_operands(item.operands).split(",")]


def parse_addr(token):
    """A numeric operand as an int, else None for symbols like INTCON."""
    token = token.strip()
    if not _NUMBER.match(token):
        return None
    return int(token, 0)


def w_sfr_pairs(items):
    """Adjacent MOVWF f + MOVFF f,SFR pairs sharing slot f. Returns sites."""
    sites = []
    for i in range(len(items) - 1):
        first, second = items[i], items[i + 1]
        if first.function != second.function:
            continue
        if first.mnemonic != "MOVWF" or second.mnemonic != "MOVFF":
            continue
        first_tokens = tokens_of(first)
        second_tokens = tokens_of(second)
        if len(first_tokens) < 1 or len(second_tokens) != 2:
            continue
        slot = parse_addr(first_tokens[0])
        src = parse_addr(second_tokens[0])
        dst = parse_addr(second_tokens[1])
        if slot is None or src is None or dst is None:
            continue
        if slot == src and dst >= PIC18_SFR_BASE:
            sites.append(first.line_no)
    return sites


_MOVE_WRITES = {"MOVWF", "CLRF", "SETF"}
_MOVE_READS = {"MOVF"}


def _write_addr(item):
    tokens = tokens_of(item)
    if item.mnemonic in _MOVE_WRITES:
        return parse_addr(tokens[0]) if tokens else None
    if item.mnemonic == "MOVFF":
        return parse_addr(tokens[1]) if len(tokens) == 2 else None
    return None


def _read_addr(item):
    tokens = tokens_of(item)
    if item.mnemonic in _MOVE_READS:
        return parse_addr(tokens[0]) if tokens else None
    if item.mnemonic == "MOVFF":
        return parse_addr(tokens[0]) if len(tokens) == 2 else None
    return None


def _in_retval(addr):
    return addr is not None and RETVAL_LO <= addr <= RETVAL_HI


def retval_moves(items):
    """Moves touching 0x000-0x003 as (stores, loads) site lists."""
    stores = [it.line_no for it in items if _in_retval(_write_addr(it))]
    loads = [it.line_no for it in items if _in_retval(_read_addr(it))]
    return stores, loads


def _is_carry_test(item):
    """A `BTFSC` on the STATUS carry bit, numeric or named form."""
    if item.mnemonic != "BTFSC":
        return False
    tokens = tokens_of(item)
    if len(tokens) != 2:
        return False
    file_ok = tokens[0].upper() == "STATUS" or parse_addr(tokens[0]) == 0xFD8
    bit_ok = tokens[1].upper() == "C" or parse_addr(tokens[1]) == 0
    return file_ok and bit_ok


def _is_add_one(item):
    if item.mnemonic != "ADDLW":
        return False
    return parse_addr(normalize_operands(item.operands)) == 1


def _is_movf_w(item, addr):
    if item.mnemonic != "MOVF":
        return False
    tokens = tokens_of(item)
    if len(tokens) != 2 or tokens[1].upper() != "W":
        return False
    return parse_addr(tokens[0]) == addr


def _is_movwf(item, addr):
    if item.mnemonic != "MOVWF":
        return False
    tokens = tokens_of(item)
    return bool(tokens) and parse_addr(tokens[0]) == addr


def inc_carry_sites(items):
    """16-bit increment carry chains. Returns starting line numbers."""
    sites = []
    for i in range(len(items) - 6):
        window = items[i : i + 7]
        if any(w.function != window[0].function for w in window):
            continue
        lo = parse_addr(tokens_of(window[0])[0]) if tokens_of(window[0]) else None
        hi = parse_addr(tokens_of(window[3])[0]) if tokens_of(window[3]) else None
        if lo is None or hi is None:
            continue
        if (
            _is_movf_w(window[0], lo)
            and _is_add_one(window[1])
            and _is_movwf(window[2], lo)
            and _is_movf_w(window[3], hi)
            and _is_carry_test(window[4])
            and _is_add_one(window[5])
            and _is_movwf(window[6], hi)
        ):
            sites.append(window[0].line_no)
    return sites


def clrf_runs(items):
    """Maximal same-function CLRF runs of length 2 or more, as line lists."""
    runs = []
    run = []

    def flush():
        if len(run) >= 2:
            runs.append([it.line_no for it in run])

    for item in items:
        if item.mnemonic != "CLRF":
            flush()
            run = []
            continue
        if run and item.function != run[0].function:
            flush()
            run = []
        run.append(item)
    flush()
    return runs


PROBES = ("w-sfr-pairs", "retval-moves", "inc-carry", "clrf-runs")


def probe_summary(probe, items):
    """Counts and sites for one probe, the shape `--json` also reports."""
    if probe == "w-sfr-pairs":
        sites = w_sfr_pairs(items)
        return {"pairs": len(sites)}, sites
    if probe == "retval-moves":
        stores, loads = retval_moves(items)
        return {"stores": len(stores), "loads": len(loads)}, stores + loads
    if probe == "inc-carry":
        sites = inc_carry_sites(items)
        return {"sites": len(sites)}, sites
    runs = clrf_runs(items)
    words = sum(len(run) for run in runs)
    return {"runs": len(runs), "words": words}, [ln for run in runs for ln in run]


def build_parser():
    parser = argparse.ArgumentParser(description="count density shapes in asm listings")
    parser.add_argument("probe", choices=PROBES)
    parser.add_argument("listing", type=pathlib.Path)
    parser.add_argument("--json", action="store_true")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        text = args.listing.read_text()
    except OSError as exc:
        print(f"size-probe: cannot read {args.listing}: {exc}", file=sys.stderr)
        return 2
    counts, sites = probe_summary(args.probe, parse_listing(text))
    if args.json:
        print(json.dumps({"probe": args.probe, "counts": counts, "sites": sites}))
    else:
        detail = " ".join(f"{name}={value}" for name, value in counts.items())
        print(f"{args.probe}: {detail} ({args.listing})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
