#!/usr/bin/env python3
"""
density-profile.py -- rank flash-word sinks in an `--emit asm` listing.

epic-cc#500. The two PIC18 density passes on epic-cc#469 were done by
hand: build with `--emit asm`, eyeball the listing, grep-count mnemonics
into a table, paste it into an issue comment. This is that method as one
command, so a density pass starts from the previous profile instead of
from zero.

  python3 scripts/density-profile.py out.asm --xc8-words 9068
  python3 scripts/density-profile.py out.asm --json > before.json
  python3 scripts/density-profile.py out.asm --compare before.json
  python3 scripts/density-profile.py --compile --device 18F4550 \
      --cflag -Iinclude -- src/*.c

Word counts are not hand-maintained here. The per-mnemonic table is read
out of `crates/asm/src/lib.rs`, the assembler that actually lays the
program out, so the profile cannot drift from the encoder: a new two-word
PIC18 mnemonic is picked up the moment the assembler learns it, and a
shape this parser no longer recognises is a hard error, never a silent
fallback to a stale table. The listing is the assembler's input, not its
output, so PIC18 totals sit a fraction under the driver's reported flash:
far-branch expansion and PCL alignment padding happen during assembly.
`--flash-words` shows that residual instead of leaving it implicit.

Attribution is by nearest preceding function label, the only structure
the emitted asm carries. Labels the backends generate inside a function
(`tmp<n>`, `<fn>_L<block>`, `__far_skip<n>`) do not open a new region.

Categorisation is a precedence-ordered list of pattern rules
(`SINK_RULES`), each consuming a window of instructions. Adding a
category is one entry plus one matcher.
"""

import argparse
import dataclasses
import json
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
ASM_SOURCE = ROOT / "crates" / "asm" / "src" / "lib.rs"


class ProfileError(RuntimeError):
    """A malformed input, or an asm-crate shape the word table cannot read."""


# --------------------------------------------------------------------------
# Word table, sourced from the asm crate
# --------------------------------------------------------------------------

_FN_PIC18 = "fn instruction_words_pic18"
_FN_PIC14 = "fn assemble_first_pass"

_MATCH_ARM = re.compile(
    r'^\s*(?P<pats>(?:"[A-Za-z0-9_]+"|_)(?:\s*\|\s*(?:"[A-Za-z0-9_]+"|_))*)'
    r"\s*=>\s*(?P<words>\d+)\s*,\s*$",
    re.M,
)


@dataclasses.dataclass(frozen=True)
class WordTable:
    """Words per instruction for one device family, plus its provenance."""

    family: str
    sizes: dict
    default: int
    source: str

    def words(self, mnemonic):
        return self.sizes.get(mnemonic.upper(), self.default)

    def describe(self):
        if not self.sizes:
            return f"{self.family}: every instruction 1 word ({self.source})"
        listed = ", ".join(
            f"{m}={w}" for m, w in sorted(self.sizes.items(), key=lambda kv: kv[0])
        )
        return f"{self.family}: default {self.default} word, {listed} ({self.source})"


def _fn_body(text, signature):
    """The brace-matched body of the first `fn` whose text starts `signature`."""
    start = text.find(signature)
    if start < 0:
        raise ProfileError(
            f"{ASM_SOURCE.name}: no `{signature}`; the word table is derived from "
            "it, so this script must be updated alongside the assembler"
        )
    open_brace = text.find("{", start)
    if open_brace < 0:
        raise ProfileError(f"{ASM_SOURCE.name}: `{signature}` has no body")
    depth = 0
    for i in range(open_brace, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace + 1 : i]
    raise ProfileError(f"{ASM_SOURCE.name}: unbalanced braces in `{signature}`")


def pic18_word_table(asm_source):
    """Read the PIC18 per-mnemonic word counts out of the assembler."""
    body = _fn_body(asm_source, _FN_PIC18)
    sizes = {}
    default = None
    for arm in _MATCH_ARM.finditer(body):
        words = int(arm.group("words"))
        for pat in arm.group("pats").split("|"):
            pat = pat.strip()
            if pat == "_":
                default = words
            else:
                sizes[pat.strip('"').upper()] = words
    if default is None or not sizes:
        raise ProfileError(
            f"{ASM_SOURCE.name}: `{_FN_PIC18}` no longer reads as a literal match "
            "over mnemonics; update the parser rather than hardcoding a table"
        )
    return WordTable("pic18", sizes, default, f"{ASM_SOURCE.name}:{_FN_PIC18}")


