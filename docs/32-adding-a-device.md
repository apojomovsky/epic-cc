# 32: Adding a device (same core)

Status: **living reference**, not a one-off task plan. Re-read this whole
document before starting a device addition, and update it whenever a real
addition teaches something this version got wrong or left out. It documents
a path proven twice: PIC14's `p16f887` (`#85`) and PIC18's `p18f2550`
(`#224`, `#226`). Mirrors `epic-hal/docs/adding-a-device.md`'s shape and
its "living reference, not a checklist" posture.

This is **Path A only**: a new device on a core this compiler already
supports. See the decision point below before assuming that's what you
have.

## §0. Is this Path A?

Same core as an already-shipped device means *all* of these hold against
the closest existing sibling:

- Same addressing model (PIC14's banked GPR + common RAM, or PIC18's
  banked GPR + access bank; not some third scheme).
- Same interrupt vector count and architecture (PIC14: one vector,
  `0x0004`; PIC18 classic mode: two, `0x0008`/`0x0018`).
- Same instruction set (`Core::Pic14` and `Core::Pic14e` are *not* the
  same core despite the shared "PIC16" datasheet naming: Enhanced
  Mid-range has its own 49 extra instructions and its own addressing
  quirks, exactly why `Core::Pic14e` exists purely as a firewall today,
  see `crates/asm/tests/pic14e_firewall.rs`).

If any of those don't hold, this is **Path B: a new core**, a design
effort the size of `docs/29-pic18-port-design.md`, not a device addition.
Read that doc as the template (ISA/encoding survey, phased plan, a
verification gate per peripheral-equivalent, ending at a device TOML and
firewall removal). A PIC14E (Enhanced Mid-range) Path B design doc does
not exist yet; when it does, it belongs at `docs/33-*`.

Everything below assumes Path A.

## §1. Source the DFP and confirm device identity

1. Find the part's Device Family Pack. Free download from
   `https://packs.download.microchip.com/`; never install XC8 to get one,
   though a local pinned XC8 install (`docker/ci-toolchain/Dockerfile`'s
   tag) also carries the same DFP data under
   `/opt/microchip/xc8/v4.00/pic/packs/*/edc/*.PIC` and works as an
   alternate `--atdf` source.
2. **Never commit the pack.** `*.atdf`, `*.PIC`, `*.atpack` are gitignored
   on purpose (ADR-020). Only the TOML the generator produces from it is
   tracked; the pack itself is a build input you fetch, use, and discard.
3. Confirm the exact part number and package against the same datasheet
   the closest sibling already cites (check that sibling's
   `[provenance]` stanza). A part outside that datasheet's family is a
   separate identity question, flag it and ask rather than assume the
   sibling's facts carry over.

## §2. Generate the TOML

```bash
python3 scripts/gen-device.py <part> --atdf <path/to/PART.PIC> \
    --out crates/device/devices/<stem>.toml
```

`<part>` accepts any spelling the generator normalizes (`PIC16F887`,
`p16f887`, `16f887`); the output stem is always `p<suffix>`. This is the
whole "add a device" step in the common case: the generator reads
`ConfigFuseSector`'s `DCRDef`/`DCRFieldDef`/`AdjustPoint` structure
directly for PIC18 config fields, and `GPRDataSector` (split by
`TraditionalModeOnly`/`RegardlessOfMode`/`ExtendedModeOnly`) for RAM and
the access bank. See the module docstring in `scripts/gen-device.py` for
what each core's fields mean and how the alias tables normalize DFP
spelling to this repo's `EPIC_CONFIG` vocabulary.

**If the part shares an existing family's datasheet with a sibling
already in the registry** (e.g. a second PIC18F2455/2550/4455/4550
device), diff the generated field list against the sibling's TOML by
`(byte_offset, shift)` before moving on. Confirmed on `p18f2550` vs.
`p18f4550`: identical hardware bits can carry different DFP pack
spellings for the same field (`div1` vs. `osc1_pll2` for `cpudiv`, `boren`
vs. `bor` as the field name itself), and a genuinely absent field on the
smaller part (`icprt`, present on `p18f4550`, reserved on `p18f2550`) can
sit at the exact same bit position as a real field on the sibling. Don't
assume symmetry; read what the generator actually produced.

## §3. Cross-check against gputils, always

`gen-device.py` succeeding is necessary, not sufficient. **The DFP pack
itself can be wrong.** Confirmed the hard way: `Microchip.PIC18Fxxxx_DFP`
1.8.178's own `GPRDataSector` data for both `p18f2550` and `p18f4550`
lists only `gpr0`-`gpr3` (1024 bytes), while gputils' linker scripts and
the datasheet agree both parts have `gpr0`-`gpr7` (2048 bytes). Nothing
about that failure mode is specific to PIC18, or to this one pack.

Run the crate's cross-check gate before trusting anything the generator
wrote for RAM or flash size:

```bash
cargo test -p device --test gputils_crosscheck
```

It reads gputils' own `.lkr` (RAM banks, the BSR-free window) and probes
`gpasm` directly (flash size), the two facts a DFP has been wrong about
before. gputils ships in the dev image, so this cannot be skipped for
want of a download (ADR-021); it fails loudly rather than passing quietly
when the oracle is genuinely absent (`PIC8_ALLOW_NO_GPUTILS` +
`PIC8_UNVERIFIED_DEVICE_DATA`, two variables, is the explicit, logged
opt-out, never a silent skip).

