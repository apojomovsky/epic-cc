#!/usr/bin/env python3
"""Flatten crates/device/devices/*.toml into a devices.json manifest for the
release bundle (epic-cc#416): a consumer outside this repo (epic-platformio's
board generator) needs the device list as data, without linking the device
crate or carrying a TOML parser of its own.

Stdlib only, and runs on the dev container's Python 3.10 (no tomllib), so it
reuses add_device.py's hand-rolled TOML parser rather than adding one.
"""

import importlib.util
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEVICES_DIR = ROOT / "crates" / "device" / "devices"

_spec = importlib.util.spec_from_file_location(
    "add_device", ROOT / "scripts" / "add_device.py"
)
add_device = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(add_device)


def build_manifest(devices_dir: pathlib.Path) -> list[dict]:
    """One entry per device TOML: name, core, and pack (the DFP a device's
    data was transcribed from, None for a device with no [provenance]
    section at all, the three originals that predate the stanza, or a
    tier="datasheet" entry with no pack, hand-transcribed straight from
    the datasheet instead)."""
    entries = []
    for path in sorted(devices_dir.glob("*.toml")):
        doc = add_device.parse_toml(path.read_text())
        entries.append(
            {
                "name": doc["name"],
                "core": doc["core"],
                "pack": doc.get("provenance", {}).get("pack"),
            }
        )
    return entries


def main():
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <out-file.json>")
    manifest = build_manifest(DEVICES_DIR)
    out_path = pathlib.Path(sys.argv[1])
    out_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"wrote {out_path} ({len(manifest)} devices)")


if __name__ == "__main__":
    main()
