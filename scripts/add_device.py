#!/usr/bin/env python3
"""Helpers for scripts/add-device.sh (docs/38 D-1): EPIC_CONFIG synthesis,
the gputils RAM correction, and the sibling field-diff. Stdlib only, and
runs on the dev container's Python 3.10 (no tomllib), so it carries a
minimal TOML parser for the device-Toml subset gen-device.py emits.

The wrapper is bash; the TOML and .lkr manipulation it needs is not, so
the data transforms live here as pure functions the script calls.
"""

import pathlib
import re
import sys


# --- minimal TOML parser for the device-Toml subset ---------------------
# gen-device.py emits a deterministic shape: top-level scalars and arrays,
# a [provenance] table, a [config] table, and [[config.fields]] array-of-
# tables whose `values` are inline tables. No floats, no multi-line arrays,
# no escapes in strings. That subset is all this parser handles.


def _split_top(s, sep=","):
    """Split on sep at bracket/brace/string depth 0."""
    parts = []
    depth = 0
    in_str = False
    cur = []
    for ch in s:
        if ch == '"':
            in_str = not in_str
            cur.append(ch)
        elif not in_str:
            if ch in "[{":
                depth += 1
                cur.append(ch)
            elif ch in "]}":
                depth -= 1
                cur.append(ch)
            elif ch == sep and depth == 0:
                parts.append("".join(cur).strip())
                cur = []
            else:
                cur.append(ch)
        else:
            cur.append(ch)
    if cur:
        parts.append("".join(cur).strip())
    return parts


def _parse_value(s):
    s = s.strip()
    if s.startswith('"'):
        return s[1:-1]
    if s.startswith("["):
        inner = s[1:-1].strip()
        return [] if not inner else [_parse_value(p) for p in _split_top(inner)]
    if s.startswith("{"):
        d = {}
        for pair in _split_top(s[1:-1]):
            k, _, v = pair.partition("=")
            d[k.strip()] = _parse_value(v)
        return d
    if s.lower().startswith("0x"):
        return int(s, 16)
    if s in ("true", "false"):
        return s == "true"
    return int(s)


def parse_toml(text):
    root = {}
    current = root
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[[") and line.endswith("]]"):
            path = line[2:-2].strip().split(".")
            obj = root
            for p in path[:-1]:
                obj = obj.setdefault(p, {})
            key = path[-1]
            obj.setdefault(key, []).append({})
            current = obj[key][-1]
        elif line.startswith("[") and line.endswith("]"):
            obj = root
            for p in line[1:-1].strip().split("."):
                obj = obj.setdefault(p, {})
            current = obj
        else:
            k, _, v = line.partition("=")
            current[k.strip()] = _parse_value(v)
    return root


def _load(toml_path):
    return parse_toml(pathlib.Path(toml_path).read_text())


def normalize_stem(raw):
    """Same normalization as gen-device.py: PIC16F887/p16f887/16f887 all
    become p16f887."""
    s = raw.strip().lower()
    if s.startswith("pic"):
        s = "p" + s[3:]
    elif not s.startswith("p"):
        s = "p" + s
    return s


def synthesize_config(toml_path):
    """Build an EPIC_CONFIG spec covering every declared field: the locked
    value if locked, else the default if present, else the first enumerated
    value (docs/38 D-1 step 4). Handling locked explicitly avoids a
    synthesis-only false failure on a locked-without-default field."""
    data = _load(toml_path)
    fields = data["config"]["fields"]
    pairs = []
    for fld in fields:
        if fld.get("locked"):
            val = fld["locked"]
        elif fld.get("default"):
            val = fld["default"]
        else:
            val = fld["values"][0]["name"]
        pairs.append(f"{fld['name']}={val}")
    return ", ".join(pairs)


def _spec_value(spec, field):
    for pair in spec.split(","):
        pair = pair.strip()
        if not pair:
            continue
        key, _, val = pair.partition("=")
        if key.strip().lower() == field:
            return val.strip().lower()
    return None


