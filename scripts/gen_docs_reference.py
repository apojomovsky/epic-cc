#!/usr/bin/env python3
"""Generate the reference pages (epic-cc#1280): the device matrix, the CLI
reference, and the README rendered from its source. Stdlib only, runs on
the dev container's Python 3.10, reuses add_device.py's hand-rolled TOML
parser like gen_devices_manifest.py does.

Outputs, all rewritten in place:
- docs/reference/devices.md: one row per crates/device/devices/*.toml
  (the parts --target accepts), with family, core, memory sizes, and a
  status read from docs/reference/device-status.toml (default
  "experimental"). The catalog (crates/device/catalog/parts.toml) is the
  cross-check: a shipped device missing there, or disagreeing on core or
  flash, fails the run instead of publishing a silent fork.
- docs/reference/cli.md: the real `--help` output of the built binary
  plus every subcommand's, captured by running it (pass --binary).
- docs/user/devices/index.md, docs/user/devices/families/<slug>.md and
  docs/user/compiler/cli-flags.md (epic-cc#1297): the pages the epic-docs
  hub serves, built from the same rows and help capture. One family page
  per (marketing family, core) pair, facts only from the catalog, the
  device TOMLs and the status file. docs/user/ is wiped of stale family
  pages on every run, so a removed family cannot linger.
- README.md: README.src.md with every `--8<-- "file"` include resolved
  (GitHub renders README.md as-is, so includes expand at generation
  time). An include may carry a `:start:end` line suffix (1-based,
  inclusive); anything else fails loudly.

RAM bytes counts the compiler-visible GPR windows (ram_banks plus
common_ram), not the datasheet total: mirrored and reserved bytes are
outside what alloc can hand out.
"""

import argparse
import importlib.util
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEVICES_DIR = ROOT / "crates" / "device" / "devices"
CATALOG = ROOT / "crates" / "device" / "catalog" / "parts.toml"
STATUS_FILE = ROOT / "docs" / "reference" / "device-status.toml"
DEVICES_MD = ROOT / "docs" / "reference" / "devices.md"
CLI_MD = ROOT / "docs" / "reference" / "cli.md"
USER_DIR = ROOT / "docs" / "user"
USER_DEVICES_DIR = USER_DIR / "devices"
USER_FAMILIES_DIR = USER_DEVICES_DIR / "families"
USER_CLI_MD = USER_DIR / "compiler" / "cli-flags.md"
HUB = "https://apojomovsky.github.io/epic-docs"
README_SRC = ROOT / "README.src.md"
README_MD = ROOT / "README.md"

GENERATED_BY = "scripts/gen_docs_reference.py"

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import shared_includes  # noqa: E402

_spec = importlib.util.spec_from_file_location(
    "add_device", ROOT / "scripts" / "add_device.py"
)
add_device = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(add_device)

CORE_LABELS = {
    "pic-baseline": "PIC12 baseline",
    "pic14": "PIC14",
    "pic14e": "PIC14E",
    "pic18": "PIC18",
}

STATUSES = ("supported", "experimental", "planned")
DEFAULT_STATUS = "experimental"

_FAMILY_RE = re.compile(r"^p(\d+)(lf|hv|f)")
_INCLUDE_RE = re.compile(r"^[ \t]*--8<--\s+[\"']([^\"']+)[\"']\s*$")
_COMMANDS_HEADER_RE = re.compile(r"(?i)^\s*(?:sub)?commands:\s*$")
_MAX_INCLUDE_DEPTH = 10


def family_of(name: str) -> str:
    """Marketing family from a canonical stem: p16f877a -> PIC16F."""
    m = _FAMILY_RE.match(name)
    if not m:
        raise ValueError(f"no family prefix in device name {name!r}")
    return f"PIC{m.group(1)}{m.group(2).upper()}"


def display_of(name: str) -> str:
    """Human-facing part name: p16f877a -> PIC16F877A."""
    return f"PIC{name[1:].upper()}"