def pic14_word_table(asm_source):
    """Confirm PIC14 is still the single-word ISA this profile assumes."""
    body = _fn_body(asm_source, _FN_PIC14)
    if "instruction_words" in body or "org += 1;" not in body:
        raise ProfileError(
            f"{ASM_SOURCE.name}: `{_FN_PIC14}` no longer advances `org` by exactly "
            "one word per instruction; PIC14 needs a real word table now"
        )
    return WordTable("pic14", {}, 1, f"{ASM_SOURCE.name}:{_FN_PIC14}")


def word_table(family, asm_source=None):
    if asm_source is None:
        asm_source = ASM_SOURCE.read_text()
    if family == "pic18":
        return pic18_word_table(asm_source)
    return pic14_word_table(asm_source)


# --------------------------------------------------------------------------
# Parsing and attribution
# --------------------------------------------------------------------------

_LIST_DIRECTIVE = re.compile(r"\blist\s+p\s*=\s*p?(?P<part>[0-9a-z]+)", re.I)
_LABEL = re.compile(r"^(?P<label>[A-Za-z_.$][A-Za-z0-9_.$]*)\s*:\s*(?P<rest>.*)$")
_BACKEND_LABEL = re.compile(r"^(tmp\d+|__far_skip\d+)$")
_NUMBER = re.compile(r"^(0[xX][0-9a-fA-F]+|\d+)$")

# Categories a region's kind assigns directly, before any pattern rule.
CAT_DATA = "const-data"
CAT_PAD = "align-padding"
CAT_GAP = "reserved-gap"
CAT_OTHER = "other"


@dataclasses.dataclass
class Item:
    """One flash-word consumer: an instruction, a data byte run, or padding."""

    kind: str
    mnemonic: str
    operands: str
    words: int
    function: str
    line_no: int
    category: str = ""


def detect_family(text, device=None):
    """pic14 or pic18, from the listing's own `list p=` line or `--device`."""
    part = None
    m = _LIST_DIRECTIVE.search(text)
    if m:
        part = m.group("part")
    elif device:
        part = device
    if part is None:
        raise ProfileError(
            "cannot tell the device family: the listing has no `list p=` line, "
            "pass --device"
        )
    return "pic18" if part.lower().lstrip("p").startswith("18") else "pic14"


def _parse_int(token):
    token = token.strip()
    if not _NUMBER.match(token):
        return None
    return int(token, 0)


def _is_internal_label(label, current):
    """True when `label` continues the current function rather than opening one."""
    if _BACKEND_LABEL.match(label):
        return True
    return bool(current) and label.startswith(current + "_L")


