#!/usr/bin/env python3
"""
gen-device.py -- DFP/ATDF -> TOML generator for crates/device/devices/*.toml.

Source posture: ATDF/EDC is authoritative, free download; gputils .inc is
byte-for-byte oracle; XC8 headers are black-box oracle only (GPL boundary
per AGENTS.md). The .atdf/.PIC itself is never committed, only the TOML it
generates; derivation is transcription, not copy.

Input priority:
  1. --atdf PATH: explicit ATDF or EDC PIC file (XML)
  2. Local XC8 DFP packs under /opt/microchip/xc8/v4.00/pic/packs/*/edc/*.PIC
  3. ini + cfgdata under the same pack (fallback, same DFP)

Output: crates/device/devices/<stem>.toml deterministically formatted.
  python3 scripts/gen-device.py PIC16F887 --out crates/device/devices/p16f887.toml
  python3 scripts/gen-device.py p16f887 --check   # CI: fails if drift

Alias table: normalises DFP names to our EPIC_CONFIG names.
  Field aliases: FOSC->osc, WDTE->wdt, PWRTE->pwrt, BOREN/BODEN->bor/boren,
                 etc. (lowercased fallback). Value aliases: INTRC->INTOSC,
                 EXTRC edge where 16F877A uses rc for EXTRC.
  Documented here so review can see the tax.

Determinism: fields sorted by byte_offset then shift, values by bits.
The generator is stdlib only (python 3.11+, no deps) per epic-tasks rule.
"""

import argparse
import hashlib
import pathlib
import re
import sys
import xml.etree.ElementTree as ET


def normalize_stem(raw: str) -> str:
    s = raw.strip().lower()
    if s.startswith("pic"):
        s = "p" + s[3:]
    elif not s.startswith("p"):
        s = "p" + s
    return s

def stem_to_suffix(stem: str) -> str:
    assert stem.startswith("p")
    return stem[1:]

def stem_to_edc_name(stem: str) -> str:
    return "PIC" + stem[1:].upper()

FIELD_ALIAS = {
    "FOSC": "osc",
    "WDTE": "wdt",
    "PWRTE": "pwrt",
    "BOREN": "boren",
    "BODEN": "bor",
    "BOR": "bor",
    "LVP": "lvp",
    "CPD": "cpd",
    "CP": "cp",
    "WRT": "wrt",
    "DEBUG": "debug",
    "BOR4V": "bor4v",
    "MCLRE": "mclre",
    "IESO": "ieso",
    "FCMEN": "fcmen",
}

def alias_field(name: str, stem: str) -> str:
    if name == "BOREN" and stem == "p16f877a":
        return "bor"
    if name == "BODEN" and stem == "p16f877a":
        return "bor"
    return FIELD_ALIAS.get(name, name.lower())

def alias_value(field: str, value: str, stem: str) -> str:
    v = value.lower()
    if field in ("osc", "fosc") and v.startswith("intrc"):
        v = v.replace("intrc", "intosc")
    if field in ("osc", "fosc") and stem == "p16f877a":
        if v == "extrc":
            return "rc"
        if v == "extrc_clkout":
            return "rc"
    return v

# Which enum value a fuse defaults to when the user names none. This is
# compiler policy, not device geometry: geometry has no defaults at all,
# a source that does not state it makes the generator fail.
SAFE_DEFAULTS = {
    "wdt": "off",
    "bor": "on",
    "boren": "on",
    "lvp": "off",
    "cpd": "off",
    "cp": "off",
    "wrt": "off",
    "debug": "off",
    "mclre": "on",
    "ieso": "off",
    "fcmen": "off",
    "bor4v": "bor40v",
}

PART_DEFAULTS = {
    "p16f877a": {
        "osc": None,
        "wdt": "off",
        "pwrt": "on",
        "bor": "on",
        "lvp": "off",
        "cpd": "off",
        "wrt": "off",
        "debug": "off",
        "cp": "off",
    },
    "p16f887": {
        "osc": None,
        "wdt": "off",
        "pwrt": "off",
        "mclre": "on",
        "cp": "off",
        "cpd": "off",
        "boren": "on",
        "ieso": "off",
        "fcmen": "off",
        "lvp": "off",
        "debug": "off",
        "bor4v": "bor40v",
        "wrt": "off",
    },
}

