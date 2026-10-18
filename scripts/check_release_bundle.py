#!/usr/bin/env python3
"""Fail unless every release zip ships both epic-cc and epic-cc-gdbserver.

A bundle without the debugger adapter breaks `pio debug` on that platform
(epic-cc#1336), so release.yml runs this on every downloaded zip before it
publishes. Windows zips (`*-windows.zip`) must carry `.exe` binaries.

usage: check_release_bundle.py <zip>...
"""

import pathlib
import sys
import zipfile

BINARIES = ("epic-cc", "epic-cc-gdbserver")


def expected_members(zip_path):
    """Zip member paths a bundle named `<root>.zip` must contain."""
    root = pathlib.Path(zip_path).stem
    suffix = ".exe" if root.endswith("-windows") else ""
    return [f"{root}/{name}{suffix}" for name in BINARIES]


def missing_members(zip_path):
    with zipfile.ZipFile(zip_path) as zf:
        # Compress-Archive on Windows may write backslash separators.
        names = {n.replace("\\", "/") for n in zf.namelist()}
    return [m for m in expected_members(zip_path) if m not in names]


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    failed = False
    for z in argv:
        missing = missing_members(z)
        if missing:
            failed = True
            print(f"{z}: missing {', '.join(missing)}", file=sys.stderr)
        else:
            print(f"{z}: ok")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
