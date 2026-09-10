#!/usr/bin/env python3
"""PIC8 flash-generation part catalog: enumerate, enrich, merge.

The catalog (crates/device/catalog/parts.toml) tracks every 8-bit PIC flash
part (PIC10F/12F/16F/18F, never the EPROM/OTP C parts) as roadmap data. It is
deliberately NOT a pile of devices/*.toml: docs/38 D-3 keeps full device data
on-demand, this table is the triage view that decides what to port next.

Sources and posture:
- gputils (GPL) is the enumeration oracle, same standing rule as the device
  gate in crates/device/tests/gputils_crosscheck.rs. We parse its outputs and
  commit only our derived numbers; nothing from gputils ships in the binary.
- The Microchip DFP packs (Apache-2.0, anonymous download) are the memory
  and core authority; the gputils rows fill RAM/EEPROM and cross-check.
- Microchip's ProductInfo MCP API (unauthenticated) supplies lifecycle and
  datasheet PDF URLs; GitHub code search supplies the popularity counts.
- Everything is stdlib-only and every network step resumes where it stopped.

Subcommands (run in order for a full regeneration):
  scan       gputils share -> part rows JSON
  fetch      download index.idx + the 8-bit DFP packs into a cache dir
  scan-index index.idx -> per-device pack/datasheet rows JSON
  scan-dfp   unpacked packs -> memory rows JSON
  scan-mcp   ProductInfo API -> lifecycle + datasheet PDF JSON
  score      GitHub code search -> reference counts JSON
  build      merge rows + scores + MCP data -> catalog TOML
"""

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

# Flash generation, including the low-voltage (LF) and high-voltage (HV)
# orderable variants: gputils ships separate headers for both.
F_PART = re.compile(r"^p(10|12|16|18)(f|lf|hv)")

CODEPAGE_ROW = re.compile(
    r"^\s*CODEPAGE\s+NAME=(\S+)\s+START=(0x[0-9A-Fa-f]+)\s+END=(0x[0-9A-Fa-f]+)(.*)$"
)
BANK_ROW = re.compile(
    r"^\s*(DATABANK|SHAREBANK|ACCESSBANK)\s+NAME=(\S+)\s+"
    r"START=(0x[0-9A-Fa-f]+)\s+END=(0x[0-9A-Fa-f]+)(.*)$"
)
PROTECTED = "PROTECTED"

# Non-flash codepages by name (the rest are flash pages). Everything listed is
# also PROTECTED in every gputils lkr, the name filter only guards against a
# future lkr dropping the flag.
NON_FLASH = {
    "idlocs",
    ".idlocs",
    "devid",
    "config",
    ".config",
    "eedata",
    ".eedata",
    "eedata1",
    "eedata2",
    "eedata3",
}


def classify_core(name: str, inc_text: str) -> str:
    """Core from the .inc symbol set: BSR only exists on the Enhanced
    Mid-Range core, PCLATH only above baseline (PIC16F54 has neither)."""
    if name.startswith("p18"):
        return "pic18"
    symbols = set(re.findall(r"^(\w+)\s+EQU", inc_text, re.M))
    if "BSR" in symbols:
        return "pic14e"
    if "PCLATH" in symbols:
        return "pic14"
    return "pic-baseline"


