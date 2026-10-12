#!/usr/bin/env python3
"""Judge a PR's size and cycle ladder measurements against the base measurement.

The ladder tests run measure-only in CI and write their rows as JSON
(`BASELINE_ARTIFACT_DIR`), so a PR never edits the committed baseline TOMLs.
This script is the growth gate: CI measures the ladders on both the PR head
and the PR base commit in the same run, this script applies the
epic-tasks#306 rules to the two measurements, writes `baseline-delta.json`
and `baseline-delta.md`, and exits 1 when a human has to decide.

Rules (epic-tasks#306, epic-cc#1135):
- net win: no metric total grows and at least one shrinks;
- per row: flash grows at most 8 words, cycles at most 1%, RAM never grows;
- a row that regresses within those limits must be named in the PR body;
- a net loss, a row over its limit, RAM growth, or a removed row holds;
- a new row has no base to regress against, so it is reported and never holds.
"""

import argparse
import json
import pathlib
import re
import subprocess
import sys

# Same-directory import: CI runs the judge from a copy beside add_device.py.
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from add_device import parse_toml  # noqa: E402

BASELINE_PATHS = {
    "size": "crates/driver/tests/fixtures/size_baseline.toml",
    "cycle": "crates/driver/tests/fixtures/cycle_baseline.toml",
}
ARTIFACT_FILES = {"size": "size.json", "cycle": "cycle.json"}
METRICS = {
    "size": ("flash_words", "ram_bytes"),
    "cycle": ("cycles", "flash_words", "ram_bytes"),
}
FLASH_WORDS_LIMIT = 8
CYCLE_PERCENT_LIMIT = 1
ISR_ROW = re.compile(r"^(speed-isr-.+)-(latency|roundtrip)((?:@O2)?)$")


def entry_key(name, profile):
    # Mirrors `entry_key` in the ladder tests: -Os rows are bare names.
    return name if profile in (None, "Os") else f"{name}@{profile}"


def binary_of(key):
    m = ISR_ROW.match(key)
    return m.group(1) + m.group(3) if m else key


def index_entries(entries):
    out = {}
    for e in entries:
        key = entry_key(e["name"], e.get("profile"))
        if key in out:
            raise ValueError(f"duplicate ladder row {key}")
        out[key] = e
    return out


def judge(base, head, pr_body):
    """Compare base and head ladders; return the verdict.

    `base` and `head` map a ladder name to its list of entries. Rows are keyed
    by ladder and display key, so a name shared by two ladders stays two rows.
    A head row with no base row is new: reported in `new_rows`, never a hold.
    """
    reasons = []
    rows = []
    new_rows = []
    totals = {m: 0 for ms in METRICS.values() for m in ms}
    counted = set()
    regressions = []
    over_limit = []

    for ladder in METRICS:
        b_idx = index_entries(base.get(ladder, []))
        h_idx = index_entries(head.get(ladder, []))
        for key in sorted(set(b_idx) | set(h_idx)):
            if key not in b_idx:
                new_rows.append(f"{ladder} {key}")
                continue
            if key not in h_idx:
                reasons.append(f"{ladder} {key}: row removed from the ladder")
                continue
            b, h = b_idx[key], h_idx[key]
            for metric in METRICS[ladder]:
                bv, hv = b[metric], h[metric]
                delta = hv - bv
                # ISR latency and roundtrip rows are two readings of one binary.
                binary = (ladder, binary_of(key), metric)
                if metric == "cycles" or binary not in counted:
                    counted.add(binary)
                    totals[metric] += delta
                if delta == 0:
                    continue
                rows.append(
                    {
                        "ladder": ladder,
                        "key": key,
                        "device": h.get("device"),
                        "metric": metric,
                        "base": bv,
                        "head": hv,
                        "delta": delta,
                    }
                )
                if delta < 0:
                    continue
                if metric == "ram_bytes":
                    over_limit.append(f"{ladder} {key}: RAM grew {bv} -> {hv}")
                elif metric == "flash_words":
                    if delta > FLASH_WORDS_LIMIT:
                        over_limit.append(
                            f"{ladder} {key}: flash +{delta} words exceeds "
                            f"{FLASH_WORDS_LIMIT}"
                        )
                    else:
                        regressions.append(key)
                else:
                    if delta * 100 > bv * CYCLE_PERCENT_LIMIT:
                        over_limit.append(
                            f"{ladder} {key}: cycles +{delta} exceeds "
                            f"{CYCLE_PERCENT_LIMIT}% of {bv}"
                        )
                    else:
                        regressions.append(key)

    net_win = all(t <= 0 for t in totals.values()) and any(
        t < 0 for t in totals.values()
    )
    unexplained = sorted(k for k in set(regressions) if not names_row(pr_body, k))
    reasons.extend(over_limit)
    if regressions and not net_win:
        reasons.append("regressions without a net win across all rows")
    if unexplained:
        reasons.append(
            "regressing rows not named in the PR body: " + ", ".join(unexplained)
        )
    verdict = "hold" if reasons else "pass"
    return {
        "verdict": verdict,
        "reasons": reasons,
        "net_win": net_win,
        "totals": totals,
        "regressions": sorted(set(regressions)),
        "rows": rows,
        "new_rows": sorted(new_rows),
    }