def find_pack_name(atdf_path: pathlib.Path) -> str:
    # DFP directories are named *_DFP (optionally version-suffixed); the
    # immediate parent is just "edc", which would misrepresent the source.
    for parent in atdf_path.resolve().parents:
        if "_DFP" in parent.name.upper():
            return parent.name
    return "unknown"

def find_edc_pic(stem: str):
    name = stem_to_edc_name(stem) + ".PIC"
    base = pathlib.Path("/opt/microchip/xc8/v4.00/pic/packs")
    if base.exists():
        for p in base.rglob("edc/*.PIC"):
            if p.name.upper() == name.upper():
                return p
    return None

def find_ini_and_cfgdata(stem: str):
    suffix = stem_to_suffix(stem).lower()
    base = pathlib.Path("/opt/microchip/xc8/v4.00/pic/packs")
    ini = None
    cfg = None
    if base.exists():
        for p in base.rglob(f"ini/{suffix}.ini"):
            ini = p
            break
        for p in base.rglob(f"cfgdata/{suffix}.cfgdata"):
            cfg = p
            break
    return ini, cfg

def parse_ini(ini_path: pathlib.Path):
    text = ini_path.read_text()
    section_re = re.compile(r"^\[(.+)\]\s*$", re.MULTILINE)
    sections = {}
    current = None
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or line.startswith(";"):
            continue
        m = section_re.match(line)
        if m:
            current = m.group(1).strip().upper()
            sections[current] = {}
            continue
        if current is None:
            continue
        if "=" not in line:
            continue
        k, v = line.split("=", 1)
        k = k.strip()
        v = v.strip()
        if k in sections[current]:
            prev = sections[current][k]
            if isinstance(prev, list):
                prev.append(v)
            else:
                sections[current][k] = [prev, v]
        else:
            sections[current][k] = v
    return sections

def parse_rambank(s: str):
    banks = []
    for part in s.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            lo, hi = part.split("-", 1)
            banks.append((int(lo, 16), int(hi, 16)))
        else:
            banks.append((int(part, 16), int(part, 16)))
    banks.sort(key=lambda x: x[0])
    return banks

def parse_common(s):
    if isinstance(s, list):
        s = s[0]
    first = s.split(",")[0].strip()
    lo, hi = first.split("-", 1)
    return (int(lo, 16), int(hi, 16))

def parse_cfgdata(cfg_path: pathlib.Path):
    cwords = []
    current_cword = None
    current_setting = None
    for raw in cfg_path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("CWORD:"):
            parts = line.split(":")
            addr = int(parts[1], 16)
            mask = int(parts[2], 16)
            default = int(parts[3], 16)
            name = parts[4] if len(parts) > 4 else ""
            current_cword = {"addr": addr, "mask": mask, "default": default, "name": name, "settings": []}
            cwords.append(current_cword)
            current_setting = None
        elif line.startswith("CSETTING:"):
            _, mask_s, name, *rest = line.split(":", 3)
            mask = int(mask_s, 16)
            cname = name.split(",")[0].strip()
            current_setting = {"mask": mask, "name": cname, "values": []}
            if current_cword is not None:
                current_cword["settings"].append(current_setting)
        elif line.startswith("CVALUE:"):
            _, val_s, name_s, *rest = line.split(":", 3)
            val = int(val_s, 16)
            primary = name_s.split(",")[0].strip()
            if current_setting is not None:
                current_setting["values"].append({"value": val, "name": primary})
        else:
            continue
    return cwords

EDC_ARCH_TO_CORE = {"16xxxx": "pic14", "16exxx": "pic14e", "18xxxx": "pic18"}
INI_ARCH_TO_CORE = {"PIC14": "pic14", "PIC14E": "pic14e", "PIC16": "pic18"}