def ram_bytes(doc: dict) -> int:
    """Compiler-visible GPR bytes: every ram_banks window plus common_ram."""
    total = sum(end - start + 1 for start, end in doc.get("ram_banks", []))
    common = doc.get("common_ram")
    if common is not None:
        total += common[1] - common[0] + 1
    return total


def load_status(path: pathlib.Path) -> dict:
    """Part stem -> status. Unknown statuses fail; unknown parts fail at
    row build time, so a typo never silently renders as experimental."""
    doc = add_device.parse_toml(path.read_text())
    status = {}
    for name, value in doc.items():
        if name in STATUSES:
            raise ValueError(f"{path}: {name!r} is a status, not a part")
        if value not in STATUSES:
            raise ValueError(f"{path}: {name!r} has unknown status {value!r}")
        status[name] = value
    return status


def load_catalog(path: pathlib.Path) -> dict:
    """Catalog name -> (display, core, flash_words)."""
    rows = {}
    for part in add_device.parse_toml(path.read_text()).get("part", []):
        rows[part["name"]] = (part["display"], part["core"], part["flash_words"])
    return rows


def build_device_rows(devices_dir, catalog, status) -> list:
    """One row per shipped device TOML, validated against the catalog."""
    for name in status:
        if not sorted(devices_dir.glob(f"{name}.toml")):
            raise ValueError(f"status file names unknown device {name!r}")
    rows = []
    for path in sorted(devices_dir.glob("*.toml")):
        doc = add_device.parse_toml(path.read_text())
        name = doc["name"]
        core = doc["core"]
        if core not in CORE_LABELS:
            raise ValueError(f"{path}: unknown core {core!r}")
        if name not in catalog:
            raise ValueError(f"{path}: {name!r} missing from the catalog")
        display, cat_core, cat_flash = catalog[name]
        if display != display_of(name):
            raise ValueError(f"{path}: display {display!r} is not {name!r}")
        if cat_core != core or cat_flash != doc["flash_words"]:
            raise ValueError(f"{path}: catalog disagrees on core or flash")
        rows.append(
            {
                "name": name,
                "part": display,
                "family": family_of(name),
                "core": CORE_LABELS[core],
                "flash": doc["flash_words"],
                "ram": ram_bytes(doc),
                "status": status.get(name, DEFAULT_STATUS),
            }
        )
    return rows