def parse_listing(text, table):
    """Walk an `--emit asm` listing into flash-word items, attributed by label.

    Mirrors the assembler's own pass 1: comments and `list`/`radix` are
    dropped, `end` stops the walk, `org`/`equ`/labels take no words, `db`
    is one byte per value on PIC18, `.align` pads to a word boundary on
    PIC14, and every other line is an instruction sized by `table`.
    Addresses are the assembler's native unit (PIC14 words, PIC18 bytes),
    so `.align` and `org` gaps land at the right size.
    """
    unit = 2 if table.family == "pic18" else 1  # address units per word
    items = []
    org = 0
    high_water = 0
    function = "<prologue>"
    pending_region = None

    for line_no, raw in enumerate(text.splitlines(), start=1):
        line = raw.split(";", 1)[0].strip()
        if not line:
            continue
        low = line.lower()
        if low.startswith(("list", "radix")):
            continue
        if low == "end" or low.startswith("end "):
            break
        if " equ " in line and not line.endswith(":"):
            continue

        m = _LABEL.match(line)
        if m:
            label = m.group("label")
            if not _is_internal_label(label, function):
                function = label
                if pending_region is not None:
                    items.append(
                        Item("region", "", "", 0, function, line_no, pending_region)
                    )
            pending_region = None
            line = m.group("rest").strip()
            if not line:
                continue
            low = line.lower()

        if low.startswith("org "):
            target = _parse_int(line[4:])
            if target is None:
                raise ProfileError(f"line {line_no}: unreadable `org` target: {line}")
            if target > org:
                gap = (target - org) // unit
                if gap:
                    items.append(Item("gap", "", "", gap, function, line_no, CAT_GAP))
            org = target
            high_water = max(high_water, org)
            continue
        if low.startswith(".align "):
            n = _parse_int(line[len(".align ") :])
            if n is None:
                raise ProfileError(f"line {line_no}: unreadable `.align`: {line}")
            padded = (org + n - 1) & ~(n - 1)
            if padded > org:
                items.append(
                    Item(
                        "pad",
                        "",
                        "",
                        (padded - org) // unit,
                        function,
                        line_no,
                        CAT_PAD,
                    )
                )
            org = padded
            high_water = max(high_water, org)
            continue
        if low.startswith(".table "):
            pending_region = CAT_DATA
            continue
        if low.startswith("."):
            # `.pcltbl`/`.pclalign`: dispatch markers, no words of their own.
            continue
        if low.startswith("db "):
            count = len([t for t in line[3:].split(",") if t.strip()])
            items.append(Item("data", "", "", 0, function, line_no, CAT_DATA))
            items[-1].words = count / 2  # PIC18 packs two `db` bytes per word
            org += count
            high_water = max(high_water, org)
            continue

        parts = line.split(None, 1)
        mnemonic = parts[0].upper()
        operands = parts[1].strip() if len(parts) > 1 else ""
        words = table.words(mnemonic)
        items.append(
            Item(
                "instr",
                mnemonic,
                operands,
                words,
                function,
                line_no,
                pending_region or "",
            )
        )
        org += words * unit
        high_water = max(high_water, org)

    return items, high_water // unit


# --------------------------------------------------------------------------
# Sink categorisation
# --------------------------------------------------------------------------

COND_BRANCH = {"BZ", "BNZ", "BC", "BNC", "BN", "BNN", "BOV", "BNOV"}
SKIP_TEST = {"BTFSC", "BTFSS"}
COMPARE_ALU = {"SUBWF", "XORWF", "SUBWFB", "ADDWF", "IORWF", "ANDWF", "MOVF", "CPFSEQ"}
LITERAL_LOAD = {"MOVLW", "SUBLW", "XORLW"}
LANE_ALU = {"ADDWF", "ADDWFC", "SUBWF", "SUBWFB", "XORWF", "IORWF", "ANDWF"}
ROTATE = {"RLCF", "RRCF", "RLNCF", "RRNCF", "RLF", "RRF"}

_PIC14_BANK_BIT = re.compile(r"^(STATUS|0x0*3)\s*,\s*[56]\b", re.I)
_CARRY_CLEAR = re.compile(r"^(STATUS|0x0*3|0xFD8)\s*,\s*0\b", re.I)


@dataclasses.dataclass(frozen=True)
class Config:
    """Thresholds the pattern rules read, so tuning stays out of the rules."""

    movff_run: int = 2
    jump_table_run: int = 4
    compare_chain_units: int = 3
    shift_run: int = 3


def is_bank_switch(item):
    """A bank-select write: `MOVLB`/`BANKSEL`, or a PIC14 STATUS RP0/RP1 poke."""
    if item.mnemonic in ("MOVLB", "BANKSEL"):
        return True
    return item.mnemonic in ("BSF", "BCF") and bool(
        _PIC14_BANK_BIT.match(item.operands)
    )


def _first_operand(operands):
    return operands.split(",", 1)[0].strip()


def _dest_address(item):
    """The file-register address an instruction writes, when it is a literal."""
    return _parse_int(_first_operand(item.operands))


def _literal(item):
    """The `MOVLW` operand as an int, or its `LOW(x)`/`HIGH(x)` token."""
    token = item.operands.strip()
    value = _parse_int(token)
    if value is not None:
        return value
    if re.match(r"^(LOW|HIGH|UPPER|PAGE)\s*\(", token, re.I):
        return token.upper()
    return None