def parse_lkr(text: str, core: str) -> dict:
    """Flash words, allocatable RAM bytes, EEPROM bytes from a generic lkr.

    gputils PIC18 CODEPAGEs are byte-addressed, PIC14/baseline word-addressed.
    RAM is the coalesced non-protected DATABANK/SHAREBANK/ACCESSBANK span,
    the same ranges crates/device/src/gputils.rs reads for the data gate.
    """
    flash_units = 0
    eeprom = 0
    ram_spans = []
    reserved = []
    for line in text.splitlines():
        m = CODEPAGE_ROW.match(line)
        if m:
            name, start, end, rest = (
                m.group(1),
                int(m.group(2), 16),
                int(m.group(3), 16),
                m.group(4),
            )
            protected = PROTECTED in rest
            if "eedata" in name.lower().lstrip("."):
                eeprom += end - start + 1
                continue
            if protected:
                # The oscillator-calibration retlw sits in the last real flash
                # word (12F629 .oscval at 0x3FF): reserved, but device flash.
                if name.lower().lstrip(".") in ("oscval", "osccal"):
                    reserved.append((start, end))
                continue
            # Word-addressed cores top out at 0x7FFF; PIC18 CODEPAGEs are
            # byte-addressed up to 128KB (0x20000), config/idlocs/eeprom
            # spaces sit above 0x200000 and are excluded by protection or
            # name, so the cap only needs to clear real flash.
            if name not in NON_FLASH and end < (
                0x100000 if core == "pic18" else 0x10000
            ):
                flash_units = max(flash_units, end + 1)
            continue
        m = BANK_ROW.match(line)
        if m and PROTECTED not in m.group(5):
            ram_spans.append((int(m.group(3), 16), int(m.group(4), 16)))
    for start, end in reserved:
        if start == flash_units:
            flash_units = end + 1
    ram = sum(hi - lo + 1 for lo, hi in coalesce(ram_spans))
    flash_words = flash_units // 2 if core == "pic18" else flash_units
    return {"flash_words": flash_words, "ram_bytes": ram, "eeprom_bytes": eeprom}


def coalesce(ranges):
    """Merge overlapping/adjacent spans into maximal disjoint ones."""
    out = []
    for lo, hi in sorted(ranges):
        if out and lo <= out[-1][1] + 1:
            out[-1][1] = max(out[-1][1], hi)
        else:
            out.append([lo, hi])
    return out


def display_name(name: str) -> str:
    return "PIC" + name[1:].upper()


def cmd_scan(args: argparse.Namespace) -> int:
    share = Path(args.share)
    header_dir = share / "header"
    lkr_dir = share / "lkr"
    rows, missing_lkr = [], []
    for inc in sorted(header_dir.glob("*.inc")):
        name = inc.stem
        if not F_PART.match(name):
            continue
        core = classify_core(name, inc.read_text(errors="replace"))
        lkr = lkr_dir / f"{name[1:]}_g.lkr"
        if not lkr.exists():
            missing_lkr.append(name)
            continue
        row = {"name": name, "display": display_name(name), "core": core}
        row.update(parse_lkr(lkr.read_text(errors="replace"), core))
        rows.append(row)
    if missing_lkr:
        print(
            f"scan: {len(missing_lkr)} headers without a generic lkr, skipped: "
            f"{', '.join(missing_lkr)}",
            file=sys.stderr,
        )
    Path(args.out).write_text(json.dumps(rows, indent=1))
    by_core = {}
    for r in rows:
        by_core[r["core"]] = by_core.get(r["core"], 0) + 1
    print(f"scan: {len(rows)} F parts -> {args.out} ({by_core})")
    return 0


def curl(url: str, dest: Path) -> None:
    proc = subprocess.run(["curl", "-sL", "--max-time", "590", "-o", str(dest), url])
    if proc.returncode != 0:
        raise RuntimeError(f"download failed: {url}")


# The 8-bit flash families ship as these DFP groups; dsPIC/PIC24/PIC32 and
# AVR packs never match because the regex anchors right after "Microchip."
PIC8_PACK = re.compile(r"^Microchip\.PIC(10|12|16|18).*_DFP$")


def select_packs(idx_path: Path) -> dict:
    """Latest version per 8-bit DFP pack from index.idx (streamed: 50MB+)."""
    import xml.etree.ElementTree as ET

    latest = {}
    for event, el in ET.iterparse(str(idx_path), events=("end",)):
        if not el.tag.endswith("pdsc"):
            continue
        name = (el.get("name") or "").removesuffix(".pdsc")
        if PIC8_PACK.match(name):
            ver = el.get("version") or "0"
            # DFP versions are numeric dotted triples today; a future suffix
            # must not abort a fetch with a bare ValueError.
            key = tuple(int(p) if p.isdigit() else 0 for p in ver.split("."))
            prev = latest.get(name, "0")
            prev_key = tuple(int(p) if p.isdigit() else 0 for p in prev.split("."))
            if key > prev_key:
                latest[name] = ver
        el.clear()
    return latest