class MissingFacts(Exception):
    """Fields the source file does not state. Never defaulted: a fabricated
    memory map carrying a real sha256 is worse than no generator at all."""


def parse_edc(edc_path: pathlib.Path):
    # Only what the XML actually states. An absent key stays absent so the
    # caller can name it in the failure instead of substituting a number.
    ns = "{http://crownking/edc}"
    root = ET.parse(edc_path).getroot()
    out = {}
    arch = root.get(ns + "arch")
    if arch and arch.lower() in EDC_ARCH_TO_CORE:
        out["core"] = EDC_ARCH_TO_CORE[arch.lower()]
    for mt in root.iter(ns + "MemTraits"):
        hw = mt.get(ns + "hwstackdepth")
        if hw:
            out["hwstack"] = int(hw, 0)
            break
    code_end = 0
    for cs in root.iter(ns + "CodeSector"):
        end = cs.get(ns + "endaddr")
        if end:
            code_end = max(code_end, int(end, 0))
    if code_end:
        out["code_end"] = code_end
    cfg = []
    for sec in root.iter(ns + "ConfigFuseSector"):
        b = sec.get(ns + "beginaddr")
        e = sec.get(ns + "endaddr")
        if b and e:
            cfg.append((int(b, 0), int(e, 0)))
    if cfg:
        out["config_sectors"] = cfg
    # EDC endaddr is exclusive. A GPR sector that another sector shadows is
    # the bank-independent window; the shadows are mirrors, not extra storage.
    sectors = []
    shadowed = set()
    for gs in root.iter(ns + "GPRDataSector"):
        b = gs.get(ns + "beginaddr")
        e = gs.get(ns + "endaddr")
        if not (b and e):
            continue
        ref = gs.get(ns + "shadowidref")
        if ref:
            shadowed.add(ref)
            continue
        sectors.append((int(b, 0), int(e, 0) - 1, gs.get(ns + "regionid")))
    if sectors:
        out["ram_banks"] = sorted((lo, hi) for lo, hi, rid in sectors if rid not in shadowed)
        common = sorted((lo, hi) for lo, hi, rid in sectors if rid in shadowed)
        if common:
            out["common_ram"] = common[0]
    return out