def match_dead_roundtrip(items, i, cfg):
    """A value parked in a slot and read straight back out of it.

    `MOVWF f` then `MOVF f,W` is two words for nothing; `MOVWF f` then
    `MOVFF f,g` is three where `MOVWF g` is one. epic-cc#209 measured the
    PIC14 half of this and epic-cc#469 found the PIC18 backend never got
    the W-tracking that fixed it.
    """
    del cfg
    if i + 1 >= len(items) or items[i].mnemonic != "MOVWF":
        return 0
    slot = _dest_address(items[i])
    nxt = items[i + 1]
    if slot is None or _dest_address(nxt) != slot:
        return 0
    if nxt.mnemonic == "MOVFF":
        return 2
    reads_into_w = re.match(r"^[^,]+,\s*W\b", nxt.operands, re.I)
    return 2 if nxt.mnemonic == "MOVF" and reads_into_w else 0


def match_struct_copy(items, i, cfg):
    """MOVFF runs: a by-value struct or aggregate copy, 2 words per byte."""
    j = i
    while j < len(items) and items[j].mnemonic == "MOVFF":
        j += 1
    return j - i if j - i >= cfg.movff_run else 0


def match_jump_table(items, i, cfg):
    """A computed-jump dispatch table: a run of bare GOTOs, 2 words each."""
    j = i
    while j < len(items) and items[j].mnemonic == "GOTO":
        j += 1
    return j - i if j - i >= cfg.jump_table_run else 0


def _compare_unit(items, i):
    """Length of one compare-against-literal-then-branch unit at `i`, else 0."""
    j = i
    if j < len(items) and is_bank_switch(items[j]):
        j += 1
    if j >= len(items) or items[j].mnemonic not in LITERAL_LOAD:
        return 0
    j += 1
    alu = 0
    while j < len(items) and items[j].mnemonic in COMPARE_ALU and alu < 3:
        j += 1
        alu += 1
    if j >= len(items):
        return 0
    if items[j].mnemonic in COND_BRANCH:
        return j + 1 - i
    if items[j].mnemonic in SKIP_TEST:
        j += 1
        if j < len(items) and items[j].mnemonic in ("GOTO", "BRA"):
            return j + 1 - i
    return 0


def match_compare_chain(items, i, cfg):
    """A switch lowered as a chain of literal compares instead of a dispatch.

    Requires several back-to-back compare-and-branch units testing at most
    two file registers (the low and high byte of one switch value), which
    is what separates a real chain from unrelated arithmetic that happens
    to compare a constant once.
    """
    j = i
    units = 0
    tested = set()
    while True:
        step = _compare_unit(items, j)
        if not step:
            break
        for k in range(j, j + step):
            if items[k].mnemonic in COMPARE_ALU:
                addr = _dest_address(items[k])
                if addr is not None:
                    tested.add(addr)
        if len(tested) > 2:
            return 0
        j += step
        units += 1
    return j - i if units >= cfg.compare_chain_units else 0


def _store_unit(items, i):
    """`MOVLW k; MOVWF f` as (length, literal, destination), else None."""
    if i + 1 >= len(items):
        return None
    if items[i].mnemonic != "MOVLW" or items[i + 1].mnemonic != "MOVWF":
        return None
    lit = _literal(items[i])
    dest = _dest_address(items[i + 1])
    if lit is None or dest is None:
        return None
    return 2, lit, dest


def match_wide_const(items, i, cfg):
    """A multi-byte constant materialised one `MOVLW`/`MOVWF` pair per byte.

    Consecutive destination addresses are what identify it as one value
    rather than unrelated stores. An all-zero run is left to the zero-init
    rule, which names the cheaper missed lowering.
    """
    del cfg
    first = _store_unit(items, i)
    if first is None:
        return 0
    _, lit, dest = first
    literals = [lit]
    j = i + 2
    while True:
        nxt = _store_unit(items, j)
        if nxt is None or nxt[2] != dest + 1:
            break
        literals.append(nxt[1])
        dest = nxt[2]
        j += 2
    if len(literals) < 2:
        return 0
    if all(v == 0 for v in literals):
        return 0
    return j - i