def cmd_fetch(args: argparse.Namespace) -> int:
    """Cache index.idx, the 8-bit DFP packs and their unpacked trees under
    the cache dir. Same posture as the gitignored vendor installers:
    Microchip downloads live outside the repo and are never committed."""
    import zipfile

    cache = Path(args.cache)
    cache.mkdir(parents=True, exist_ok=True)
    idx = cache / "index.idx"
    if not idx.exists():
        print("fetch: index.idx ...", file=sys.stderr)
        curl("https://packs.download.microchip.com/index.idx", idx)
    base = "https://packs.download.microchip.com"
    for name, ver in sorted(select_packs(idx).items()):
        pack_dir = cache / "packs" / name
        if pack_dir.exists():
            continue
        atpack = cache / f"{name}.{ver}.atpack"
        if not atpack.exists():
            print(f"fetch: {name} {ver} ...", file=sys.stderr)
            curl(f"{base}/{name}.{ver}.atpack", atpack)
        pack_dir.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(atpack) as zf:
            zf.extractall(pack_dir)
    print(
        f"fetch: {len(list((cache / 'packs').glob('*')))} packs under {cache / 'packs'}"
    )
    return 0


def cmd_scan_index(args: argparse.Namespace) -> int:
    """Per-device rows from index.idx: which pack ships the part and where
    its datasheet PDFs live. This is the Microchip-universe enumeration;
    every flash part the DFPs know appears here with its book URLs."""
    import xml.etree.ElementTree as ET

    wanted = select_packs(Path(args.index))
    rows = {}
    pack_of = {}
    for event, el in ET.iterparse(str(args.index), events=("end",)):
        if not el.tag.endswith("pdsc"):
            continue
        name = (el.get("name") or "").removesuffix(".pdsc")
        ver = el.get("version") or "0"
        is_latest = wanted.get(name) == ver
        # Devices live under releases/release/devices, not directly under
        # pdsc; walk descendants and match on the local tag name.
        for dev in el.iter():
            if not dev.tag.rsplit("}", 1)[-1] == "device":
                continue
            display = dev.get("name")
            if not PIC8_PART.match(display or ""):
                continue
            pack_of[display] = name
            if not is_latest:
                continue
            books = [
                {"title": b.get("title"), "url": b.get("name")}
                for b in dev.iter()
                if b.tag.rsplit("}", 1)[-1] == "book" and b.get("name")
            ]
            rows[display] = {
                "display": display,
                "pack": name,
                "family": dev.get("family"),
                "core_str": dev.get("core"),
                "books": books,
            }
        el.clear()
    Path(args.out).write_text(
        json.dumps({"devices": rows, "pack_of": pack_of}, indent=1)
    )
    print(f"scan-index: {len(rows)} devices -> {args.out}")
    return 0


# Flash generation, including the low-voltage (LF) and high-voltage (HV)
# orderable variants of the same silicon. C (EPROM/OTP) parts never match.
PIC8_PART = re.compile(r"^PIC(10F|10LF|12F|12LF|12HV|16F|16LF|16HV|18F|18LF)")


def parse_ini_facts(ini_path: Path) -> dict:
    """Per-device {ROMSIZE, RAMBANK, COMMON, ARCH} sections of a DFP ini."""
    sections = {}
    current = None
    for raw in ini_path.read_text(errors="replace").splitlines():
        line = raw.strip()
        if not line or line[0] in "#;":
            continue
        if line.startswith("[") and line.endswith("]"):
            current = line[1:-1].strip().upper()
            sections.setdefault(current, {})
            continue
        if current is None or "=" not in line:
            continue
        k, v = line.split("=", 1)
        sections[current].setdefault(k.strip(), v.strip())
    return sections