def generate_toml(stem: str, ini_path, cfg_path, edc_path=None):
    ini_sections = parse_ini(ini_path) if ini_path and ini_path.exists() else {}
    suffix = stem_to_suffix(stem).upper()
    sec = ini_sections.get(suffix, {})
    if not sec:
        for k, v in ini_sections.items():
            if k.upper() == suffix.upper():
                sec = v
                break
    edc = parse_edc(edc_path) if edc_path and edc_path.exists() else {}
    missing = []

    def scalar(key):
        v = sec.get(key)
        if isinstance(v, list):
            v = v[0]
        return v.strip() if isinstance(v, str) and v.strip() else None

    def pick(field, *candidates):
        for c in candidates:
            if c is not None:
                return c
        missing.append(field)
        return None

    # ini/cfgdata first, EDC second: the committed TOMLs were generated from
    # the ini path, so it stays the primary and EDC fills what it omits.
    arch = scalar("ARCH")
    core = pick(
        "core (ini ARCH or EDC edc:arch)",
        INI_ARCH_TO_CORE.get(arch.upper()) if arch else None,
        edc.get("core"),
    )
    romsize = scalar("ROMSIZE")
    edc_words = None
    if "code_end" in edc and core is not None:
        # EDC states PIC18 program memory in bytes; flash_words counts words.
        edc_words = edc["code_end"] // 2 if core == "pic18" else edc["code_end"]
    flash_words = pick(
        "flash_words (ini ROMSIZE or EDC CodeSector)",
        int(romsize, 16) if romsize else None,
        edc_words,
    )
    if flash_words == 0:
        missing.append("flash_words (source states 0)")
    # common_ram is read from whichever source supplied ram_banks, so a part
    # with no shared window yields none instead of borrowing another part's.
    rambank = scalar("RAMBANK")
    common_ram = None
    if rambank:
        ram_banks = parse_rambank(rambank)
        common = scalar("COMMON")
        common_ram = parse_common(common) if common else None
    elif "ram_banks" in edc:
        ram_banks = edc["ram_banks"]
        common_ram = edc.get("common_ram")
    else:
        missing.append("ram_banks (ini RAMBANK or EDC GPRDataSector)")
        ram_banks = []
    stack_s = scalar("STACKDEPTH")
    stack_depth = pick(
        "stack_depth (EDC MemTraits hwstackdepth or ini STACKDEPTH)",
        edc.get("hwstack"),
        int(stack_s, 0) if stack_s and re.fullmatch(r"0[xX][0-9a-fA-F]+|\d+", stack_s) else None,
    )
    cfg_cwords = []
    if cfg_path and cfg_path.exists():
        cwords_all = parse_cfgdata(cfg_path)
        cfg_cwords = [c for c in cwords_all if c["name"].upper().startswith("CONFIG")]
    cfg_span = None
    if not cfg_cwords:
        cfg_range = scalar("CONFIG")
        if cfg_range:
            lo_s, _, hi_s = cfg_range.partition("-")
            cfg_span = (int(lo_s, 16), int(hi_s, 16) if hi_s else int(lo_s, 16))
        elif edc.get("config_sectors"):
            lo = min(b for b, _ in edc["config_sectors"])
            cfg_span = (lo, max(e for _, e in edc["config_sectors"]) - 1)
        else:
            missing.append("config words (cfgdata, ini CONFIG or EDC ConfigFuseSector)")
    if missing:
        raise MissingFacts(missing)
    if cfg_span is not None:
        # No per-fuse table, so only the word range is known. The erased value
        # is a core fact (PIC14 words read all ones in 14 bits), not per-part.
        erased_word = 0xFF if core == "pic18" else 0x3FFF
        cfg_cwords = [
            {"addr": a, "mask": erased_word, "default": erased_word, "name": "CONFIG", "settings": []}
            for a in range(cfg_span[0], cfg_span[1] + 1)
        ]
    if common_ram:
        canonical_lo, canonical_hi = common_ram
        adjusted = []
        for idx, (lo, hi) in enumerate(ram_banks):
            m_lo = canonical_lo + idx * 0x80
            m_hi = canonical_hi + idx * 0x80
            if lo <= m_lo <= hi and hi == m_hi:
                adjusted.append((lo, m_lo - 1))
            elif lo <= m_lo <= hi or lo <= m_hi <= hi:
                if lo < m_lo:
                    adjusted.append((lo, m_lo - 1))
                if m_hi < hi:
                    adjusted.append((m_hi + 1, hi))
            else:
                adjusted.append((lo, hi))
        ram_banks = adjusted
    if core == "pic18":
        interrupt_vectors = [0x0008, 0x0018]
    else:
        interrupt_vectors = [0x0004]
    cfg_cwords.sort(key=lambda c: c["addr"])
    base_word = cfg_cwords[0]["addr"]
    num_words = cfg_cwords[-1]["addr"] - base_word + 1
    base_byte_addr = base_word * 2
    num_bytes = num_words * 2
    erased = []
    for cw in cfg_cwords:
        default = cw["default"]
        erased.append(default & 0xFF)
        erased.append((default >> 8) & 0xFF)
    while len(erased) < num_bytes:
        erased.append(0xFF)
    erased = erased[:num_bytes]
    fields = []
    for cw in cfg_cwords:
        cword_byte_base = (cw["addr"] - base_word) * 2
        for setting in cw["settings"]:
            mask_word = setting["mask"]
            if mask_word == 0:
                continue
            lowbit = mask_word & -mask_word
            ctz = (lowbit.bit_length() - 1)
            word_shift = ctz
            byte_in_word = word_shift // 8
            shift_in_byte = word_shift % 8
            mask_in_byte = (mask_word >> (byte_in_word * 8)) & 0xFF
            width = bin(mask_in_byte).count("1")
            byte_offset = cword_byte_base + byte_in_word
            raw_field_name = setting["name"]
            field_name = alias_field(raw_field_name, stem)
            vals = []
            for v in setting["values"]:
                raw_val = v["value"]
                raw_name = v["name"]
                normalized = (raw_val >> word_shift) & ((1 << width) - 1) if width < 8 else (raw_val >> word_shift)
                alias = alias_value(field_name, raw_name, stem)
                vals.append((alias, normalized))
            seen_bits = {}
            deduped = []
            for name, bits in vals:
                if bits not in seen_bits:
                    seen_bits[bits] = name
                    deduped.append((name, bits))
            deduped.sort(key=lambda x: x[1])
            if not deduped:
                continue
            part_def = PART_DEFAULTS.get(stem, {})
            default_name = part_def.get(field_name)
            if default_name is None:
                default_name = SAFE_DEFAULTS.get(field_name)
                if default_name and not any(n == default_name for n, _ in deduped):
                    default_name = None
            fields.append({
                "name": field_name,
                "byte_offset": byte_offset,
                "mask": mask_in_byte,
                "shift": shift_in_byte,
                "default": default_name,
                "values": deduped,
            })
    fields.sort(key=lambda f: (f["byte_offset"], f["shift"]))
    out_lines = []
    out_lines.append(f'name = "{stem}"')
    out_lines.append(f'core = "{core}"')
    out_lines.append(f'flash_words = {flash_words}')
    banks_str = ", ".join(f"[0x{lo:04X}, 0x{hi:04X}]" for lo, hi in ram_banks)
    out_lines.append(f'ram_banks = [{banks_str}]')
    if common_ram:
        out_lines.append(f'common_ram = [0x{common_ram[0]:04X}, 0x{common_ram[1]:04X}]')
    out_lines.append(f'stack_depth = {stack_depth}')
    vectors_str = ", ".join(f"0x{v:04X}" for v in interrupt_vectors)
    out_lines.append(f'interrupt_vectors = [{vectors_str}]')
    if edc_path is not None:
        # edc_path is the ATDF/EDC PIC XML actually parsed; hash it so the
        # TOML is traceable to the exact pack that produced it.
        pack_name = find_pack_name(edc_path)
        digest = hashlib.sha256(edc_path.read_bytes()).hexdigest()
        out_lines.append("")
        out_lines.append("[provenance]")
        out_lines.append('tier = "atdf"')
        out_lines.append(f'source = "{edc_path.name}"')
        out_lines.append(f'pack = "{pack_name}"')
        out_lines.append(f'sha256 = "{digest}"')
    out_lines.append("")
    out_lines.append("[config]")
    out_lines.append(f'base_byte_addr = 0x{base_byte_addr:04X}')
    out_lines.append(f'num_bytes = {num_bytes}')
    erased_str = ", ".join(f"0x{b:02X}" for b in erased)
    out_lines.append(f'erased_baseline = [{erased_str}]')
    out_lines.append("")
    for f in fields:
        out_lines.append("[[config.fields]]")
        out_lines.append(f'name = "{f["name"]}"')
        out_lines.append(f'byte_offset = {f["byte_offset"]}')
        out_lines.append(f'mask = 0x{f["mask"]:02X}')
        out_lines.append(f'shift = {f["shift"]}')
        if f["default"] is not None:
            out_lines.append(f'default = "{f["default"]}"')
        vals_str = ", ".join(f'{{ name = "{n}", bits = {b} }}' for n, b in f["values"])
        out_lines.append(f'values = [{vals_str}]')
        out_lines.append("")
    content = "\n".join(out_lines).rstrip() + "\n"
    return content

