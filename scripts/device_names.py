#!/usr/bin/env python3
"""
device_names.py -- add pack names to an existing device TOML.

Fills the two name surfaces the XC8 source conventions need (docs/46 D-3)
from a Microchip DFP `.PIC` (EDC) file, Apache-2.0 per ADR-029:

  * `[[sfrs]]`: every SFR's name, address, width and bitfields, one mode
    per alternate bit naming (XC8's `<REG>bits` union members);
  * `aliases` on `[[config.fields]]` and their values: the pack's own
    config names (`FOSC`, `HS`), so `#pragma config` spellings resolve.

It edits the TOML in place instead of regenerating it (gen-device.py), so
hand-curated facts survive, including datasheet-tier files. Config fields
are matched to the pack by bit position, never by name: a TOML field with
no pack field at the same byte and mask, or a value with no pack value of
the same bits, is reported, which is the cross-check between transcribed
and pack config data.

  python3 scripts/device_names.py p16f877a --atdf <pack>/edc/PIC16F877A.PIC
  python3 scripts/device_names.py p16f877a --atdf ... --check   # stale?

Stdlib only, and runs on the dev container's Python 3.10: it reads TOML
with add_device.py's subset parser, which is why every value it writes,
`fields` included, stays on one line.
"""

import argparse
import pathlib
import re
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from add_device import parse_toml  # noqa: E402

NS = "{http://crownking/edc}"
ROOT = pathlib.Path(__file__).resolve().parent.parent


def attr(el, name, default=None):
    return el.get(NS + name, default)


def num(s):
    return int(s, 0)


def _sfr_elements(el):
    """SFRs in document order, descending into joined and muxed wrappers so
    both a 16-bit pair's halves and every register sharing a muxed address
    are listed."""
    for child in el:
        tag = child.tag[len(NS) :]
        if tag in ("SFRDef", "JoinedSFRDef"):
            yield child
            if tag == "JoinedSFRDef":
                yield from _sfr_elements(child)
        elif tag in ("MuxedSFRDef", "SelectSFR"):
            yield from _sfr_elements(child)


def _sfr_fields(el, width_bits):
    """Bitfields per mode. `AdjustPoint` skips reserved bits before the next
    field (the same cursor walk gen-device.py uses for config bytes). A name
    already given by an earlier mode is dropped: C rejects two union members
    of one name, and the first mode is the datasheet's."""
    seen = set()
    modes = []
    mode_list = el.find(NS + "SFRModeList")
    if mode_list is None:
        return []
    for mode in mode_list.findall(NS + "SFRMode"):
        cursor = 0
        fields = []
        for child in mode:
            tag = child.tag[len(NS) :]
            if tag == "AdjustPoint":
                cursor += num(attr(child, "offset"))
            elif tag == "SFRFieldDef":
                width = num(attr(child, "nzwidth"))
                name = attr(child, "cname")
                whole = width >= width_bits
                hidden = attr(child, "islanghidden") == "true"
                if not whole and not hidden and name not in seen:
                    mask = (num(attr(child, "mask")) << cursor) & 0xFF
                    fields.append((name, mask, cursor))
                    seen.add(name)
                cursor += width
        if fields:
            modes.append(fields)
    return [(n, m, s, i) for i, fs in enumerate(modes) for (n, m, s) in fs]


def parse_sfrs(root):
    out = []
    names = set()
    for sector in root.iter(NS + "SFRDataSector"):
        for el in _sfr_elements(sector):
            name = attr(el, "cname")
            addr = attr(el, "_addr")
            if addr is None or attr(el, "ishidden") == "true" or name in names:
                continue
            width_bits = num(attr(el, "nzwidth"))
            width = width_bits // 8
            fields = _sfr_fields(el, width_bits) if width == 1 else []
            # Legacy, migration and HI-TECH aliases are all spellings older
            # code uses (`SPBRG` on the 1937's `SP1BRGL`).
            alias_list = el.find(NS + "AliasList")
            legacy = (
                [] if alias_list is None else [attr(a, "cname") for a in alias_list]
            )
            legacy = [a for a in dict.fromkeys(legacy) if a != name]
            out.append(
                {
                    "name": name,
                    "addr": num(addr),
                    "width": width,
                    "aliases": legacy,
                    "fields": fields,
                }
            )
            names.add(name)
    return out


WHEN = re.compile(r"\(field\s*&\s*(0x[0-9a-fA-F]+)\)\s*==\s*(0x[0-9a-fA-F]+)")


def parse_config(root, core, base_byte_addr):
    """Pack config fields as `(byte_offset, byte_mask) -> (names, {abs_bits:
    names})`, with bits already placed in the byte, so they compare with a
    TOML field without knowing how either side encodes a scattered mask.
    PIC14-family config words are word-addressed and split into bytes."""
    out = {}
    for dcr in root.iter(NS + "DCRDef"):
        addr = num(attr(dcr, "_addr"))
        byte_addr = addr if core == "pic18" else addr * 2
        for mode in dcr.iter(NS + "DCRMode"):
            cursor = 0
            for child in mode:
                tag = child.tag[len(NS) :]
                if tag == "AdjustPoint":
                    cursor += num(attr(child, "offset"))
                    continue
                if tag != "DCRFieldDef":
                    continue
                span = num(attr(child, "nzwidth"))
                word_mask = num(attr(child, "mask")) << cursor
                name = attr(child, "cname")
                values = {}
                for sem in child.findall(NS + "DCRFieldSemantic"):
                    m = WHEN.search(attr(sem, "when", ""))
                    if m:
                        bits = num(m.group(2)) << cursor
                        values.setdefault(bits, []).append(attr(sem, "cname"))
                if word_mask & 0xFF and word_mask >> 8:
                    print(
                        f"device_names: pack field {name} straddles the bytes of "
                        f"0x{addr:X} and is not matched",
                        file=sys.stderr,
                    )
                for lane in (0, 1):
                    mask = (word_mask >> (8 * lane)) & 0xFF
                    if mask and (mask << (8 * lane)) == word_mask:
                        key = (byte_addr + lane - base_byte_addr, mask)
                        vals = {(b >> (8 * lane)) & 0xFF: n for b, n in values.items()}
                        names, known = out.setdefault(key, ([], {}))
                        if name not in names:
                            names.append(name)
                        for b, ns in vals.items():
                            for n in ns:
                                if n not in known.setdefault(b, []):
                                    known[b].append(n)
                cursor += span
    return out