def edc_facts(edc_path: Path):
    """Minimal EDC scrape: arch, exclusive code end, GPR spans. Full
    config-field extraction stays in gen-device.py; the catalog only needs
    memory granularity, and gen-device's own header documents that PIC18
    packs can understate RAM, which the gputils backbone corrects."""
    import xml.etree.ElementTree as ET

    ns = "{http://crownking/edc}"
    root = ET.parse(edc_path).getroot()
    out = {"core": root.get(ns + "arch", "").lower() or None}
    code_end = 0
    for cs in root.iter(ns + "CodeSector"):
        end = cs.get(ns + "endaddr")
        if end:
            code_end = max(code_end, int(end, 0))
    out["code_end"] = code_end
    spans = []
    parent_of = {child: parent for parent in root.iter() for child in parent}

    def mode_ancestor(el):
        cur = el
        while cur in parent_of:
            cur = parent_of[cur]
            tag = cur.tag[len(ns) :] if cur.tag.startswith(ns) else cur.tag
            if tag in ("TraditionalModeOnly", "ExtendedModeOnly"):
                return tag
        return None

    shadowed = set()
    for gs in root.iter(ns + "GPRDataSector"):
        if mode_ancestor(gs) == "ExtendedModeOnly":
            continue
        b, e = gs.get(ns + "beginaddr"), gs.get(ns + "endaddr")
        if not (b and e):
            continue
        if mode_ancestor(gs) == "TraditionalModeOnly":
            spans.append((int(b, 0), int(e, 0) - 1))
            continue
        ref = gs.get(ns + "shadowidref")
        if ref:
            shadowed.add(ref)
            continue
        if gs.get(ns + "regionid") not in shadowed:
            spans.append((int(b, 0), int(e, 0) - 1))
    out["ram_spans"] = spans
    # EEPROM sectors share the exclusive endaddr convention (18856:
    # 0xF000-0xF100 is 256 bytes), so no +1 unlike the inclusive lkr rows.
    eeprom = 0
    for tag in ("EEPROMDataSector", "EepromDataSector", "EEDataSector"):
        for es in root.iter(ns + tag):
            b, e = es.get(ns + "beginaddr"), es.get(ns + "endaddr")
            if b and e:
                eeprom += int(e, 0) - int(b, 0)
    out["eeprom_bytes"] = eeprom
    return out


