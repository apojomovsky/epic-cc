#!/usr/bin/env python3
"""Render SIZE_REPORT.md from measured sizes plus reference snapshots.

Reads the JSON dump `size_regression_e2e.rs` prints under SIZE_REPORT_JSON=1,
the checked-in size baseline, an XC8 snapshot (refreshed by
size-refresh-xc8.py when the oracle image is present), and optionally a
density-profile --json run with clusters, and writes the ladder report.

Pure rendering: every number is measured elsewhere, nothing is compiled
here. Run through `make size-report`, which gathers the inputs first.
"""

import argparse
import json
import pathlib
import re
import sys

CLUSTER_CAP = 13

SUFFIX_RE = re.compile(r"-(16f877a|18f4550)$", re.IGNORECASE)


def stem(name):
    return SUFFIX_RE.sub("", name)


def parse_baseline(text):
    """The [[entry]] subset of TOML size_baseline.toml uses, nothing more."""
    entries = {}
    cur = {}
    for raw in text.splitlines():
        line = raw.strip()
        if line == "[[entry]]":
            if cur:
                entries[cur["name"]] = cur
            cur = {}
        elif cur is not None and "=" in line and not line.startswith("#"):
            key, value = line.split("=", 1)
            key = key.strip()
            value = value.strip()
            if value.startswith('"'):
                value = value.strip('"')
            else:
                value = int(value)
            cur[key] = value
    if cur:
        entries[cur["name"]] = cur
    return entries


def delta(current, base):
    if base is None:
        return "(new)"
    diff = current - base
    if diff == 0:
        return "="
    return f"{diff:+d}"


def approx(value, is_approx):
    return f"~{value}" if is_approx else str(value)


def load_json(path, what):
    try:
        return json.loads(pathlib.Path(path).read_text())
    except (OSError, json.JSONDecodeError) as e:
        raise SystemExit(f"size-report: cannot read {what} from {path}: {e}")


def menu_entry(sizes):
    menu = next((e for e in sizes if e["name"] == "hal-pic18-menu-demo-18f4550"), None)
    if menu is None:
        raise SystemExit("size-report: no hal-pic18-menu-demo-18f4550 in sizes")
    return menu


def micro_rows(sizes, baseline, xc8):
    rows = []
    for entry in sizes:
        if not entry["name"].startswith("bench-"):
            continue
        base = baseline.get(entry["name"], {})
        # Every XC8 row was measured on 18F4550; other devices get no
        # join rather than a wrong one (struct-scan runs on both cores).
        if entry["device"] == "18F4550":
            ref = xc8["benches"].get(stem(entry["name"]), {})
            xdate = ref.get("measured", "n/a")
        else:
            ref = {}
            xdate = "n/a (XC8 rows are 18F4550 only)"
        rows.append(
            {
                "bench": stem(entry["name"]),
                "device": entry["device"],
                "flash": entry["flash_words"],
                "ram": entry["ram_bytes"],
                "dflash": delta(entry["flash_words"], base.get("flash_words")),
                "dram": delta(entry["ram_bytes"], base.get("ram_bytes")),
                "xflash": ref.get("flash", "n/a"),
                "xram": ref.get("ram", "n/a"),
                "gap": (
                    entry["flash_words"] - ref["flash"] if "flash" in ref else "n/a"
                ),
                "xdate": xdate,
            }
        )
    return rows


def program_rows(sizes, baseline, xc8):
    rows = []
    for entry in sizes:
        if entry["name"].startswith("bench-"):
            continue
        base = baseline.get(entry["name"], {})
        ref = xc8["programs"].get(entry["name"], {})
        rows.append(
            {
                "name": entry["name"],
                "device": entry["device"],
                "flash": entry["flash_words"],
                "ram": entry["ram_bytes"],
                "dflash": delta(entry["flash_words"], base.get("flash_words")),
                "dram": delta(entry["ram_bytes"], base.get("ram_bytes")),
                "xflash": ref.get("flash", "n/a"),
                "xram": ref.get("ram", "n/a"),
                "gap": (
                    entry["flash_words"] - ref["flash"] if "flash" in ref else "n/a"
                ),
                "xprov": ref.get("provenance", "no XC8 counterpart"),
            }
        )
    return rows