def synthesize_xtal_hz(toml_path):
    """A crystal frequency the driver's fosc derivation accepts for the
    synthesized spec. PLL oscillator modes need xtal = 4 MHz * plldiv factor
    (DS39632E Register 25-1); crystal modes use xtal directly; internal
    modes ignore it. Returns an integer Hz string."""
    spec = synthesize_config(toml_path)
    osc = _spec_value(spec, "osc")
    pll = osc in ("hspll", "xtpll", "ecpll", "ecpio")
    if pll:
        factor = {
            "noprescale": 1,
            "div2": 2,
            "div3": 3,
            "div4": 4,
            "div5": 5,
            "div6": 6,
            "div10": 10,
            "div12": 12,
        }.get(_spec_value(spec, "plldiv"), 1)
        return str(4_000_000 * factor)
    return "4000000"


def _lkr_ram(lkr_text):
    """Parse a generic .lkr into (banks, shared, access), mirroring
    crates/device/src/gputils.rs: guards evaluated with no symbol defined,
    PROTECTED lines skipped, only gpr* DATABANKs counted."""
    banks, shared, access = [], [], []
    arms = []  # one bool per open guard: whether the arm is live
    for line in lkr_text.splitlines():
        line = line.strip()
        if line.startswith("#IFDEF"):
            arms.append(False)
            continue
        if line.startswith("#IFNDEF"):
            arms.append(True)
            continue
        if line.startswith("#ELSE"):
            if arms:
                arms[-1] = not arms[-1]
            continue
        if line.startswith("#FI") or line.startswith("#ENDIF"):
            if arms:
                arms.pop()
            continue
        if any(not live for live in arms):
            continue
        kind = None
        rest = None
        for prefix, k in (
            ("DATABANK", "bank"),
            ("SHAREBANK", "shared"),
            ("ACCESSBANK", "access"),
        ):
            if line.startswith(prefix):
                kind = k
                rest = line[len(prefix) :]
                break
        if kind is None or "PROTECTED" in rest:
            continue
        if kind == "bank":
            m = re.search(r"NAME=(\S+)", rest)
            if not m or not m.group(1).startswith("gpr"):
                continue

        def field(key):
            m = re.search(re.escape(key) + r"(\S+)", rest)
            return m.group(1) if m else None

        lo = field("START=")
        hi = field("END=")
        if lo and hi:
            lo = int(lo, 16)
            hi = int(hi, 16)
            if kind == "bank":
                banks.append((lo, hi))
            elif kind == "shared":
                shared.append((lo, hi))
            else:
                access.append((lo, hi))
    banks.sort()
    shared.sort()
    access.sort()
    return banks, shared, access


def _fmt_ram(ranges):
    return "[" + ", ".join(f"[0x{lo:04X}, 0x{hi:04X}]" for lo, hi in ranges) + "]"


def correct_ram(toml_path, lkr_path):
    """Widen ram_banks to match gputils when the DFP understates RAM
    (docs/32 §3: gputils wins). Returns True if the TOML was corrected.

    PIC18: ram_banks lumps the GPR part of the access window (0x10-0x5F)
    with the banked banks, so the corrected span is fixed_retval.1+1 to
    the top of the coalesced access+banks. PIC14/PIC14E: ram_banks is the
    banked GPR alone. The rewrite is a targeted line swap that preserves
    gen-device.py's deterministic formatting.

    Only ever widens. A device whose entire GPR is one gputils SHAREBANK
    (no DATABANK gpr* entries at all, confirmed on PIC12F675/PIC12F629's
    `gprnobank`) makes `banks` empty here even though gen-device.py's own
    ATDF-derived ram_banks is correct: this compiler's `common_ram` is a
    narrow, reserved-for-fixed-scratch concept (crates/alloc never places
    globals there), not a general stand-in for "gputils called it
    SHAREBANK", so collapsing to that empty reading would silently zero
    out real, already-correct RAM rather than genuinely correct it."""
    data = _load(toml_path)
    core = data["core"]
    banks, shared, access = _lkr_ram(lkr_path.read_text())
    if core == "pic18":
        fixed_retval = data.get("fixed_retval")
        if not fixed_retval:
            return False
        lo = fixed_retval[1] + 1
        hi = max([b[1] for b in banks] + [a[1] for a in access] + [lo])
        new_ram = [[lo, hi]]
    else:
        new_ram = [[lo, hi] for lo, hi in banks]
    if not new_ram:
        return False
    old_bytes = sum(hi - lo + 1 for lo, hi in data["ram_banks"])
    new_bytes = sum(hi - lo + 1 for lo, hi in new_ram)
    if new_bytes <= old_bytes:
        return False
    text = toml_path.read_text()
    old = "ram_banks = " + _fmt_ram(data["ram_banks"])
    new = "ram_banks = " + _fmt_ram(new_ram)
    if old not in text:
        return False
    toml_path.write_text(text.replace(old, new))
    return True