def cmd_scan_dfp(args: argparse.Namespace) -> int:
    """Memory facts for parts the gputils backbone lacks (the newest DFP
    tail: Q-series, five-digit 16F1xxxx parts). ini first, EDC fallback,
    the same preference gen-device.py encodes; the EDC pass also fills
    per-field gaps (core, RAM) in ini-sourced rows."""
    EDC_ARCH = {
        "16c5x": "pic-baseline",
        "16xxxx": "pic14",
        "16exxx": "pic14e",
        "18xxxx": "pic18",
    }
    # XC8's ini ARCH taxonomy: PIC12/PIC12E are the baseline 12F5xx core,
    # PIC14EX the extended enhanced mid-range of the newest 16F1xxxx.
    INI_ARCH = {
        "PIC12": "pic-baseline",
        "PIC12E": "pic-baseline",
        "PIC14": "pic14",
        "PIC14E": "pic14e",
        "PIC14EX": "pic14e",
        "PIC18": "pic18",
        "PIC18XV": "pic18",
        "PIC16": "pic18",
    }
    rows = []
    for pack_dir in sorted(Path(args.cache).glob("packs/Microchip.PIC*")):
        ini_rows = {}
        for ini_path in sorted(pack_dir.rglob("dat/ini/*.ini")):
            for suffix, sec in parse_ini_facts(ini_path).items():
                display = "PIC" + suffix
                rom = sec.get("ROMSIZE")
                # Family-template sections ship inside the XC8 inis
                # (PIC18FXXK42, PIC16F188EX, PIC16F18NVM): not orderable
                # parts, so they never enter the union.
                if (
                    not rom
                    or not PIC8_PART.match(display)
                    or "XX" in display
                    or "NVM" in display
                    or display.endswith("EX")
                ):
                    continue
                core = INI_ARCH.get(sec.get("ARCH", "").upper())
                # ini ROMSIZE counts words on PIC14, bytes on PIC18
                # (4550: ROMSIZE=8000, flash_words 16384).
                rom_words = int(rom, 16) // 2 if core == "pic18" else int(rom, 16)
                row = {
                    "display": display,
                    "pack": pack_dir.name,
                    "core": core,
                    "flash_words": rom_words,
                }
                if sec.get("RAMBANK"):
                    spans = []
                    for part in sec["RAMBANK"].split(","):
                        lo, _, hi = part.strip().partition("-")
                        spans.append((int(lo, 16), int(hi or lo, 16)))
                    row["ram_bytes"] = sum(hi - lo + 1 for lo, hi in coalesce(spans))
                ini_rows[display] = row
        rows.extend(ini_rows.values())
        edc_by_name = {p.stem: p for p in pack_dir.rglob("edc/*.PIC")}
        for stem, edc_path in sorted(edc_by_name.items()):
            if not PIC8_PART.match(stem):
                continue
            facts = edc_facts(edc_path)
            core = EDC_ARCH.get(facts["core"] or "")
            flash = None
            if facts["code_end"]:
                flash = facts["code_end"] // 2 if core == "pic18" else facts["code_end"]
            ram = (
                sum(hi - lo + 1 for lo, hi in coalesce(facts["ram_spans"]))
                if facts["ram_spans"]
                else None
            )
            known = ini_rows.get(stem)
            if known:
                # Per-field: a RAM-less ini row is still the ROMSIZE source.
                # The EDC arch is the core authority: XC8's ini labels the
                # baseline PIC10F320/322 PIC14, its EDC says 16c5x.
                if core != known.get("core"):
                    print(
                        f"scan-dfp: {stem}: ini core {known.get('core')} "
                        f"vs edc {core}, taking edc",
                        file=sys.stderr,
                    )
                known["core"] = core or known.get("core")
                known.setdefault("ram_bytes", ram)
                continue
            row = {"display": stem, "pack": pack_dir.name, "core": core}
            if flash:
                row["flash_words"] = flash
            if ram:
                row["ram_bytes"] = ram
            if facts["eeprom_bytes"]:
                row["eeprom_bytes"] = facts["eeprom_bytes"]
            rows.append(row)
    Path(args.out).write_text(json.dumps(rows, indent=1))
    print(f"scan-dfp: {len(rows)} devices -> {args.out}")
    return 0


def gh_total(query: str) -> int:
    # Transient TLS/HTTP failures are routine on a 1000-query sweep; retry
    # before giving up so one blip does not kill a long run.
    for attempt in range(3):
        proc = subprocess.run(
            [
                "gh",
                "api",
                "search/code",
                "-X",
                "GET",
                "-f",
                f"q={query}",
                "-F",
                "per_page=1",
                "--jq",
                ".total_count",
            ],
            capture_output=True,
            text=True,
        )
        if proc.returncode == 0:
            return int(proc.stdout.strip())
        time.sleep(15)
    raise RuntimeError(proc.stderr.strip() or "gh api failed")


