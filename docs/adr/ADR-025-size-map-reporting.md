# ADR-025 -- Size and map reporting (CC-6, reporting half)

**Status:** Accepted 2026-08-26<br>
**Decides:** `epic-cc#74`<br>
**Parent:** `docs/31-ecosystem-integration-design.md` (§3 CC-6); `docs/superpowers/specs/2026-08-26-size-map-reporting-design.md`

## Decision

* The driver prints a **size report to stderr after every hex build**,
  unconditional. The report covers flash words used out of the device
  total and RAM bytes used out of total, split by region: banked GPR per
  bank, the fixed common/access-bank region, and the disjoint ISR region.
* **"RAM used" is defined on the report line**: the bytes of RAM the
  program's allocation occupies, i.e. the per-bank high-water marks from
  the overlay layout plus the fixed scratch/retval/ISR-save region isel
  reserves. Overlay makes "used" non-obvious (a byte can be live in
  several frames), so the report states the definition rather than making
  a reader guess.
* `--map <file>` writes the **symbol-to-address map**: `global <name>
  0xNN`, `const <name>` (flash-resident, no RAM address), and `local
  <key> 0xNN` where `<key>` is the driver's `{func}::{name}` HashMap key
  (the AGENTS.md contract), sorted deterministically.
* `alloc::AllocLayout` gains `bank_used: Vec<u16>` (per-bank high-water
  bytes, main and ISR contexts) and `isr_bytes: u16` (the disjoint ISR
  region's span). `asm` gains `assemble_words(device, src) -> Vec<u16>`
  (program words with the flash-fit assert); `assemble_file_to_hex`
  delegates to it.
* **Flash used is the program's assembled words before config insertion**:
  the PIC14 config word lives past the flash ceiling (0x2007 on the
  877A), so the hex vec is resized to include it and its length would
  overcount.

## Rationale

* PlatformIO parses build output for size after every build (PIO-1's
  builder), and the config-words report already goes to stderr
  unconditionally (D-4 precedent, `config_e2e.rs` asserts it). Size on
  stderr by default is the same class of report, so no flag is needed to
  ask for it.
* The map is a file a user opens when a program does not fit; it is the
  natural companion to the diffable text boundaries the pipeline already
  has. Reusing the driver's `{func}::{name}` keys keeps one symbol
  spelling across the compiler and its artifacts.
* The overlay allocator already holds every RAM fact (the issue's own
  framing: "this is a reporting surface over facts it holds"), so the
  reporting adds no new analysis, only the two fields that make the facts
  public.
* The high-water-mark definition is honest for sequential allocation:
  values are placed from each bank start, so the highest allocated
  address is the occupied bytes; the only holes are the 1-byte
  region-tail gaps an i16 leaves when it moves wholesale to the next
  bank, which the high-water mark conservatively includes.

## Alternatives rejected

* **Size only on request.** PlatformIO expects size after every build and
  the config report already owns stderr unconditionally; a flag would
  force PIO-1 to remember to pass it.
* **Map file as a new format.** `alloc::map_text` is the established text
  contract, but it splits locals into `local <func> <name>`; the issue
  and AGENTS.md name the `{func}::{name}` key as the contract, so the map
  file uses it directly.
* **Flash used from the hex vec length.** The PIC14 config word at 0x2007
  (past the 8192-word flash) would inflate the count; capturing the
  program words before config insertion is exact.

## Known trade-offs

* **Per-bank high-water marks include region-tail holes.** A program whose
  i16 local crosses a bank boundary reports that bank as full (the hole
  byte counts). This is the conservative fit signal a user cutting RAM
  needs, not a reallocation promise.
* **PIC18 fixed-region reporting uses the access bank** (the
  `fixed_retval` reservation is a policy slice of it), so the fixed line
  shows a 16-byte used slice of the 96-byte access bank.

## Revisit if

* A consumer (PIO-1's builder) needs a machine-parseable size line with a
  stricter grammar than the prose-plus-numbers format.
* isel's fixed scratch/retval/ISR-save layout changes, invalidating the
  report's fixed-region constants (the constants mirror isel's asserts).
