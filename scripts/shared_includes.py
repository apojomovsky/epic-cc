#!/usr/bin/env python3
"""Keep the vendored copies of epic-docs files honest (epic-docs#7).

The "Ways to use epic8" card is owned by apojomovsky/epic-docs and copied
into docs/snippets/ so README.src.md can include it. docs/snippets/SHARED-INCLUDES
records the hub commit the copy came from and the sha256 of the copy.

`check` runs from gen_docs_reference.py (make docs-gen and the CI docs job) and fails
when the copy no longer matches its recorded checksum, which means it was
edited here. It is local and never touches the network. `sync` fetches the
hub file at a commit and rewrites the copy and the record; only that
make target needs the network.
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]

HUB = "apojomovsky/epic-docs"
HUB_FILE = "includes/ways-to-use-epic8.md"
COPY = "docs/snippets/ways-to-use-epic8.md"
RECORD = "docs/snippets/SHARED-INCLUDES"
RAW_URL = "https://raw.githubusercontent.com/{hub}/{commit}/{path}"

_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
_DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
_SYNC_HINT = "run make sync-shared-includes REF=<epic-docs commit sha>"


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def parse_record(text: str) -> dict[str, str]:
    """The `key = value` lines of a SHARED-INCLUDES record; `#` lines skipped."""
    fields = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, sep, value = line.partition("=")
        if not sep:
            raise ValueError(f"malformed line {line!r}, want key = value")
        fields[key.strip()] = value.strip()
    return fields


def render_record(commit: str, digest: str) -> str:
    return (
        "# Vendored from apojomovsky/epic-docs. Do not edit the copy: change the\n"
        "# hub file, then run make sync-shared-includes REF=<commit>.\n"
        f"source = {HUB}:{HUB_FILE}\n"
        f"commit = {commit}\n"
        f"sha256 = {digest}\n"
    )


def check(root: pathlib.Path) -> list[str]:
    """Problems with the vendored copy; an empty list means it is clean."""
    record_path = root / RECORD
    copy_path = root / COPY
    if not record_path.is_file():
        return [f"{RECORD} missing; {_SYNC_HINT}"]
    try:
        fields = parse_record(record_path.read_text())
    except ValueError as exc:
        return [f"{RECORD}: {exc}"]
    problems = []
    if not _COMMIT_RE.match(fields.get("commit", "")):
        problems.append(f"{RECORD}: commit must be a full 40-character sha")
    digest = fields.get("sha256", "")
    digest_ok = bool(_DIGEST_RE.match(digest))
    if not digest_ok:
        problems.append(f"{RECORD}: sha256 must be 64 lowercase hex digits")
    if not copy_path.is_file():
        problems.append(f"{COPY} missing; {_SYNC_HINT}")
    elif digest_ok and sha256_hex(copy_path.read_bytes()) != digest:
        problems.append(
            f"{COPY} does not match its recorded sha256: it was edited here."
            f" Restore it with git checkout, or change {HUB}:{HUB_FILE}"
            f" and {_SYNC_HINT}"
        )
    return problems


def fetch_hub(commit: str) -> bytes:
    url = RAW_URL.format(hub=HUB, commit=commit, path=HUB_FILE)
    with urllib.request.urlopen(url, timeout=30) as resp:
        return resp.read()


def sync(root: pathlib.Path, commit: str, fetch=fetch_hub) -> str:
    """Copy the hub file at `commit` into the repo; returns its sha256."""
    if not _COMMIT_RE.match(commit):
        raise ValueError(
            f"REF must be a full 40-character epic-docs commit sha, got {commit!r}"
        )
    data = fetch(commit)
    if not data:
        raise ValueError(f"{HUB}:{HUB_FILE} at {commit} is empty")
    digest = sha256_hex(data)
    (root / COPY).parent.mkdir(parents=True, exist_ok=True)
    (root / COPY).write_bytes(data)
    (root / RECORD).write_text(render_record(commit, digest))
    return digest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check", help="fail when the copy was edited locally")
    sync_p = sub.add_parser("sync", help="refresh the copy from epic-docs")
    sync_p.add_argument(
        "--ref", required=True, help="epic-docs commit sha to copy from"
    )
    args = parser.parse_args(argv)
    if args.cmd == "check":
        problems = check(ROOT)
        for problem in problems:
            print(f"shared_includes: {problem}", file=sys.stderr)
        return 1 if problems else 0
    try:
        digest = sync(ROOT, args.ref)
    except (ValueError, OSError) as exc:
        print(f"shared_includes: {exc}", file=sys.stderr)
        return 2
    print(f"shared_includes: wrote {COPY} from {HUB}@{args.ref} (sha256 {digest})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