def match_bool_materialization(items, i, cfg):
    """A one-bit value turned into 0/1 through a branch diamond.

    `BRA join / MOVLW 0 / BRA out / MOVLW 1` is how a comparison result
    reaches a byte slot today. The branches and both literal loads are
    pure overhead against a rotate of the carry or status bit.
    """
    del cfg
    j = i
    if j < len(items) and items[j].mnemonic == "BRA":
        j += 1
    if j + 2 >= len(items):
        return 0
    if items[j].mnemonic != "MOVLW" or items[j + 1].mnemonic != "BRA":
        return 0
    if items[j + 2].mnemonic != "MOVLW":
        return 0
    first, second = _literal(items[j]), _literal(items[j + 2])
    if {first, second} != {0, 1}:
        return 0
    return j + 3 - i


def match_shift_chain(items, i, cfg):
    """A shift by a constant unrolled into one rotate per bit position."""
    j = i
    while j < len(items):
        mne = items[j].mnemonic
        carry_clear = mne in ("BCF", "BSF") and _CARRY_CLEAR.match(items[j].operands)
        if mne in ROTATE or carry_clear:
            j += 1
            continue
        break
    span = j - i
    if span < cfg.shift_run:
        return 0
    if not any(items[k].mnemonic in ROTATE for k in range(i, j)):
        return 0
    return span


def _lane_unit(items, i):
    """`MOVLW k; <alu> f,W` as (length, file address), else None."""
    if i + 1 >= len(items):
        return None
    if items[i].mnemonic != "MOVLW" or items[i + 1].mnemonic not in LANE_ALU:
        return None
    addr = _dest_address(items[i + 1])
    if addr is None:
        return None
    length = 2
    if i + 2 < len(items) and items[i + 2].mnemonic in ("MOVWF", "MOVFF"):
        length = 3
    return length, addr


def match_wide_literal_arith(items, i, cfg):
    """A 16-bit or wider add/subtract/compare against a literal, byte by byte.

    Each extra byte lane costs its own `MOVLW` even when the literal byte
    is zero, so the literal half of a wide arithmetic op is as expensive
    as the arithmetic. Consecutive file addresses identify the lanes of
    one value.
    """
    del cfg
    first = _lane_unit(items, i)
    if first is None:
        return 0
    length, addr = first
    j = i + length
    lanes = 1
    while True:
        nxt = _lane_unit(items, j)
        if nxt is None or nxt[1] != addr + 1:
            break
        addr = nxt[1]
        j += nxt[0]
        lanes += 1
    return j - i if lanes >= 2 else 0


def match_zero_init(items, i, cfg):
    """Zero written as `MOVLW 0x00` + `MOVWF`, where `CLRF` is one word."""
    del cfg
    unit = _store_unit(items, i)
    if unit is None or unit[1] != 0:
        return 0
    return 2


def match_bank_switch(items, i, cfg):
    """A bank select. One word each, but the count tracks slot placement."""
    del cfg
    return 1 if is_bank_switch(items[i]) else 0


@dataclasses.dataclass(frozen=True)
class Rule:
    name: str
    match: object


# Precedence order: the first rule that matches consumes its window.
# Wide-const runs ahead of zero-init so a half-zero 16-bit constant is
# reported as one materialisation, not a pair plus a stray store.
SINK_RULES = (
    Rule("dead-store-reload", match_dead_roundtrip),
    Rule("struct-copy-movff", match_struct_copy),
    Rule("switch-jump-table", match_jump_table),
    Rule("switch-compare-chain", match_compare_chain),
    Rule("bool-materialization", match_bool_materialization),
    Rule("shift-chain", match_shift_chain),
    Rule("wide-const-materialization", match_wide_const),
    Rule("wide-literal-arith", match_wide_literal_arith),
    Rule("zero-init-pair", match_zero_init),
    Rule("bank-switch", match_bank_switch),
)


def categorize(items, cfg=None):
    """Stamp `item.category` on every instruction item, in place."""
    cfg = cfg or Config()
    i = 0
    while i < len(items):
        item = items[i]
        if item.kind != "instr" or item.category:
            i += 1
            continue
        for rule in SINK_RULES:
            span = rule.match(items, i, cfg)
            if span:
                for k in range(i, i + span):
                    if items[k].kind == "instr" and not items[k].category:
                        items[k].category = rule.name
                i += span
                break
        else:
            item.category = CAT_OTHER
            i += 1
    return items


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def _region_carry(items):
    """Propagate a `.table` region marker to the instructions that follow it."""
    region = None
    for item in items:
        if item.kind == "region":
            region = item.category
            continue
        if item.kind == "instr" and region:
            item.category = region
    return items