def strip_provenance_block(text: str) -> str:
    # --check compares device numbers, not origin metadata: the stanza's
    # shape is validated separately by crates/device/provenance.rs, and its
    # tier legitimately differs between hand-written and generated TOMLs.
    blocks = text.rstrip("\n").split("\n\n")
    blocks = [b for b in blocks if not b.startswith("[provenance]")]
    return "\n\n".join(blocks) + "\n"

def main():
    ap = argparse.ArgumentParser(description="DFP/ATDF -> TOML generator for epic-cc device registry")
    ap.add_argument("device", help="device name: p16f887, PIC16F887, 16f887, etc.")
    ap.add_argument("--atdf", type=pathlib.Path, help="explicit ATDF/EDC PIC file path")
    ap.add_argument("--out", type=pathlib.Path, help="output TOML path (default crates/device/devices/<stem>.toml)")
    ap.add_argument("--check", action="store_true", help="verify existing TOML matches generated; exit 1 on drift")
    ap.add_argument("--with-sfrs", action="store_true", help="include SFR table (placeholder)")
    args = ap.parse_args()
    stem = normalize_stem(args.device)
    atdf_path = args.atdf
    ini = None
    cfg = None
    edc = None
    if atdf_path:
        if not atdf_path.exists():
            print(f"gen-device: --atdf {atdf_path} not found", file=sys.stderr)
            sys.exit(2)
        edc = atdf_path
        ini2, cfg2 = find_ini_and_cfgdata(stem)
        ini = ini2
        cfg = cfg2
        if atdf_path.suffix.lower() == ".ini":
            ini = atdf_path
            edc = None
        elif atdf_path.suffix.lower() == ".cfgdata":
            cfg = atdf_path
            edc = None
    else:
        edc = find_edc_pic(stem)
        ini, cfg = find_ini_and_cfgdata(stem)
    if (not edc or not edc.exists()) and (not ini or not ini.exists()) and (not cfg or not cfg.exists()):
        print(f"gen-device: no DFP source found for {stem}", file=sys.stderr)
        print("  Fetch the Microchip DFP pack:", file=sys.stderr)
        print("    https://packs.download.microchip.com/  (Microchip.PIC16Fxxx_DFP)", file=sys.stderr)
        print("  Or install XC8 and ensure /opt/microchip/xc8/v4.00/pic/packs exists", file=sys.stderr)
        print("  The .atdf/.PIC itself is not committed; only the generated TOML is.", file=sys.stderr)
        print("  Alternatively pass --atdf /path/to/PIC16F887.PIC", file=sys.stderr)
        sys.exit(2)
    try:
        toml_content = generate_toml(stem, ini, cfg, edc)
    except MissingFacts as e:
        # Refusing beats guessing: a TOML the generator invented would still
        # carry a real sha256 and read as attested. See ADR-021.
        print(f"gen-device: {stem}: source does not supply:", file=sys.stderr)
        for field in e.args[0]:
            print(f"  - {field}", file=sys.stderr)
        print(f"  sources read: edc={edc} ini={ini} cfgdata={cfg}", file=sys.stderr)
        sys.exit(3)
    out_path = args.out
    if not out_path:
        out_path = pathlib.Path(f"crates/device/devices/{stem}.toml")
    if args.check:
        if not out_path.exists():
            print(f"gen-device --check: {out_path} does not exist (would create)", file=sys.stderr)
            print(toml_content)
            sys.exit(1)
        existing = out_path.read_text()
        existing_cmp = strip_provenance_block(existing)
        generated_cmp = strip_provenance_block(toml_content)
        if existing_cmp != generated_cmp:
            print(f"gen-device --check: {out_path} drifts from DFP source", file=sys.stderr)
            import difflib
            diff = difflib.unified_diff(existing_cmp.splitlines(keepends=True), generated_cmp.splitlines(keepends=True), fromfile=str(out_path), tofile="generated")
            sys.stdout.writelines(diff)
            sys.exit(1)
        print(f"gen-device --check: {out_path} ok")
        sys.exit(0)
    else:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(toml_content)
        print(f"gen-device: wrote {out_path} from {ini or edc} + {cfg}")

if __name__ == "__main__":
    main()