def cmd_score(args: argparse.Namespace) -> int:
    rows = json.loads(Path(args.rows).read_text())
    scores = {}
    out = Path(args.out)
    if out.exists():
        scores = json.loads(out.read_text())
    todo = [r["display"] for r in rows if args.refill or r["display"] not in scores]
    # Classic tutorial parts get the first slots so a partial run covers the
    # parts that dominate community code before the run ends.
    classic = [
        "PIC16F877A",
        "PIC16F877",
        "PIC16F876A",
        "PIC16F873A",
        "PIC16F628A",
        "PIC16F627A",
        "PIC16F648A",
        "PIC16F887",
        "PIC16F88",
        "PIC16F84A",
        "PIC16F84",
        "PIC16F690",
        "PIC16F675",
        "PIC12F629",
        "PIC12F675",
        "PIC12F683",
        "PIC16F72",
        "PIC16F819",
        "PIC18F4550",
        "PIC18F2550",
        "PIC18F452",
        "PIC18F4520",
        "PIC18F4620",
        "PIC18F45K22",
        "PIC18F45K50",
        "PIC18F26K83",
        "PIC16F18877",
        "PIC16F1937",
        "PIC16F1939",
        "PIC16F1788",
        "PIC16F1827",
        "PIC16F1503",
        "PIC16F1455",
        "PIC16F1705",
        "PIC16F18326",
        "PIC18F26Q43",
        "PIC12F1572",
        "PIC10F200",
    ]
    flash = {r["display"]: r.get("flash_words") or 0 for r in rows}
    # Rows may carry a priority (0 first); the union builder marks Active
    # parts so a partial run fills orderable parts before the NRND tail.
    prio = {r["display"]: r.get("priority", 1) for r in rows}
    todo.sort(
        key=lambda d: (
            classic.index(d) if d in classic else len(classic),
            prio[d],
            -flash[d],
            d,
        )
    )
    if args.limit:
        todo = todo[: args.limit]
    # REST code search allows 10 authenticated req/min; pace at 6.5s so a long
    # run never trips, and write after every query so a kill loses at most one.
    for i, part in enumerate(todo):
        try:
            count = gh_total(f'"{part}"')
        except RuntimeError as exc:
            print(f"score: {part} skipped ({exc})", file=sys.stderr)
            continue
        scores[part] = count
        out.write_text(json.dumps(scores, indent=1, sort_keys=True))
        print(f"score: {part} = {count} ({i + 1}/{len(todo)})", file=sys.stderr)
        if i + 1 < len(todo):
            time.sleep(6.5)
    print(f"score: {len(todo)} parts scored -> {args.out}")
    return 0


