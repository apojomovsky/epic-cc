# Size and map reporting design

**Status:** Design for `epic-cc#74` (CC-6, the reporting half)
**Parent:** `docs/31-ecosystem-integration-design.md` §3 CC-6

## Problem

The driver emits HEX only. PlatformIO expects a size report after every build, and a
user whose program does not fit needs a symbol-to-address map to decide what to cut.
The overlay allocator already holds every RAM fact; this is a reporting surface over
them, plus a flash count from the assembled words.

## Decisions

### D-1: Size goes to stderr by default; `--map <file>` writes the map

The size report prints to stderr on every successful hex build, unconditional. This
matches the D-4 config-words report precedent (`epic-cc: resolved configuration for
...` on stderr, `config_e2e.rs` asserts it) and PlatformIO parses build output. No
suppress flag in v1; the config report has none either.

`--map <file>` writes the allocator's address map. The file format is `alloc::map_text`'s
existing text contract, already tested and documented: `global <name> 0xNN`,
`local <func> <name> 0xNN` (the driver's `{func}::{name}` HashMap keys, split for
readability), and `const <name>` for flash-resident globals. Reusing it avoids a second
format to maintain. The map is written right after allocation, so it is available for
`--emit asm` and `--emit hex` alike.

### D-2: "RAM used" means the bytes of RAM the program's allocation occupies

Overlay allocation makes "used" non-obvious: a byte can be live in several frames, and
the physical span from GPR start to the highest frame end overcounts (it includes the
inter-bank SFR/gap regions). The report therefore defines used as the sum of:

- **per-bank high-water marks**: for each GPR bank, the highest allocated address
  (globals + locals, main and ISR contexts) minus the bank start, floored at 0. The
  allocator places sequentially from each bank start, so the high-water mark is the
  occupied bytes; the only holes are the 1-byte region-tail gaps an i16 leaves when it
  moves wholesale to the next bank, which the high-water mark conservatively includes.
- **the fixed common/access-bank region**: isel's scratch/retval/ISR-save layout.
  PIC14: scratch (1) + retval (4) = 5 bytes, + ISR save (9) = 14 with an ISR. PIC18:
  retval + flag (4) = 4 bytes, + ISR save (12) = 16 with an ISR. These are isel's
  layout constants, asserted in isel; the report documents them in one place.

The report states this definition on the RAM line, per the issue's requirement that the
report say what it means by used.

### D-3: Flash used = the program's assembled words, before config insertion

The PIC14 config word lives at word 0x2007, past the 8192-word flash; the driver's
current hex path resizes the word vec to include it, so `words.len()` would overcount.
The driver captures the program words before config insertion and reports that count.
`asm` gains `assemble_words(device, src) -> Vec<u16>` (program words, with the
flash-size assert `assemble_file_to_hex` already performs); `assemble_file_to_hex`
delegates to it, and the driver uses it for both the count and the hex emission.

### D-4: AllocLayout gains the RAM facts

`AllocLayout` adds `bank_used: Vec<u16>` (per-bank high-water marks, both contexts) and
`isr_bytes: u16` (the disjoint ISR region's span, 0 without an ISR). Both are computed in
`allocate` where the widths and bases live. The ISR region is reported as its own line
and is included in the bank totals, so the report says so rather than double-counting.

## Report format (stderr, after the config report)

```
epic-cc: program size for p16f877a:
  flash: 123/8192 words (1.5%)
  RAM: 123/368 bytes (33.4%) (overlay: a byte can be live in several frames; used = the bytes of RAM the program's allocation occupies)
    bank 0: 80/80 bytes
    bank 1: 27/80 bytes
    bank 2: 0/80 bytes
    bank 3: 0/80 bytes
    common: 14/16 bytes (fixed scratch/retval/ISR save)
    ISR region: 12 bytes (disjoint, after the main context, included in the bank totals)
```

PIC18 renders the same shape: one GPR bank line, and `fixed: N/16 bytes` for the
`fixed_retval` region instead of `common`.

## Files

- `crates/alloc/src/lib.rs`: `AllocLayout` + `allocate` gain `bank_used`/`isr_bytes`.
- `crates/asm/src/lib.rs`: `assemble_words`; `assemble_file_to_hex` delegates.
- `crates/driver/src/cli.rs`: `--map <file>` parsing + USAGE.
- `crates/driver/src/report.rs` (new): `render_size` + the fixed-region constants.
- `crates/driver/src/main.rs`: capture program words, write the map, print the report.
- Tests: `crates/driver/tests/size_map_e2e.rs` (report vs HEX and layout, map vs
  `map_text`), `crates/driver/tests/cli.rs` (`--map` parsing), `crates/alloc/tests`
  (bank_used/isr_bytes unit checks).

## Acceptance mapping

- Size line for a fixture, checked against the HEX and the allocator: `size_map_e2e.rs`
  counts program words in the emitted HEX and compares the report's flash line, and
  compares the RAM lines against the layout's `bank_used`/`isr_bytes` and the device
  totals.
- Map file for the same fixture, addresses matching the allocator's map:
  `size_map_e2e.rs` compares the written file to `alloc::map_text(&layout)`.
- PIO-1 can render size after a build: the stderr report is the parseable surface
  PlatformIO consumes; the format is stable and documented here.