def summarize(items, total_words):
    cells = {}
    per_function = {}
    per_category = {}
    remainder = {}
    for item in items:
        if item.kind == "region" or not item.words:
            continue
        key = (item.function, item.category)
        cells[key] = cells.get(key, 0) + item.words
        per_function[item.function] = per_function.get(item.function, 0) + item.words
        per_category[item.category] = per_category.get(item.category, 0) + item.words
        if item.category == CAT_OTHER:
            remainder[item.mnemonic] = remainder.get(item.mnemonic, 0) + item.words
    return {
        "total_words": total_words,
        "counted_words": sum(per_function.values()),
        "cells": cells,
        "functions": per_function,
        "categories": per_category,
        "remainder": remainder,
    }


def _pct(words, total):
    return (100.0 * words / total) if total else 0.0


def _fmt_words(words):
    return f"{words:.0f}" if float(words).is_integer() else f"{words:.1f}"


def render(summary, table, args):
    total = summary["total_words"]
    out = []
    out.append(f"epic-cc density profile: {args.label}")
    out.append(f"word table  {table.describe()}")
    out.append(f"program     {_fmt_words(total)} flash words in the listing")
    if args.flash_words:
        residual = args.flash_words - total
        out.append(
            f"driver      {args.flash_words} flash words reported, "
            f"{_fmt_words(residual)} not in the listing "
            "(far-branch expansion, PCL alignment padding)"
        )
        total = args.flash_words
    if args.xc8_words:
        ratio = total / args.xc8_words
        out.append(
            f"vs XC8      {args.xc8_words} words, ratio {ratio:.2f}x, "
            f"{_fmt_words(total - args.xc8_words)} words over"
        )
    out.append("")

    out.append("By category")
    out.append(f"  {'category':<28}{'words':>9}{'%':>8}")
    for cat, words in sorted(summary["categories"].items(), key=lambda kv: -kv[1]):
        out.append(f"  {cat:<28}{_fmt_words(words):>9}{_pct(words, total):>7.1f}%")
    out.append("")

    out.append(f"By function (top {args.top})")
    out.append(f"  {'function':<44}{'words':>9}{'%':>8}")
    for fn, words in sorted(summary["functions"].items(), key=lambda kv: -kv[1])[
        : args.top
    ]:
        out.append(f"  {fn:<44}{_fmt_words(words):>9}{_pct(words, total):>7.1f}%")
    out.append("")

    out.append(f"Top sinks, function x category (top {args.top})")
    out.append(f"  {'function':<36}{'category':<28}{'words':>9}{'%':>8}")
    ranked = sorted(summary["cells"].items(), key=lambda kv: -kv[1])
    for (fn, cat), words in ranked[: args.top]:
        if words < args.min_words:
            break
        out.append(
            f"  {fn:<36}{cat:<28}{_fmt_words(words):>9}{_pct(words, total):>7.1f}%"
        )

    if args.show_other:
        out.append("")
        out.append(f"Uncategorised remainder by mnemonic (top {args.top})")
        out.append(f"  {'mnemonic':<28}{'words':>9}{'%':>8}")
        for mne, words in sorted(summary["remainder"].items(), key=lambda kv: -kv[1])[
            : args.top
        ]:
            out.append(f"  {mne:<28}{_fmt_words(words):>9}{_pct(words, total):>7.1f}%")
    return "\n".join(out)


def to_json(summary):
    return {
        "total_words": summary["total_words"],
        "counted_words": summary["counted_words"],
        "categories": summary["categories"],
        "functions": summary["functions"],
        "cells": [
            {"function": fn, "category": cat, "words": words}
            for (fn, cat), words in sorted(
                summary["cells"].items(), key=lambda kv: -kv[1]
            )
        ],
    }