def mcp_call(part: str) -> dict | None:
    """One Microchip ProductInfo MCP call: lifecycle + datasheet PDF URL for
    a base part number. Unauthenticated, ~2s; failures return None and stay
    retryable (nothing is cached for a failed part)."""
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "get_full_product_profile",
                "arguments": {"partNumber": part},
            },
        }
    )
    try:
        proc = subprocess.run(
            [
                "curl",
                "-s",
                "--max-time",
                "60",
                "-X",
                "POST",
                "https://api.microchip.com/mcp/resources",
                "-H",
                "Content-Type: application/json",
                "-H",
                "Accept: application/json, text/event-stream",
                "-d",
                body,
            ],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            return None
        line = next(
            (line for line in proc.stdout.splitlines() if line.startswith("data: ")),
            None,
        )
        if not line:
            return None
        inner = json.loads(json.loads(line[6:])["result"]["content"][0]["text"])
        # A null `product` (Microchip has no page for the part) surfaces as
        # TypeError on the subscript: unknown to Microchip, stays retryable.
        prods = inner["data"]["product"]["data"]["products"]
    except (json.JSONDecodeError, KeyError, StopIteration, IndexError, TypeError):
        return None
    pdfs = [p["datasheetUrl"] for p in prods if p.get("datasheetUrl")]
    lives = [p["lifecycleStatus"] for p in prods if p.get("lifecycleStatus")]
    return {"lifecycle": (lives or [None])[0], "pdf": (pdfs or [None])[0]}


def cmd_scan_mcp(args: argparse.Namespace) -> int:
    """Fill lifecycle + datasheet PDF for every union part from the
    Microchip ProductInfo MCP API. Resumable: written after every part,
    failed parts are left out for the next run. No auth, so pacing stays
    modest instead of hammering a free endpoint."""
    from concurrent.futures import ThreadPoolExecutor

    gp = json.loads(Path(args.gputils).read_text())
    dfp = json.loads(Path(args.dfp).read_text())
    union = sorted({r["display"] for r in gp} | {r["display"] for r in dfp})
    out = Path(args.out)
    state = json.loads(out.read_text()) if out.exists() else {}
    todo = [p for p in union if p not in state]
    print(f"scan-mcp: {len(todo)} of {len(union)} parts to fetch", file=sys.stderr)

    def fetch(part):
        time.sleep(0.2)
        return part, mcp_call(part)

    done = 0
    with ThreadPoolExecutor(max_workers=3) as pool:
        for part, result in pool.map(fetch, todo):
            done += 1
            if result:
                state[part] = result
            if done % 10 == 0 or done == len(todo):
                out.write_text(json.dumps(state, indent=1, sort_keys=True))
                print(f"scan-mcp: {done}/{len(todo)}", file=sys.stderr)
    out.write_text(json.dumps(state, indent=1, sort_keys=True))
    print(f"scan-mcp: {len(state)} parts enriched -> {args.out}")


def tier_of(refs) -> str:
    """Popularity tier from GitHub code-search references. Thresholds split
    the observed distribution: flagship parts are orders of magnitude above
    the long tail, which clusters under a few hundred mentions. Code search
    matches substrings, so a number that prefixes siblings reads as its
    family total (documented in ADR-029, not adjusted here)."""
    if refs is None:
        return "unscored"
    if refs >= 5000:
        return "high"
    if refs >= 500:
        return "mid"
    return "low"


def cmd_build(args: argparse.Namespace) -> int:
    """Merge to the committed catalog TOML. Precedence follows each
    source's proven strength: flash and core from the DFP (Microchip's
    current numbers; the gputils lkr under-declares, e.g. PIC16F1786
    ships one page of four), RAM and EEPROM from gputils (the DFP's GPR
    sectors understate some PIC18s, per gen-device.py's own header), with
    the family-prefix rule as last-resort core fallback. Every
    disagreement is printed, and the catalog test pins the shipped
    devices against their TOMLs."""
    gp_rows = {r["display"]: r for r in json.loads(Path(args.gputils).read_text())}
    dfp_rows = {r["display"]: r for r in json.loads(Path(args.dfp).read_text())}
    scores = (
        json.loads(Path(args.scores).read_text()) if Path(args.scores).exists() else {}
    )
    mcp = json.loads(Path(args.mcp).read_text()) if Path(args.mcp).exists() else {}

    union = sorted(set(gp_rows) | set(dfp_rows))
    merged = {}
    for display in union:
        gp, dfp = gp_rows.get(display), dfp_rows.get(display)
        core = (dfp or {}).get("core") or (gp or {}).get("core")
        if core is None:
            core = (
                "pic-baseline"
                if re.match(r"^PIC(10F2|12F5|16F5)", display)
                else "pic18"
                if display.startswith("PIC18")
                else "pic14"
            )
        entry = {
            "name": ((gp or dfp).get("name") or "p" + display[3:].lower()),
            "core": core,
        }
        if dfp:
            entry["pack"] = dfp["pack"]
        flash = [
            s
            for s in ((dfp or {}).get("flash_words"), (gp or {}).get("flash_words"))
            if s
        ]
        if flash and len(set(flash)) > 1:
            print(
                f"build: flash mismatch {display}: dfp={flash[0]} gputils={flash[1]}",
                file=sys.stderr,
            )
        if flash:
            entry["flash_words"] = flash[0]
        ram = [
            s for s in ((gp or {}).get("ram_bytes"), (dfp or {}).get("ram_bytes")) if s
        ]
        if ram and len(set(ram)) > 1:
            print(
                f"build: ram mismatch {display}: gputils={ram[0]} dfp={ram[1]}",
                file=sys.stderr,
            )
        if ram:
            entry["ram_bytes"] = ram[0]
        ee = [
            s
            for s in ((gp or {}).get("eeprom_bytes"), (dfp or {}).get("eeprom_bytes"))
            if s
        ]
        if ee and len(set(ee)) > 1:
            print(
                f"build: eeprom mismatch {display}: gputils={ee[0]} dfp={ee[1]}",
                file=sys.stderr,
            )
        if ee:
            entry["eeprom_bytes"] = ee[0]
        merged[display] = entry

    lines = [
        "# Every 8-bit PIC flash part (PIC10F/12F/16F/18F, LF and HV variants",
        "# included): the roadmap table behind docs/38 D-3's on-demand device",
        "# rule. This table tracks and prioritizes; devices/*.toml stays the",
        "# ported, verified set.",
        "# Derived data, regenerate with scripts/catalog.py: memory and core",
        "# from the DFP packs, RAM/EEPROM from the gputils oracle, lifecycle",
        "# and pdf from Microchip's ProductInfo API, gh_refs from GitHub code",
        f"# search (tier high/mid/low = refs >= 5000/500/1); fetched {args.date}.",
    ]
    for display in sorted(merged, key=lambda d: merged[d]["name"]):
        m = merged[display]
        lines.append("[[part]]")
        lines.append(f'name = "{m["name"]}"')
        lines.append(f'display = "{display}"')
        lines.append(f'core = "{m["core"]}"')
        for key in ("flash_words", "ram_bytes", "eeprom_bytes"):
            if key in m:
                lines.append(f"{key} = {m[key]}")
        if m.get("pack"):
            lines.append(f'dfp = "{m["pack"]}"')
        refs = scores.get(display)
        if refs is not None:
            lines.append(f"gh_refs = {refs}")
        lines.append(f'tier = "{tier_of(refs)}"')
        info = mcp.get(display)
        if info:
            if info.get("lifecycle"):
                lines.append(f'lifecycle = "{info["lifecycle"]}"')
            if info.get("pdf"):
                lines.append(f'pdf = "{info["pdf"]}"')
        lines.append(f'page = "{args.page_template.format(display=display)}"')
        lines.append("")
    Path(args.out).write_text("\n".join(lines))
    print(f"build: {len(merged)} parts -> {args.out}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("scan", help="gputils share -> part rows JSON")
    p.add_argument("--share", default="/usr/local/share/gputils")
    p.add_argument("--out", required=True)
    p.set_defaults(fn=cmd_scan)

    p = sub.add_parser("fetch", help="download index.idx + 8-bit DFP packs")
    p.add_argument("--cache", required=True)
    p.set_defaults(fn=cmd_fetch)

    p = sub.add_parser("scan-index", help="index.idx -> per-device rows JSON")
    p.add_argument("--index", required=True)
    p.add_argument("--out", required=True)
    p.set_defaults(fn=cmd_scan_index)

    p = sub.add_parser("scan-dfp", help="unpacked packs -> memory rows JSON")
    p.add_argument("--cache", required=True)
    p.add_argument("--out", required=True)
    p.set_defaults(fn=cmd_scan_dfp)

    p = sub.add_parser("scan-mcp", help="Microchip MCP -> lifecycle + PDF")
    p.add_argument("--gputils", required=True)
    p.add_argument("--dfp", required=True)
    p.add_argument("--out", required=True)
    p.set_defaults(fn=cmd_scan_mcp)

    p = sub.add_parser("score", help="fill GitHub code-search counts")
    p.add_argument("--rows", required=True)
    p.add_argument("--out", required=True)
    p.add_argument(
        "--limit", type=int, default=None, help="score at most N parts this run"
    )
    p.add_argument(
        "--refill", action="store_true", help="re-score parts that already have a count"
    )
    p.set_defaults(fn=cmd_score)

    p = sub.add_parser("build", help="merge rows + scores -> catalog TOML")
    p.add_argument("--gputils", required=True)
    p.add_argument("--dfp", required=True)
    p.add_argument("--scores", required=True)
    p.add_argument("--mcp", required=True)
    p.add_argument(
        "--page-template", default="https://www.microchip.com/en-us/product/{display}"
    )
    p.add_argument("--date", required=True)
    p.add_argument("--out", required=True)
    p.set_defaults(fn=cmd_build)

    args = ap.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