def names_row(pr_body, key):
    # An explicit `delta-ok: <key>` line, not a substring: a prose mention
    # of the row must not pass the gate.
    pattern = rf"^[ \t]*delta-ok:[ \t]*{re.escape(key)}[ \t]*\r?$"
    return re.search(pattern, pr_body, re.MULTILINE) is not None


def render_markdown(result, base_desc="base"):
    icon = "pass" if result["verdict"] == "pass" else "HOLD, needs a human decision"
    lines = [f"## Baseline delta: {icon}", ""]
    for reason in result["reasons"]:
        lines.append(f"- {reason}")
    if result["reasons"]:
        lines.append("")
    for new in result.get("new_rows", []):
        lines.append(f"- {new}: new row (informational, nothing to regress against)")
    if result.get("new_rows"):
        lines.append("")
    t = result["totals"]
    lines.append(
        f"Totals (head minus {base_desc}): flash {t['flash_words']:+d} words, "
        f"RAM {t['ram_bytes']:+d} bytes, cycles {t.get('cycles', 0):+d}."
    )
    lines.append("")
    if result["rows"]:
        lines.append(f"| ladder | row | metric | {base_desc} | PR | delta |")
        lines.append("| --- | --- | --- | ---: | ---: | ---: |")
        for r in result["rows"]:
            lines.append(
                f"| {r['ladder']} | {r['key']} | {r['metric']} | {r['base']} "
                f"| {r['head']} | {r['delta']:+d} |"
            )
    else:
        lines.append(f"No row moved against {base_desc}.")
    return "\n".join(lines) + "\n"


def read_committed_baseline(base_sha, path):
    proc = subprocess.run(
        ["git", "show", f"{base_sha}:{path}"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        return None
    entries = parse_toml(proc.stdout).get("entry", [])
    # An empty table is no baseline: holding it keeps a wiped file from passing.
    return entries or None


def read_measured_base(base_artifacts, ladder):
    # The base ladder run writes the same JSON shape as the head run, so the
    # judge compares two measurements instead of a measurement and a TOML.
    if base_artifacts is None:
        return None
    path = base_artifacts / ARTIFACT_FILES[ladder]
    if not path.is_file():
        return None
    try:
        entries = json.loads(path.read_text())
    except json.JSONDecodeError:
        return None
    return entries or None


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base-sha", required=True, help="PR base commit")
    ap.add_argument("--artifacts", required=True, type=pathlib.Path)
    ap.add_argument(
        "--base-artifacts",
        type=pathlib.Path,
        help="dir holding the base SHA's measured size.json and cycle.json",
    )
    ap.add_argument("--pr-body", type=pathlib.Path, help="file holding the PR body")
    args = ap.parse_args(argv)

    pr_body = args.pr_body.read_text() if args.pr_body else ""
    base, head = {}, {}
    missing = []
    short = args.base_sha[:12]
    measured, fallback = [], []
    for ladder in METRICS:
        entries = read_measured_base(args.base_artifacts, ladder)
        if entries is not None:
            base[ladder] = entries
            measured.append(ladder)
        else:
            committed = read_committed_baseline(args.base_sha, BASELINE_PATHS[ladder])
            if committed is None:
                missing.append(
                    f"{ladder}: no base measurement and no baseline at "
                    f"{BASELINE_PATHS[ladder]} on {short}"
                )
                base[ladder] = []
            else:
                base[ladder] = committed
                fallback.append(ladder)
        artifact = args.artifacts / ARTIFACT_FILES[ladder]
        head[ladder] = json.loads(artifact.read_text())
        if not head[ladder]:
            missing.append(f"{ladder}: artifact has no rows")

    base_desc = f"base {short} (measured)"
    notes = []
    if fallback:
        notes.append(
            f"Base measurement missing for {', '.join(fallback)}: judged "
            f"against the committed TOML at {short} instead."
        )
        if measured:
            base_desc = f"base {short} (measured {', '.join(measured)}, committed {', '.join(fallback)})"
        else:
            base_desc = f"base {short} (committed TOML)"

    result = judge(base, head, pr_body)
    if missing:
        result["verdict"] = "hold"
        result["reasons"] = missing + result["reasons"]
    result["base"] = {"sha": args.base_sha, "measured": measured, "fallback": fallback}
    args.artifacts.mkdir(parents=True, exist_ok=True)
    (args.artifacts / "baseline-delta.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    markdown = render_markdown(result, base_desc=base_desc)
    if notes:
        markdown += "\n" + "\n".join(notes) + "\n"
    (args.artifacts / "baseline-delta.md").write_text(markdown)
    sys.stdout.write(markdown)
    return 0 if result["verdict"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