def cluster_rows(density, xc8):
    """Top clusters by listing words that have an XC8 counterpart.

    Startup and const pools have no oracle side, so they are skipped by
    construction, not by a hardcoded skip list. Skips print to stderr so
    a new hot function missing from the snapshot is noticed, not hidden.
    """
    clusters = density.get("clusters", {})
    refs = xc8.get("clusters", {})
    ranked = []
    for root, info in clusters.items():
        ref = refs.get(root)
        if ref is None:
            print(
                f"size-report: no XC8 counterpart for cluster {root} "
                f"({info.get('words', 0):.0f} words), skipping",
                file=sys.stderr,
            )
            continue
        if ref.get("xc8") is None:
            print(
                f"size-report: no XC8 word count for cluster {root}, skipping",
                file=sys.stderr,
            )
            continue
        ranked.append((root, info))
    ranked.sort(key=lambda kv: -kv[1].get("words", 0))
    if len(ranked) > CLUSTER_CAP:
        print(
            f"size-report: showing top {CLUSTER_CAP} of {len(ranked)} "
            "clusters with XC8 counterparts",
            file=sys.stderr,
        )
    rows = []
    for root, info in ranked[:CLUSTER_CAP]:
        ref = refs[root]
        is_approx = bool(ref.get("approx"))
        ours = info.get("words", 0)
        rows.append(
            {
                "cluster": root
                + (f" ({ref['suffix']})" if ref.get("suffix") else "")
                + (
                    f" +{len(info.get('folded', []))} folded"
                    if info.get("folded")
                    else ""
                ),
                "ours": ours,
                "xc8": approx(ref["xc8"], is_approx),
                "ratio": approx(f"{ours / ref['xc8']:.2f}", is_approx),
                "gap": approx(f"{ours - ref['xc8']:+.0f}", is_approx),
            }
        )
    return rows