def render_compare(summary, previous, top):
    """Deltas against an earlier `--json` run: what a fix actually moved."""
    old_cells = {
        (c["function"], c["category"]): c["words"] for c in previous.get("cells", [])
    }
    new_cells = summary["cells"]
    out = []
    old_total = previous.get("total_words", 0)
    delta = summary["total_words"] - old_total
    out.append(
        f"total {_fmt_words(old_total)} -> {_fmt_words(summary['total_words'])} "
        f"words ({delta:+.0f})"
    )
    out.append("")
    out.append(f"  {'function':<36}{'category':<28}{'delta':>9}")
    keys = set(old_cells) | set(new_cells)
    rows = [(k, new_cells.get(k, 0) - old_cells.get(k, 0)) for k in keys]
    rows = [r for r in rows if r[1]]
    rows.sort(key=lambda r: -abs(r[1]))
    for (fn, cat), d in rows[:top]:
        out.append(f"  {fn:<36}{cat:<28}{d:>+9.0f}")
    if not rows:
        out.append("  (no change)")
    return "\n".join(out)


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def compile_to_asm(args, workdir):
    """Run the driver to produce the listing, for a `.c`-input invocation."""
    if not args.device:
        raise ProfileError("--compile needs --device")
    out = pathlib.Path(workdir) / "density.asm"
    cmd = [args.epic_cc, "--target", args.device, "--emit", "asm", "-o", str(out)]
    cmd += args.cflag + args.inputs
    run = subprocess.run(cmd, capture_output=True, text=True)
    if run.returncode != 0:
        raise ProfileError(f"{args.epic_cc} failed:\n{run.stderr}")
    return out


def profile_text(text, args):
    family = detect_family(text, args.device)
    table = word_table(family)
    items, total = parse_listing(text, table)
    _region_carry(items)
    categorize(
        items,
        Config(
            movff_run=args.movff_run,
            jump_table_run=args.jump_table_run,
            compare_chain_units=args.compare_chain_units,
            shift_run=args.shift_run,
        ),
    )
    return table, summarize(items, total)


def build_parser():
    p = argparse.ArgumentParser(
        description="rank flash-word sinks in an epic-cc `--emit asm` listing"
    )
    p.add_argument(
        "inputs", nargs="+", help="an .asm listing, or sources with --compile"
    )
    p.add_argument("--compile", action="store_true", help="run epic-cc on the inputs")
    p.add_argument("--epic-cc", default="epic-cc", help="driver binary for --compile")
    p.add_argument(
        "--cflag", action="append", default=[], help="extra driver flag (repeatable)"
    )
    p.add_argument("--device", help="device, e.g. 18F4550 (else read from `list p=`)")
    p.add_argument("--xc8-words", type=int, help="XC8 reference word count for a ratio")
    p.add_argument(
        "--flash-words",
        type=int,
        help="the driver's own reported flash words, to show what the listing misses",
    )
    p.add_argument("--top", type=int, default=25, help="rows per table (default 25)")
    p.add_argument("--min-words", type=float, default=1, help="hide sinks below this")
    p.add_argument("--movff-run", type=int, default=Config.movff_run)
    p.add_argument("--jump-table-run", type=int, default=Config.jump_table_run)
    p.add_argument(
        "--compare-chain-units", type=int, default=Config.compare_chain_units
    )
    p.add_argument("--shift-run", type=int, default=Config.shift_run)
    p.add_argument(
        "--show-other",
        action="store_true",
        help="break the uncategorised remainder down by mnemonic",
    )
    p.add_argument("--json", action="store_true", help="emit JSON instead of a table")
    p.add_argument("--compare", help="an earlier --json file to diff against")
    return p


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        with tempfile.TemporaryDirectory() as workdir:
            if args.compile:
                path = compile_to_asm(args, workdir)
            else:
                if len(args.inputs) != 1:
                    raise ProfileError("without --compile, pass exactly one .asm file")
                path = pathlib.Path(args.inputs[0])
            args.label = str(path if not args.compile else f"{args.device} build")
            table, summary = profile_text(path.read_text(), args)
    except (ProfileError, OSError) as e:
        print(f"density-profile: {e}", file=sys.stderr)
        return 2
    if args.compare:
        previous = json.loads(pathlib.Path(args.compare).read_text())
        print(render_compare(summary, previous, args.top))
        return 0
    if args.json:
        print(json.dumps(to_json(summary), indent=2))
        return 0
    print(render(summary, table, args))
    return 0


if __name__ == "__main__":
    sys.exit(main())