If it disagrees with the generator's output, **gputils wins.** Correct
the TOML field by hand and leave a comment explaining why, citing the
`.lkr` file and the specific DFP defect (see `p18f2550.toml`'s
`ram_banks` comment for the shape this should take). This is a real,
attributable correction to bad upstream data, not a workaround: leaving
it silent is what would make the next person distrust the file.

## §4. Per-device sanity: what CI actually runs

```bash
bash scripts/sanity.sh <stem>
```

This is not a suggestion to approximate: it is *exactly* what
`.github/workflows/ci.yml`'s `devices-changed` job runs for every
`crates/device/devices/*.toml` touched in your diff, discovered
automatically via `git diff --name-only origin/master -- ...`. No file
needs editing to register a new device with CI (ADR-019's whole point:
one TOML plus a diff is the unit of change, never a hand-maintained
list). It covers: device schema/invariants, an empty-program alloc
sanity check, an 80-byte global placement check, an `asm` flash-bound
check, and the same thing a human would do first: compile `add.c` to HEX
for `<stem>` and cross-check the assembly with `gpasm -p <stem>`.

## §5. Drive a real `EPIC_CONFIG`, not just `add.c`

`add.c`-and-`gpasm` proves the device compiles. It does not prove every
declared fuse field actually resolves, and `resolve_config` requires an
explicit value for every field with no `default` (deliberately: geometry
and policy are different classes of fact, see `scripts/gen-device.py`'s
`SAFE_DEFAULTS` comment). Build a small fixture with an `EPIC_CONFIG(...)`
string covering every field the new TOML declares and confirm it compiles
without panicking:

```c
#include <epic-cc.h>
EPIC_CONFIG("field1=value1, field2=value2, ...");
int main(void) { return 0; }
```

Two failure shapes to expect and fix, not route around:

- **`unknown value ... for field ...`**: the DFP's cname for this device
  doesn't match a spelling already hardcoded somewhere it's matched
  literally (`fosc.rs::pic18_hz`'s `osc`/`cpudiv`/`plldiv` handling, for
  PIC18). Fix it in `scripts/gen-device.py`'s `PART_FIELD_ALIASES` /
  `PART_VALUE_ALIASES` (same shape as `PART_DEFAULTS`), normalizing the
  new device's cname to the vocabulary already shipped, not by editing
  the literal-matching Rust code and not by hand-editing the generated
  TOML's field values.
- **A field the backend cannot honor for every declared value on this
  core**: PIC18's `xinst` is the shipped example. This compiler never
  emits Extended Instruction Set / Indexed Addressing code for any PIC18
  device, so every PIC18 TOML needs `xinst` forced to
  `default = "off"` / `locked = "off"` (`Device::FuseField::locked`,
  `crates/device/src/config.rs`), or `EPIC_CONFIG("...xinst=on")` would
  silently produce firmware whose addressing the backend never actually
  emitted for. `gen-device.py` already forces this for `xinst`
  specifically on `core = "pic18"`; a new core-wide backend limitation
  like it belongs in the generator the same way, not as a per-device
  TOML edit.

## §6. Full verification and sign-off

- [ ] `cargo test -p device` passes (schema, provenance, the gputils
      cross-check from §3).
- [ ] `scripts/sanity.sh <stem>` passes (§4).
- [ ] A full `EPIC_CONFIG` string covering every declared field compiles
      without panicking (§5), and any generator/backend gap it surfaced
      is fixed at the source (gen-device.py's alias tables, or a
      documented `locked` field), not patched into this one TOML alone.
- [ ] Any DFP data defect found (§3) is corrected with a citing comment,
      not silently overridden.
- [ ] If the addition surfaced a literal in `crates/driver`,
      `crates/isel-pic18`, `crates/asm` or `crates/sim` that was
      transcribed from one specific device rather than derived from
      `Device` fields, either fix it (preferred, see `#234` for the
      shape: a hardcoded access-bank boundary threaded through as real
      device-derived state) or confirm and cite that it is genuinely
      core-wide architecture, not device data (see `#234`'s
      `PIC18_SFR_ACCESS_LO` for that shape too, and `fosc.rs`'s
      `INTOSC = 8 MHz` citation for how to write the confirmation).
      A second device on an existing core is the only thing that can
      surface this class of bug; take the check seriously even when
      `add.c` alone compiles cleanly.
- [ ] User has signed off on the final state before anything gets
      pushed.

## Known pitfalls (living list, update this when a new one is found)

| Pattern | Confirmed on | Where the full account lives |
|---|---|---|
| DFP pack understates RAM banks for a device the generator otherwise handles correctly | `p18f2550`, `p18f4550`, same DFP pack | `#231`, `#232`, `scripts/gen-device.py` module docstring |
| DFP pack revision spells config enum values (and even a field name) differently than an already-shipped sibling for the identical hardware bit | `p18f2550` vs. `p18f4550` | `#232`, `scripts/gen-device.py`'s `PART_FIELD_ALIASES`/`PART_VALUE_ALIASES` |
| A field the backend cannot honor for every silicon-legal value needs an explicit `locked` value, not just a `default` | PIC18 `xinst` | `#232`, `crates/device/src/config.rs`'s `locked` handling |
| Core-dispatched Rust code hardcoding a literal transcribed from the first device on a core instead of deriving it from `Device` fields | PIC18 access-bank boundary in `isel-pic18` | `#226`, `#234` |

This table is deliberately device-specific in its "confirmed on" column
and generic in its "pattern" column, the same posture
`epic-hal/docs/adding-a-device.md`'s own pitfalls appendix uses: a new
device should assume none of these are ruled out until actually checked,
not treat a short list as exhaustive.