def render(args, sizes, density, baseline, xc8):
    out = []
    out.append("# Size ladder report")
    out.append("")
    out.append(
        "Generated from the tree, not by hand. Every epic-cc number below "
        "was measured by `size_regression_e2e.rs` on this commit; XC8 "
        "numbers quote the snapshot unless the oracle image was present."
    )
    out.append("")
    out.append(f"- Date (UTC): {args.date}")
    out.append(f"- Commit: {args.sha}")
    out.append(f"- Baseline: {args.baseline} (checked in)")
    out.append(
        f"- XC8 snapshot: measured {xc8.get('generated', 'unknown')} ({args.oracle})"
    )
    if density is not None:
        out.append(
            f"- Menu-demo listing: {density['total_words']:.0f} words "
            "(assembler input; far-branch expansion and PCL padding "
            "account for the residual to the driver total)"
        )
    out.append("")
    out.append("## Micro benches (flash / RAM)")
    out.append("")
    out.append(
        "| bench | device | epic-cc flash | epic-cc RAM | "
        "vs baseline | XC8 flash | XC8 RAM | gap | XC8 measured |"
    )
    out.append("|---|---|---|---|---|---|---|---|---|")
    for row in micro_rows(sizes, baseline, xc8):
        gap = f"{row['gap']:+d}" if isinstance(row["gap"], int) else row["gap"]
        out.append(
            f"| {row['bench']} | {row['device']} | {row['flash']} | "
            f"{row['ram']} | {row['dflash']} / {row['dram']} | "
            f"{row['xflash']} | {row['xram']} | {gap} | {row['xdate']} |"
        )
    out.append("")
    out.append(
        "Negative gap means epic-cc is smaller. `vs baseline` is flash / "
        "RAM delta against the checked-in pins."
    )
    out.append("")
    out.append("## Whole programs (flash / RAM)")
    out.append("")
    out.append(
        "| program | device | epic-cc flash | epic-cc RAM | "
        "vs baseline | XC8 flash | XC8 RAM | gap | XC8 provenance |"
    )
    out.append("|---|---|---|---|---|---|---|---|---|")
    for row in program_rows(sizes, baseline, xc8):
        gap = f"{row['gap']:+d}" if isinstance(row["gap"], int) else row["gap"]
        out.append(
            f"| {row['name']} | {row['device']} | {row['flash']} | "
            f"{row['ram']} | {row['dflash']} / {row['dram']} | "
            f"{row['xflash']} | {row['xram']} | {gap} | {row['xprov']} |"
        )
    out.append("")
    if density is not None:
        for key in ("total_words", "categories"):
            if key not in density:
                raise SystemExit(f"size-report: density profile is missing {key!r}")
        menu = menu_entry(sizes)
        program = xc8.get("programs", {}).get("hal-pic18-menu-demo-18f4550")
        if program is None:
            raise SystemExit("size-report: snapshot has no menu-demo program row")
        sim = program.get("sim_xc8")
        out.append("## Menu-demo clusters (listing words)")
        out.append("")
        if sim:
            folded = sum(
                len(info.get("members", [])) - 1 + len(info.get("folded", []))
                for info in density.get("clusters", {}).values()
            )
            out.append(
                f"Driver total {menu['flash_words']} vs XC8 sim-variant "
                f"{sim['xc8']} "
                f"({menu['flash_words'] / sim['xc8']:.2f}x). "
                f"Folded single-caller callees are rolled into their caller "
                f"via `scripts/inline-map.py` ({folded} functions, the "
                f"same merge `docs/43-menu-triage-findings.md` does by hand)."
            )
            out.append("")
        out.append("| cluster | ours | XC8 | ratio | gap |")
        out.append("|---|---|---|---|---|")
        for row in cluster_rows(density, xc8):
            out.append(
                f"| {row['cluster']} | {row['ours']:.0f} | {row['xc8']} | "
                f"{row['ratio']} | {row['gap']} |"
            )
        out.append("")
        out.append("Top profiler categories on the same listing:")
        out.append("")
        total = density["total_words"]
        for cat, words in sorted(density["categories"].items(), key=lambda kv: -kv[1])[
            :8
        ]:
            out.append(f"- `{cat}`: {words:.0f} ({100 * words / total:.1f}%)")
    out.append("")
    out.append("## Regenerating")
    out.append("")
    out.append("```")
    out.append("make size-report")
    out.append("```")
    out.append("")
    out.append(
        "The target recompiles every ladder case through the real driver, "
        "rebuilds the menu-demo listing for the cluster table, refreshes "
        "the XC8 bench rows when the oracle image exists, and rewrites "
        "this file. XC8 whole-program and cluster rows stay snapshot "
        "quotes: they need different file sets and a manual `.map` join, "
        "so refreshing them is a deliberate act, not a side effect."
    )
    out.append("")
    return "\n".join(out)


def check_asm_inputs(sizes, path):
    """Fail loudly when MENU_DEMO_SRCS drifts from the ladder case."""
    want = menu_entry(sizes)["inputs"]
    got = pathlib.Path(path).read_text().split()
    missing = [w for w in want if not any(g.endswith("/" + w) for g in got)]
    extra = [g for g in got if not any(g.endswith("/" + w) for w in want)]
    if missing or extra:
        raise SystemExit(
            "size-report: MENU_DEMO_SRCS drifted from cases(): "
            f"missing {missing}, extra {extra}"
        )


def main(argv=None):
    parser = argparse.ArgumentParser(description="render SIZE_REPORT.md")
    parser.add_argument("--sizes", required=True, help="sizes JSON from the test")
    parser.add_argument("--density", help="density-profile --json with clusters")
    parser.add_argument("--baseline", required=True, help="size_baseline.toml")
    parser.add_argument("--xc8", required=True, help="XC8 snapshot JSON")
    parser.add_argument("--sha", required=True, help="commit sha stamped")
    parser.add_argument("--date", required=True, help="UTC date stamped")
    parser.add_argument("--oracle", required=True, help="oracle status stamped")
    parser.add_argument("--out", required=True, help="report path to write")
    parser.add_argument("--asm-inputs", help="built listing inputs, drift-checked")
    args = parser.parse_args(argv)
    sizes = load_json(args.sizes, "sizes")
    if args.asm_inputs:
        check_asm_inputs(sizes, args.asm_inputs)
    baseline = parse_baseline(pathlib.Path(args.baseline).read_text())
    xc8 = load_json(args.xc8, "XC8 snapshot")
    density = None
    if args.density:
        density = load_json(args.density, "density profile")
    pathlib.Path(args.out).write_text(render(args, sizes, density, baseline, xc8))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