def config_aliases(toml, pack):
    """Per TOML field: (field aliases, [value aliases]) plus mismatches. An
    alias is kept only when it differs from the TOML name ignoring case."""
    result = []
    problems = []
    for f in toml["config"].get("fields", []):
        key = (f["byte_offset"], f["mask"])
        if key not in pack:
            problems.append(
                f"config field {f['name']!r} (byte {key[0]}, mask 0x{key[1]:02X}) has no pack field"
            )
            result.append(([], [[] for _ in f["values"]]))
            continue
        names, known = pack[key]
        fa = [n for n in names if n.lower() != f["name"].lower()]
        vas = []
        for v in f["values"]:
            placed = (v["bits"] << f["shift"]) & f["mask"]
            pvals = known.get(placed)
            if pvals is None:
                problems.append(
                    f"config {f['name']}={v['name']} (bits 0x{placed:02X}) has no pack value"
                )
                vas.append([])
            else:
                vas.append([n for n in pvals if n.lower() != v["name"].lower()])
        result.append((fa, vas))
    return result, problems


def _str_list(items):
    return "[" + ", ".join(f'"{s}"' for s in items) + "]"


def render_values(values, aliases):
    parts = []
    for v, al in zip(values, aliases):
        extra = f", aliases = {_str_list(al)}" if al else ""
        parts.append(f'{{ name = "{v["name"]}", bits = {v["bits"]}{extra} }}')
    return "values = [" + ", ".join(parts) + "]"


def render_sfrs(sfrs):
    out = []
    for s in sfrs:
        out.append("")
        out.append("[[sfrs]]")
        out.append(f'name = "{s["name"]}"')
        out.append(f"addr = 0x{s['addr']:04X}")
        out.append(f"width = {s['width']}")
        if s["aliases"]:
            out.append(f"aliases = {_str_list(s['aliases'])}")
        parts = []
        for name, mask, shift, mode in s["fields"]:
            m = f", mode = {mode}" if mode else ""
            parts.append(
                f'{{ name = "{name}", mask = 0x{mask:02X}, shift = {shift}{m} }}'
            )
        out.append("fields = [" + ", ".join(parts) + "]")
    return out


def rewrite(text, toml, aliases, sfrs):
    """Strip what this script owns (`aliases` lines and value lines inside
    `[[config.fields]]` blocks, the `[[sfrs]]` tail) and write it back fresh,
    so a rerun is idempotent and `--check` is a plain text comparison."""
    lines = text.split("\n")
    sfr_at = next((i for i, ln in enumerate(lines) if ln == "[[sfrs]]"), None)
    if sfr_at is not None:
        lines = lines[:sfr_at]
    while lines and lines[-1] == "":
        lines.pop()
    fields = toml["config"].get("fields", [])
    out = []
    k = -1
    in_field = False
    anchored = set()
    for ln in lines:
        if ln.startswith("["):
            in_field = ln == "[[config.fields]]"
            k += in_field
        elif in_field and ln.startswith("aliases = "):
            continue
        elif in_field and ln.startswith("values = ["):
            ln = render_values(fields[k]["values"], aliases[k][1])
        out.append(ln)
        if in_field and ln.startswith("name = ") and k not in anchored:
            anchored.add(k)
            if aliases[k][0]:
                out.append(f"aliases = {_str_list(aliases[k][0])}")
    if anchored != set(range(len(fields))):
        sys.exit("device_names: a [[config.fields]] block has no name line")
    out.extend(render_sfrs(sfrs))
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(
        description="add pack SFR and config names to a device TOML"
    )
    ap.add_argument("device", help="registry stem, e.g. p16f877a")
    ap.add_argument(
        "--atdf", type=pathlib.Path, required=True, help="the part's EDC .PIC file"
    )
    ap.add_argument(
        "--toml", type=pathlib.Path, help="default crates/device/devices/<device>.toml"
    )
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if the TOML is not what this would write",
    )
    args = ap.parse_args()

    path = args.toml or ROOT / "crates" / "device" / "devices" / f"{args.device}.toml"
    text = path.read_text()
    # Only the part this script does not own is read back, so a table
    # written in an older layout never has to parse.
    toml = parse_toml(text.split("\n[[sfrs]]\n", 1)[0])
    root = ET.parse(args.atdf).getroot()
    pack = parse_config(root, toml["core"], toml["config"]["base_byte_addr"])
    aliases, problems = config_aliases(toml, pack)
    for p in problems:
        print(f"device_names: {args.device}: {p}", file=sys.stderr)
    new = rewrite(text, toml, aliases, parse_sfrs(root))
    parse_toml(new)
    if args.check:
        if new != text:
            print(
                f"device_names --check: {path} is stale; rerun without --check",
                file=sys.stderr,
            )
            sys.exit(1)
        return
    path.write_text(new)


if __name__ == "__main__":
    main()
