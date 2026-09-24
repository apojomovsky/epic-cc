#!/usr/bin/env python3
"""Refresh the XC8 bench snapshot when the oracle image is present.

Compiles each size-bench/ program with `xc8-cc -mcpu=18f4550 -O2` in the
oracle image and reads the Memory Summary (program bytes halve to PIC18
words, data bytes are RAM bytes). Without the image, or without docker,
it copies the checked-in snapshot through unchanged and says so, so
`make size-report` never needs the licence-gated toolchain.

Whole-program and cluster rows are never touched here: they need
different file sets and a manual `.map` join, so refreshing them stays
a deliberate act recorded in `xc8_reference.md` and `docs/43`.
"""

import argparse
import datetime
import json
import pathlib
import re
import shutil
import subprocess

PROGRAM_RE = re.compile(r"Program space\s+used\s+\S+\s+\(\s*(\d+)\)")
DATA_RE = re.compile(r"Data space\s+used\s+\S+\s+\(\s*(\d+)\)")


def image_present(image):
    if shutil.which("docker") is None:
        return False
    run = subprocess.run(
        ["docker", "images", "-q", image], capture_output=True, text=True
    )
    return bool(run.stdout.strip())


def measure(repo, image, source):
    """Compile one bench in the oracle image, return (words, ram)."""
    cmd = [
        "docker",
        "run",
        "--rm",
        "-v",
        f"{repo}:/workspace",
        "-w",
        "/workspace",
        image,
        "xc8-cc",
        "-mcpu=18f4550",
        "-O2",
        str(source),
        "-o",
        "/tmp/size-refresh.hex",
    ]
    run = subprocess.run(cmd, capture_output=True, text=True)
    if run.returncode != 0:
        raise RuntimeError(f"xc8-cc failed on {source}:\n{run.stderr}")
    program = PROGRAM_RE.search(run.stdout)
    data = DATA_RE.search(run.stdout)
    if not program or not data:
        raise RuntimeError(f"no Memory Summary for {source}:\n{run.stdout}")
    words = (int(program.group(1)) + 1) // 2
    return words, int(data.group(1))


def main(argv=None):
    parser = argparse.ArgumentParser(description="refresh XC8 bench rows")
    parser.add_argument("--snapshot", required=True, help="checked-in snapshot")
    parser.add_argument("--out", required=True, help="snapshot to write")
    parser.add_argument("--bench-dir", required=True, help="size-bench/ directory")
    parser.add_argument(
        "--repo", required=True, help="repo root to mount into the image"
    )
    parser.add_argument(
        "--image",
        default="epic-cc-xc8-oracle:local",
        help="oracle image name",
    )
    args = parser.parse_args(argv)
    snapshot = json.loads(pathlib.Path(args.snapshot).read_text())
    if not image_present(args.image):
        pathlib.Path(args.out).write_text(json.dumps(snapshot, indent=2) + "\n")
        print(f"size-refresh-xc8: no {args.image}, quoting snapshot", flush=True)
        return 0
    today = datetime.date.today().isoformat()
    benches = {}
    for source in sorted(pathlib.Path(args.bench_dir).glob("bench-*.c")):
        words, ram = measure(args.repo, args.image, source)
        benches[source.stem] = {"flash": words, "ram": ram, "measured": today}
    snapshot["benches"] = benches
    snapshot["generated"] = today
    snapshot["image"] = args.image
    pathlib.Path(args.out).write_text(json.dumps(snapshot, indent=2) + "\n")
    print(f"size-refresh-xc8: refreshed {len(benches)} benches", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