def field_diff(toml_path, sibling_path):
    """Diff the generated TOML's config fields against a sibling's by
    (byte_offset, shift), the docs/32 §2 review shape. Returns a list of
    human-readable lines naming fields the sibling lacks, fields the
    generated TOML lacks, and fields whose value set differs."""
    gen = _load(toml_path)
    sib = _load(sibling_path)
    gen_fields = gen["config"]["fields"]
    sib_fields = sib["config"]["fields"]
    gen_by_pos = {(f["byte_offset"], f["shift"]): f for f in gen_fields}
    sib_by_pos = {(f["byte_offset"], f["shift"]): f for f in sib_fields}
    lines = []
    for pos, f in sorted(gen_by_pos.items()):
        if pos not in sib_by_pos:
            lines.append(f"  + {f['name']} at byte {pos[0]} shift {pos[1]} (new)")
        else:
            s = sib_by_pos[pos]
            if f["name"] != s["name"]:
                lines.append(
                    f"  ~ {f['name']} at byte {pos[0]} shift {pos[1]} "
                    f"(sibling calls it {s['name']})"
                )
            gen_vals = {v["name"] for v in f["values"]}
            sib_vals = {v["name"] for v in s["values"]}
            if gen_vals != sib_vals:
                lines.append(
                    f"  ~ {f['name']} at byte {pos[0]} shift {pos[1]} "
                    f"(value set differs)"
                )
    for pos, s in sorted(sib_by_pos.items()):
        if pos not in gen_by_pos:
            lines.append(f"  - {s['name']} at byte {pos[0]} shift {pos[1]} (absent)")
    return lines


def sibling(toml_path, devices_dir):
    """The closest existing registry sibling on the same core, by name
    distance (docs/38 D-1 step 5). Returns the sibling TOML path, or None
    when no other device shares the core."""
    data = _load(toml_path)
    core = data["core"]
    stem = pathlib.Path(toml_path).stem
    best = None
    best_dist = None
    for p in sorted(pathlib.Path(devices_dir).glob("*.toml")):
        if p.stem == stem:
            continue
        try:
            other = _load(p)
        except Exception:
            continue
        if other.get("core") != core:
            continue
        dist = abs(len(p.stem) - len(stem))
        if best is None or dist < best_dist:
            best = p
            best_dist = dist
    return best


def main():
    cmd = sys.argv[1]
    if cmd == "stem":
        print(normalize_stem(sys.argv[2]))
    elif cmd == "synthesize":
        print(synthesize_config(pathlib.Path(sys.argv[2])))
    elif cmd == "synthesize-xtal":
        print(synthesize_xtal_hz(pathlib.Path(sys.argv[2])))
    elif cmd == "correct-ram":
        sys.exit(
            0
            if correct_ram(pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]))
            else 1
        )
    elif cmd == "field-diff":
        for line in field_diff(pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])):
            print(line)
    elif cmd == "sibling":
        sib = sibling(pathlib.Path(sys.argv[2]), sys.argv[3])
        print(sib if sib else "")
    else:
        print(f"add_device.py: unknown command {cmd!r}", file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