def render_devices(rows) -> str:
    lines = [
        f"<!-- generated by {GENERATED_BY}, do not edit -->",
        "# Device matrix",
        "",
        "Every part `epic-cc --target` accepts, with its core, memory sizes,",
        "and support status.",
        "",
        "| Part | Family | Core | Flash words | RAM bytes | Status |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for r in rows:
        lines.append(
            f"| {r['part']} | {r['family']} | {r['core']} "
            f"| {r['flash']} | {r['ram']} | {r['status']} |"
        )
    return "\n".join(lines) + "\n"


def load_catalog_facts(path: pathlib.Path) -> dict:
    """Catalog name -> the raw part table (eeprom, lifecycle, pdf, page)."""
    return {
        p["name"]: p for p in add_device.parse_toml(path.read_text()).get("part", [])
    }


def family_slug(family: str, core: str) -> str:
    """PIC16F + PIC14E -> pic16f-pic14e, stable and URL safe."""
    return re.sub(r"[^a-z0-9]+", "-", f"{family} {core}".lower()).strip("-")


def group_families(rows) -> dict:
    """(family, core) -> rows, ordered by family then core label."""
    groups = {}
    for r in rows:
        groups.setdefault((r["family"], r["core"]), []).append(r)
    return dict(sorted(groups.items()))


def _datasheet(facts) -> str:
    pdf = facts.get("pdf")
    return f"[datasheet]({pdf})" if pdf else "not listed"


def _eeprom(facts) -> str:
    return str(facts["eeprom_bytes"]) if "eeprom_bytes" in facts else "none"


def render_user_devices(rows) -> str:
    """Hub device matrix: the reference matrix with each family linked."""
    lines = [
        f"<!-- generated by {GENERATED_BY}, do not edit -->",
        "# Device matrix",
        "",
        "This page lists every part `epic-cc --target` accepts, with its core,",
        "memory sizes and support status, for anyone choosing a chip or checking",
        "whether theirs works. Each family name links to its own page.",
        "",
        "RAM bytes counts what the compiler can allocate, not the datasheet total.",
        "Flash is counted in words. For what each status means, see",
        f"[What's in 1.0]({HUB}/status/).",
        "",
        "| Part | Family | Core | Flash words | RAM bytes | Status |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for r in rows:
        slug = family_slug(r["family"], r["core"])
        lines.append(
            f"| {r['part']} | [{r['family']}](families/{slug}.md) | {r['core']} "
            f"| {r['flash']} | {r['ram']} | {r['status']} |"
        )
    return "\n".join(lines) + "\n"


def render_family(family: str, core: str, rows, facts) -> str:
    """One family page from templates/device-family.md.tmpl in epic-docs:
    the fact box is generated, the notes only point at pages that own the
    facts."""
    lines = [
        f"<!-- generated by {GENERATED_BY}, do not edit -->",
        f"# {family} ({core})",
        "",
        f"This page lists the {family} parts that use the {core} core and that",
        "`epic-cc` can target, for anyone picking a chip in this family.",
        "",
        "| Part | Flash words | RAM bytes | EEPROM bytes | Status | Lifecycle | Datasheet |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for r in rows:
        f = facts[r["name"]]
        lines.append(
            f"| {r['part']} | {r['flash']} | {r['ram']} | {_eeprom(f)} "
            f"| {r['status']} | {f.get('lifecycle', 'not listed')} | {_datasheet(f)} |"
        )
    lines += [
        "",
        "## Notes",
        "",
        "- RAM bytes counts what the compiler can allocate, not the datasheet total.",
        "- Parts with `experimental` status compile, but are not yet checked by the",
        "  project's own test matrix.",
        "- The full list of parts is in the [device matrix](../index.md).",
        "",
        "## Status",
        "",
        f"See [What's in 1.0]({HUB}/status/) for this family's support level.",
    ]
    return "\n".join(lines) + "\n"


def render_user_cli(sections) -> str:
    lines = [
        f"<!-- generated by {GENERATED_BY}, do not edit -->",
        "# CLI flags",
        "",
        "This page lists every flag the `epic-cc` command accepts, captured from",
        "the built binary, for anyone running the compiler directly.",
        "",
    ]
    for title, help_text in sections:
        lines += [f"## `{title}`", "", "```text", help_text.rstrip("\n"), "```", ""]
    return "\n".join(lines).rstrip("\n") + "\n"


def write_user_pages(rows, facts, sections) -> list:
    """Write docs/user/{devices,compiler} pages; returns the paths written."""
    for stale in USER_FAMILIES_DIR.glob("*.md"):
        stale.unlink()
    USER_FAMILIES_DIR.mkdir(parents=True, exist_ok=True)
    USER_CLI_MD.parent.mkdir(parents=True, exist_ok=True)
    written = []
    for (family, core), group in group_families(rows).items():
        path = USER_FAMILIES_DIR / f"{family_slug(family, core)}.md"
        path.write_text(render_family(family, core, group, facts))
        written.append(path)
    index = USER_DEVICES_DIR / "index.md"
    index.write_text(render_user_devices(rows))
    USER_CLI_MD.write_text(render_user_cli(sections))
    return written + [index, USER_CLI_MD]


def capture_help(binary: str, args: list) -> str:
    proc = subprocess.run([binary, *args], capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise RuntimeError(
            f"{binary} {' '.join(args)} exited {proc.returncode}: {proc.stderr.strip()}"
        )
    return proc.stdout


def discover_subcommands(top_help: str) -> list:
    """First words under a `Commands:` section header, if the help has one.
    Today's binary has no subcommands, so this finds none; it exists so a
    future subcommand's `--help` is picked up without touching this script.
    """
    found = []
    in_commands = False
    for line in top_help.splitlines():
        if _COMMANDS_HEADER_RE.match(line):
            in_commands = True
            continue
        if not in_commands:
            continue
        if not line.strip():
            break
        m = re.match(r"^\s{2,}(\S+)\s{2,}\S", line)
        if m:
            found.append(m.group(1))
    return found


def build_cli_sections(binary: str) -> list:
    top = capture_help(binary, ["--help"])
    sections = [("epic-cc --help", top)]
    for sub in discover_subcommands(top):
        sub_help = capture_help(binary, [sub, "--help"])
        if sub_help and sub_help != top:
            sections.append((f"epic-cc {sub} --help", sub_help))
    return sections


def render_cli(sections) -> str:
    lines = [
        f"<!-- generated by {GENERATED_BY}, do not edit -->",
        "# CLI reference",
        "",
        "The compiler command line, captured from the built binary.",
        "",
    ]
    for title, help_text in sections:
        lines += [f"## `{title}`", "", "```text", help_text.rstrip("\n"), "```", ""]
    return "\n".join(lines).rstrip("\n") + "\n"


def expand_includes(text: str, base_dir: pathlib.Path, stack=()) -> str:
    """Resolve every `--8<-- "file[:start:end]"` line, relative to the
    file carrying the directive. Included files resolve recursively;
    missing files, unknown regions, cycles, and excess depth fail."""
    if len(stack) > _MAX_INCLUDE_DEPTH:
        raise ValueError(f"include depth exceeds {_MAX_INCLUDE_DEPTH}: {stack}")
    out = []
    for lineno, line in enumerate(text.split("\n"), start=1):
        m = _INCLUDE_RE.match(line)
        if not m:
            out.append(line)
            continue
        spec = m.group(1)
        target, _, region = spec.partition(":")
        inc_path = (base_dir / target).resolve()
        if not inc_path.is_file():
            raise ValueError(f"line {lineno}: include {target!r} not found")
        if inc_path in stack:
            raise ValueError(f"line {lineno}: include cycle at {target!r}")
        content = inc_path.read_text().split("\n")
        if content and content[-1] == "":
            content.pop()
        if region:
            try:
                start_s, end_s = region.split(":")
                start, end = int(start_s), int(end_s)
            except ValueError:
                raise ValueError(
                    f"line {lineno}: bad region {region!r}, want file:start:end"
                ) from None
            if not 1 <= start <= end <= len(content):
                raise ValueError(
                    f"line {lineno}: region {region!r} out of range "
                    f"(file has {len(content)} lines)"
                )
            content = content[start - 1 : end]
        out.extend(
            expand_includes(
                "\n".join(content), inc_path.parent, (*stack, inc_path)
            ).split("\n")
        )
    return "\n".join(out)


def render_readme() -> str:
    expanded = expand_includes(
        README_SRC.read_text(), README_SRC.parent, (README_SRC.resolve(),)
    )
    header = (
        f"<!-- generated by {GENERATED_BY}, do not edit; edit README.src.md instead -->"
    )
    return f"{header}\n{expanded}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, help="built epic-cc binary")
    args = parser.parse_args()

    problems = shared_includes.check(ROOT)
    if problems:
        for problem in problems:
            print(f"gen_docs_reference: {problem}", file=sys.stderr)
        return 1

    catalog = load_catalog(CATALOG)
    status = load_status(STATUS_FILE)
    rows = build_device_rows(DEVICES_DIR, catalog, status)
    sections = build_cli_sections(args.binary)
    DEVICES_MD.write_text(render_devices(rows))
    CLI_MD.write_text(render_cli(sections))
    write_user_pages(rows, load_catalog_facts(CATALOG), sections)
    README_MD.write_text(render_readme())
    print(f"wrote {DEVICES_MD}, {CLI_MD}, {README_MD}, and docs/user pages")
    return 0


if __name__ == "__main__":
    sys.exit(main())
