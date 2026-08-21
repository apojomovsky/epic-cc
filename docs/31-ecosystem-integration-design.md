# 31: epic-cc / epic-hal ecosystem integration design

> **Approval status:** parity scope, compatibility split, fuse surface, toolchain role
> and sequencing **approved by the user on 2026-08-20**.
> This document is the **decomposition of record**. It holds the map and the decisions
> that constrain every piece; it is not an implementation plan. Each sub-project below
> earns its own design and plan cycle.

**Goal:** epic-cc becomes the default toolchain for [epic-hal](https://github.com/apojomovsky/epic-hal),
and the two ship together as a fully open-source 8-bit PIC ecosystem, reachable from
PlatformIO and from a one-line installer that touches nothing on Microchip's servers.

**Definition of done:** every `epic-*` module and both `pic16f87xa-hal` and
`pic18fxx5x-hal` build and pass their gates under epic-cc, per module, on both families.
`pic16f193x-hal` (enhanced mid-range) is explicitly out of scope: epic-cc has no backend
for that core.

---

## 1. Where the two repos actually stand

Established by inspection on 2026-08-20, with the locations that prove each claim.

### epic-cc

| Claim | Evidence |
|---|---|
| **No multi-translation-unit compilation.** `wholeprog` is a validating pass-through, and the driver reads one positional argument. | `crates/wholeprog/src/lib.rs:5`, `crates/driver/src/main.rs:19` |
| **No include paths, no defines, no device selection.** The clang invocation is a fixed argument list and the device is hard-coded. | `crates/driver/src/main.rs:21`, `:38-50` |
| **No configuration words.** Nothing in the workspace mentions `__CONFIG`, `CONFIG1`, or fuses. | grep over `crates/`, empty |
| **No standard headers.** clang runs `-nostdinc -ffreestanding`, and the repo ships no headers of its own. | `crates/driver/src/main.rs:44-45`; no `.h` outside `vendor/` |
| **No inline assembly.** `irparse` has no handling for LLVM's `asm` construct. | `crates/irparse/src/lib.rs`, no match |
| **Absolute placement is half-built.** `ir::Global` carries the address and the IR text format prints it, but the parser never populates it, because no C-level syntax feeds it. | `crates/ir/src/lib.rs:137`, `:213`; `crates/irparse/src/lib.rs:1054` |
| **The PIC14 backend is feature-complete.** Interrupts, `long`, soft-float, `const` tables, structs, pointers and multi-page all have e2e fixtures with committed HEX. | `crates/driver/tests/fixtures/` |
| **The PIC18 backend is at P3.** P4 (`const` via `TBLRD`) is planned and unlanded; P5-P8 remain. | `docs/29-pic18-port-design.md` §4 |

### epic-hal

| Claim | Evidence |
|---|---|
| **The SFR layer is already compiler-neutral.** Every SFR access is a volatile dereference of a literal address, which is the same `inttoptr` shape epic-cc's fixtures already compile. | `pic18fxx5x-hal/include/target/pic18_platform.h`; compare `crates/driver/tests/fixtures/interrupt.c` |
| **The environment split already anticipates a fourth toolchain.** `src/core|peripherals` are shared; `src/target|sim|mdb` are selected at build time, never by `#ifdef`. | `pic18fxx5x-hal/src/`, `AGENTS.md` |
| **Two of four XC8-isms are already behind macros.** `EPIC_PLACE` wraps `__at` (36 sites), `EPIC_WEAK` is a no-op. | `pic18fxx5x-hal/include/target/pic18_platform.h:19,16` |
| **Two are not.** `#pragma config` (117 lines) and `__interrupt(high_priority|low_priority)` (6 sites) are written raw. | `pic18fxx5x-hal/src/target/pic18_isr_vector.c` and family equivalents |
| **`epic-math` is the dialect problem.** Roughly 400 XC8-dialect `asm()` statements across `src/pic16/` and `src/pic18/`, operating on `__at`-pinned scratch globals. | `epic-math/src/pic16/epic_math_mul.c` (83), `src/pic16/epic_math_div.c` (75), and six more |
| **`epic-math` already has a portable C path.** `src/host/` implements the same four modules in C for the host-simulation build. | `epic-math/src/host/` |
| **The hand asm loses to an optimizing compiler.** On the 4550 it is beaten on every operation at `-O2`/`-O3`; on the 877A it is beaten on mul and div at `-O3`. It exists to beat XC8's licence-gated optimizer. | `docs/experiments/math-cycle-benchmark/README.md` |
| **Getting started is gated on two Microchip downloads**, and CI pulls a private image because the EULA forbids redistributing XC8. | `README.md` getting-started, `AGENTS.md` CI section |
| **`epic-serial` retargets XC8's `printf`.** | `epic-serial/src/epic_serial.c:207` |

The load-bearing conclusion: **the HAL is far better positioned for this than the compiler
is.** epic-hal needs macro mappings and a build backend. epic-cc needs to grow from
"compiles a single fixture into HEX" into "compiles a real multi-file project onto real
silicon," and that is where the work is.

---

## 2. Decisions

### D-1: Parity means two families, per module

**Decision.** Every `epic-*` module, on `pic16f87xa-hal` and `pic18fxx5x-hal`.
`pic16f193x-hal` is out of scope.

**Rationale.** epic-cc's two device profiles, `PIC16F877A` and `PIC18F4550`
(`crates/device/src/lib.rs:33,45`), line up exactly with those two HAL families.
Covering the third would mean a third instruction-selection backend for the enhanced
mid-range core, which is a port on the scale of the PIC18 one and is a separate decision.

**"Per module" is load-bearing.** The 877A has 368 bytes of RAM and 8K words of flash.
Parity means each module builds and passes its gate, never that the whole shelf links into
one image. This matches how epic-hal already builds, so nothing changes; it is recorded
here so it is not relitigated later as a regression.

### D-2: Split the XC8 compatibility surface by category

**Decision.** epic-cc natively owns the things every PIC compiler must have, with epic-cc's
own spelling: absolute placement and configuration words (interrupt-vector marking turned
out to need no epic-cc-specific spelling at all; see D-8). epic-hal owns what is genuinely
dialect-specific, which is `epic-math`'s assembly.

epic-cc ships a header, `epic-cc.h`, defining its spellings. epic-hal's platform-header
seam maps its `EPIC_*` macros onto them, one line per item:

```c
/* pic18fxx5x-hal/include/epiccc/pic18_platform.h  (new, alongside target/ and host/) */
#define EPIC_PLACE(a)   EPIC_AT(a)
```

**Superseded by D-8:** the `EPIC_ISR_HIGH`/`EPIC_ISR_LOW` mapping shown above at the time of
this decision turned out to be unnecessary. See D-8.

**Rationale.** Placement and config words are not concessions to XC8. Without them epic-cc can
only compile test fixtures that fake SFRs with integer-to-pointer casts and never boot a
physical part. epic-cc owes both regardless of epic-hal, so they are epic-cc API, documented
as such.

The header must exist in some repo because clang is the front end and rejects unknown
attributes; the one thing it forwards verbatim is `__attribute__((section("...")))`, on globals
and on functions alike (probed, see §5), so every option reduces to a macro over section names. Putting the header in epic-cc is what makes
epic-cc usable by someone who is not using epic-hal, which the PlatformIO platform requires
to be true anyway.

**Rejected, epic-hal abstracts everything and epic-cc stays neutral.** Produces nearly
identical code, but leaves epic-cc with no documented API of its own, so a standalone user has
to reverse-engineer the section-name convention.

**Rejected, epic-cc mimics XC8's surface so XC8 projects are drop-in.** Attractive for
adoption and unaffordable in practice. `#pragma config` names are per-device and come from
Microchip's device packs, so epic-cc would ship its own table of every part and be judged
against a black box. XC8-dialect `asm()` has no operand constraints, so the compiler cannot
know what a statement clobbers; XC8 gets away with it by owning the whole pipeline. Matching
it would require inferring behaviour from a binary the project rules forbid inspecting.

### D-3: Inline assembly is in scope, delivered as a four-rung ladder

**Decision.** epic-cc supports inline assembly, because a standalone compiler must. It is
delivered in four rungs, in this order, and the order is chosen so the cheapest rungs cover
the most real usage:

1. **Naked functions** and **`.asm` compile-unit inputs**: whole hand-written routines.
2. **Intrinsics** for the single-instruction cases: `nop`, `sleep`, `clrwdt`, interrupt
   enable and disable, rotates, hardware multiply.
3. **Opaque statement-level assembly**: an assembly string interleaved with C statements,
   no operands.
4. **Memory-operand statement-level assembly**: the same, able to name C locals.

Rungs 3 and 4 are the only ones that can appear inside a function body; rungs 1 and 2 cover
whole routines and single instructions respectively. Rung 4 is speculative and is built only
if rungs 1 to 3 prove insufficient in practice.

**Empirical basis.** Every claim below was probed against the pinned clang 20.1.8 in the dev
image on 2026-08-20, using the exact flags in `crates/driver/src/main.rs:38-50`.

| Form written in C | What clang 20.1.8 puts in the `.ll` |
|---|---|
| File-scope `asm("...")` | `module asm "..."`, verbatim, at module top |
| `asm volatile("movwf _g")` in a function | `call void asm sideeffect "  movwf _g", ""()`, correctly ordered against surrounding volatile accesses |
| `asm volatile(... : "+m"(x) : "m"(y))` | `call void asm sideeffect "... $0 ... $1 ...", "=*m,*m,*m"(ptr %1, ptr %2, ptr %1)` |
| `... ::: "memory", "cc"` | `"~{memory},~{cc}"`, accepted |
| `__attribute__((naked))` | `naked noinline` attribute, assembly body, `unreachable` |

#### Rung 1: naked functions and `.asm` inputs

```c
EPIC_NAKED void epic_mul8(void) {
    asm("movf    _mul_a, w");
    asm("mulwf   _mul_b");
    asm("movff   PRODL, _mul_lo");
    asm("movff   PRODH, _mul_hi");
    asm("return");
}
```

**Why naked functions rather than a file-scope `asm` blob**, which would be marginally cheaper:
a naked function is a *real function*. `callgraph` sees it, so it counts toward the eight-level
hardware stack depth check. `alloc` sees it, so storage it owns can be overlaid against
functions that never run concurrently. A file-scope blob is opaque to both, so every byte it
touches must be pinned by hand and is burned for the whole program. That pinned-scratch pattern
is exactly what `epic-math` suffers from under XC8 today, and it is a limitation to remove, not
to reproduce. `.asm` files as compile-unit inputs share the blob's opacity but cost almost
nothing, since epic-cc already owns `crates/asm`; they are the escape hatch, not the main road.

#### Rung 2: intrinsics

`__epic_di()` / `__epic_ei()` / `__epic_nop()` / `__epic_sleep()` and friends. Each is trivial
next to rung 3's cross-crate plumbing, and unlike an assembly string epic-cc *understands* them,
so it can keep optimizing around them instead of assuming the worst. Most real embedded inline
assembly is one of these, so this rung removes most of the demand for the two above it.

#### Rung 3: opaque statement-level assembly

```c
asm("bcf INTCON, 7");
counter = counter + 1;
asm("bsf INTCON, 7");
```

Ordering against volatile accesses is preserved by clang, as probed. Cost is a new `Inst::Asm`
through `ir` and `irparse`, verbatim emission in both backends, barrier rules in `banking` and
`peephole`, and symbol-to-address substitution in the string. This is the first rung that costs
work across crates.

#### Rung 4: memory operands

```c
asm("movf  %1, w\n"
    "addwf %0, f"
    : "+m"(t) : "m"(y));
```

The only rung that can name a local. epic-cc substitutes each operand's allocated address, which
it already knows from the `{func}::{name}` address map. Strictly more capable than XC8, where a
local cannot be referenced at all. **Restriction:** operands must resolve to a direct local or
global; a GEP-derived pointer such as `p->b` panics.

#### Two constraints found by probing, which bound every rung

**PIC register clobbers are not expressible.** `asm("nop" ::: "W")` is a hard clang error,
`unknown register name 'W' in asm`, because clang validates clobber names against msp430. Only
`"memory"` and `"cc"` pass. Therefore **epic-cc must always assume an assembly block clobbers
`W` and `STATUS` and leaves the bank unknown.** This is permanent, not a v1 simplification. If
it ever becomes a measured bottleneck, the only route is out of band, for example a
`; epic:clobber=w,status` comment inside the assembly string that epic-cc parses and strips.

**clang accepts operand constraints that are meaningless on PIC.** `asm("movwf %0" : "+r"(x))`
compiles cleanly and yields `"=r,0"(i8 1)`, a value operand. PIC's one register is not
allocatable, so epic-cc must reject register constraints with a precise error rather than emit
nonsense. This is the panics-are-the-error-surface rule applied to a case where the front end
will not help.

**The allocator is still the hard part, not the syntax.** A block touching `W`, `STATUS`, `FSR`
or a scratch byte interacts with overlay allocation and with `BANKSEL` insertion, and the
skip-sensitive `BANKSEL` hazard recorded in `AGENTS.md` is exactly the failure mode. `banking`
never inserts inside a block.

**This is separate from whether `epic-math` uses any of it.** epic-cc supports assembly because
a standalone compiler must. Whether the HAL's math routines go through it under epic-cc is
HAL-3's call, and the benchmark argues they should not (see §5).

### D-4: Fuses are sparse overrides over safe defaults, using datasheet names

**Decision.**

```c
#include <epic-cc.h>

EPIC_CONFIG("osc=hspll, plldiv=5, cpudiv=osc1_pll2, wdt=off, lvp=off");

void main(void) { ... }
```

Everything unstated takes a documented per-device default profile (watchdog off, low-voltage
programming off, brownout on, no code protection, debug off). Names are the data sheet's own.
epic-cc **prints the resolved config words** so defaults never mean unexamined silicon state.
epic-cc also **predefines `EPIC_FOSC_HZ`** from the resolved fuses.

**Rationale.** A PIC18F4550 needs roughly 28 `#pragma config` lines before it boots, and a user
cares about perhaps four of them. XC8 makes you write all 28 because it refuses to guess. Safe
defaults, not a cleverer syntax, are what removes the verbosity: the syntax above is barely
different from XC8's, and the line count is a quarter of it.

`EPIC_FOSC_HZ` fixes a real bug class. XC8 requires `FOSC` in a pragma and `_XTAL_FREQ` in a
separate `#define`, with nothing checking that they agree; desync is a classic source of silent
timing bugs. epic-cc knows the fuses, so there is one source of truth.

**Config-bit tables come from the data sheets, transcribed by hand** (DS39632 §25.1 for the
PIC18F4550, DS39582 for the PIC16F877A), and live as `device` crate data under ADR-004. Not
from XC8's headers, which the project rules place off limits, and not from `gputils`' `.inc`
files: invoking `gpasm` as a process is within the GPL boundary, transcribing its tables into
our source is a different act. Two devices makes this bounded.

**Rejected, goal-directed clock configuration** (state the crystal and the frequency you want,
epic-cc solves `PLLDIV`/`CPUDIV`/`USBDIV`/`FOSC`). A real ergonomic win, since the 4550 clock
tree is genuinely nasty, but it makes epic-cc own a per-device clock model and a solver.
Deferred, not closed: it would resolve to the same named settings, so it can be layered on as
sugar over the same table later.

**Rejected, fuses in the build configuration only** (CLI flags or `platformio.ini`). Better for
the ecosystem story, worse for a single file you want to hand to someone. Can be added as an
override channel later without changing the primary surface.

### D-5: epic-cc becomes epic-hal's default path; XC8 stays supported

**Decision.** epic-cc is the documented default: `install.sh` requires no Microchip downloads,
the scaffolded project builds with epic-cc, and epic-cc's own simulator (`crates/sim`) is the
CI gate for that path. XC8 remains a fully supported alternate environment and remains the
differential oracle.

**Rationale.** This is the largest user-facing win in the whole effort, and it is not a
compiler feature. epic-hal's getting-started today is gated on XC8 (licence-gated, and whose
free tier is the crippled optimizer that `epic-math` exists to beat) and on hand-unzipping a
device pack into `/opt/microchip/xc8/v4.00/pic/packs`. epic-cc needs neither. The same
constraint is why epic-hal's `target` CI job pulls a **private** GHCR image: the EULA forbids
redistribution. An epic-cc path has no such restriction, so that job can be public.

Keeping XC8 supported keeps the differential oracle that validates the HAL, and does not strand
existing users.

### D-6: PlatformIO gets its own repository

**Decision.** A new `platform-pic8` repository, plus two packages cut from the existing repos'
releases: `toolchain-epiccc` from epic-cc, `framework-epichal` from epic-hal.

**Rationale.** PlatformIO's registry model is one repository per platform (`platform.json`,
`builder/main.py`, `boards/*.json`), referencing toolchain and framework packages by URL.
Folding it into either existing repo welds that repo's release tags to PlatformIO package
versions, which goes wrong the first time a board JSON needs a fix that no compiler change
justifies. `[VERIFY]` the current PlatformIO platform and package manifest schema before
building against this shape.

### D-7: Translation units merge through `llvm-link`, out of process

**Decision.** The driver runs clang once per `.c` file, merges the resulting `.ll` files with
`llvm-link -S`, and hands the single merged `.ll` to `irparse` unchanged. `wholeprog` stays a
validator and never becomes a linker.

**Rationale.** Merging translation units is not concatenation. Four things have to happen, and
`llvm-link` does all four correctly. Probed on 2026-08-20 with two units carrying same-named
statics and a cross-unit global:

```
a.ll:  @shared  = dso_local global i8 0            <- definition
       @scratch = internal global i8 0
       define internal fastcc i8 @helper()
       declare i8 @from_b(i8)                      <- unresolved

b.ll:  @shared  = external dso_local global i8     <- declaration
       @scratch = internal global i8 0             <- collides
       define internal fastcc i8 @helper(i8)       <- collides
       define i8 @from_b(i8)

merged.ll:
       @shared    = dso_local global i8 0          <- one definition kept
       @scratch   = internal global i8 0
       @scratch.4 = internal global i8 0           <- renamed
       define internal fastcc i8 @helper()
       define internal fastcc i8 @helper.3(i8)     <- renamed
       define i8 @from_b(i8)
                                                   <- the declare is gone
```

None of it is expressible today. `irparse` strips `internal` as a noise attribute
(`crates/irparse/src/lib.rs:39`), so linkage is discarded before anything could use it; `declare`
lines are ignored entirely; and `@shared = external dso_local global i8` parses as a *definition*,
because the matcher looks for the substring `global ` (`:947`), so one extern referenced from
three units becomes three conflicting definitions of one byte. `ir::Module` has nowhere to record
any of it.

**The posture is identical to clang's.** Out of process, text in, text out, no libLLVM linked, so
[ADR-001](03-decisions.md) holds unchanged. The merged `.ll` is still a diffable text artifact, so
the stage boundary the pipeline rests on is preserved and in fact gains a bisect point.
`llvm-link` is already in the dev image at 6 MB against clang's 218 MB, so the bundle cost is
noise.

**The decisive argument is not cost.** Getting one-definition selection or internal-linkage
renaming subtly wrong produces a miscompile, which is the single failure class this project's
architecture exists to prevent.

**Rejected, implement the merge in `wholeprog`.** Requires teaching `irparse` to preserve linkage,
adding declaration state to `ir::Func` and `ir::Global`, and writing renaming and one-definition
resolution by hand. Full ownership of subtle semantics that ship in the box, bought with the
miscompile risk above.

**Rejected, textual concatenation of the `.ll` files with our own mangling.** Cheap, and wrong for
any program with two same-named statics.

#### Consequences

**`llvm-link` runs even for one input, and that is safe.** Probed against
`crates/driver/tests/fixtures/add.c`: the only differences are the module-ID comment,
`source_filename`, and metadata ordering. `irparse` already skips `;` and `!` lines
(`crates/irparse/src/lib.rs:933`) and already ignores `source_filename`. The committed golden HEX
fixtures are unaffected. Running it unconditionally keeps one code path instead of two.

**`wholeprog` gains real validation, with no IR format change.** `llvm-link` does not error on a
`declare` it could not satisfy, it just leaves it, which downstream becomes a `CALL` to a label the
assembler never heard of. `wholeprog` collects called-but-not-defined names and fails with the
list. Every call target is already in the IR, so this needs no new field and no change to the
`serialize`/`parse` round trip. It also checks there is exactly one `main`.

**Symbols are sanitized once, as a text transform on the merged `.ll`, before `irparse::parse_ll`
ever runs.** `llvm-link` emits `@helper.3`. Our assembler does not care, since labels are plain
string keys (`crates/asm/src/lib.rs:43`), but the `gpasm` byte-for-byte oracle has identifier
rules and that oracle is load-bearing. Rewriting `.` to `_` immediately after the merge is what
keeps `alloc`'s address map and `isel`'s labels consistent, since both key off `{func}::{name}`.
A sanitized name colliding with a real user symbol panics with both names rather than silently
overwriting.

**Implemented as `irparse::sanitize_symbols`, a text transform, not a walk over the parsed
`ir::Module` in `wholeprog`.** A name reaches the IR through seven different fields
(`Func.name`, `Global.name`, `Call.func`, `Val::Global`, `GepBase::Global`, `Load.ptr`,
`Store.ptr`) across `Inst`'s twenty variants; an IR-level walk in `wholeprog` would have to stay
exhaustive across all of them, silently, forever. One text pass over `@`-prefixed identifiers in
the merged `.ll` covers every one of those sites in a single pass and cannot miss a future `Inst`
variant. See [ADR-011](adr/ADR-011-multi-tu-front-end.md) for the full decision and the
`llvm-link` probe evidence this rests on.

**The CLI becomes conventional and the positional output goes away.**

```
epic-cc [options] <input.c>...
  -o <file>            output HEX (default: a.hex)
  -I <dir>             include path, repeatable, forwarded to clang
  -D <name[=value]>    define, repeatable, forwarded to clang
  --device <name>      p16f877a | p18f4550, required
  --emit <stage>       ll | ir | asm | hex (default hex)
  --save-temps <dir>   write every stage artifact
  -v                   echo the clang and llvm-link commands
```

`--emit` makes the pipeline's text boundaries a user-facing feature rather than a test-only one.
`--device` is required, matching `avr-gcc -mmcu`, and is what retires the hard-coded device at
`crates/driver/src/main.rs:21`. Dropping the positional output form touches roughly 25 call sites
across `crates/driver/tests/`, the fuzz harness's `driver_binary()`, the Makefile's `compile`
target and the README example: mechanical, and the bulk of CC-1's diff.

### D-8: interrupt-vector marking needs no epic-cc spelling; drop it from CC-3

**Decision.** epic-cc does not define an `EPIC_ISR` macro. Interrupt handlers are marked with
`__attribute__((interrupt(N)))`, msp430's own native syntax (the argument `N` is syntactic
noise clang requires but nothing downstream reads), which clang already lowers to the
`msp430_intrcc` calling convention. `irparse` already turns that into `Func.isr`
(`crates/irparse/src/lib.rs:1070`), and both PIC14 (shipped) and PIC18-in-compatibility-mode
(`feat/pic18-p5-interrupts`, in flight as of 2026-08-21) use it identically. There is nothing
for CC-3 to build here.

**Rationale.** D-2, written before this was checked, assumed interrupt-vector marking needed
the same section-attribute treatment as placement and config words. It does not: clang's
msp430 target already provides a portable attribute for exactly this purpose, and the
compiler already consumes it with zero epic-cc-specific code on either core.

**PIC18 high/low priority stays unbuilt, on purpose.** `docs/29-pic18-port-design.md`'s P5 row
promised two vectors with priority; the PR actually landing it is titled "single-vector
compatibility mode" deliberately deferring priority. Since the backend does not yet support two
distinct vector destinations, `EPIC_ISR_HIGH`/`EPIC_ISR_LOW` (D-2's original example) would
either have no consumer or would silently promise a capability that is not there, which is
exactly the kind of surface this project's panics-over-silent-miscompile rule exists to
prevent. When PIC18 priority interrupts land, if the plain `interrupt(N)` attribute cannot
express which vector a handler belongs to, that decision belongs to that work, not to CC-3.

**Rejected, build `EPIC_ISR_HIGH`/`EPIC_ISR_LOW` now, matching D-2's original example.** Ships
a macro with no consumer, or an API surface that lies about hardware support until a later PR
happens to catch up. Deferred, not closed.

### D-9: config words are per-device field tables, verified against `gpasm`, HEX emission gains multi-region support

**Decision.** Both device config regions are modeled as data, following ADR-004's
device-as-data convention:

```rust
pub struct FuseValue { pub name: &'static str, pub bits: u8 }
pub struct FuseField {
    pub name: &'static str,            // "osc", "wdt", "lvp": datasheet names, per D-4
    pub byte_offset: u16,               // offset within the config region
    pub mask: u8,
    pub shift: u8,
    pub values: &'static [FuseValue],
    pub default: &'static str,
    pub locked: Option<&'static str>,   // Some(only-legal-value) if epic-cc cannot honor an override
}
pub struct ConfigRegion {
    pub base_byte_addr: u32,            // 0x400E (PIC16F877A), 0x300000 (PIC18F4550) [VERIFY]
    pub reserved_ones: &'static [u8],   // unimplemented bits that must read 1, per datasheet
    pub fields: &'static [FuseField],
}
```

`EPIC_CONFIG("...")`'s string is comma-separated `key=value`, matched case-insensitively
against the device's field table. An unrecognized field or value panics naming the offending
token and the valid options. A field not mentioned takes its `default`. The resolved bytes are
folded with `reserved_ones` OR'd in. epic-cc prints the resolved config bytes and the named
setting behind each one unconditionally on success (D-4's promise; not gated behind `-v`).

**`locked` is a correctness rule, not an ergonomics nicety.** `isel-pic18` only ever emits
classic-mode PIC18 encoding. `XINST` (PIC18's extended-instruction-set config bit) must be
modeled, since v1's fuse coverage is the full bit set per device, but `locked = Some("off")`:
an override attempting `xinst=on` panics with the field and value, because the alternative is
silently shipping code whose addressing-mode semantics do not match what the silicon is
configured to execute, a miscompile, not a build error. `[VERIFY]` whether any other PIC18 or
PIC16F877A config bit shares this hazard before assuming `XINST` is the only one.

**Full-bit-set transcription is verified against `gpasm`, not trusted by hand alone.** Both
config regions (roughly one word for the PIC16F877A, roughly thirteen bytes for the
PIC18F4550, DS39632 §25.1) are transcribed from the datasheets by hand, real transcription-risk
work. `gpasm` is already this project's byte-for-byte oracle elsewhere; the same pattern
applies: assemble an equivalent `CONFIG`/`__CONFIG` pragma through `gpasm` and diff the
resulting bytes against epic-cc's, per fuse combination exercised by the test matrix, rather
than trusting hand transcription alone. Stays inside the GPL boundary: invoking `gpasm` as a
process in tests, never linking it.

**HEX emission gains a multi-region entry point; the single-region path is untouched.**
`to_hex` (`crates/asm/src/lib.rs:561`) already writes byte addresses as `word_index * 2` inside
one `0x04` extended-linear-address record fixed at `upper=0`. The PIC16F877A's config word sits
at word address `0x2007`, byte address `0x400E` `[VERIFY]`, still under `0x10000`, so it needs
no new address window, only widening the `words.len() <= device.flash_words` assert
(`:417`), which today conflates "program flash size" with "total addressable word space";
those are different concepts once a config word lives past the program's own ceiling. The
PIC18F4550's config bytes at byte address `0x300000+` `[VERIFY]` are outside any 16-bit window
and need a second `0x04` record with `upper=0x0030`. Rather than special-case PIC18, a new
`to_hex_regions(&[(base_byte_addr, &[u16])]) -> String` accepts a list of chunks and emits a new
`0x04` record only when a chunk's upper 16 bits differ from the previous one; PIC14 becomes the
single-chunk case, byte-identical to today's `to_hex` output, and PIC18 becomes two chunks.
`to_hex` itself is not modified, so every existing PIC14 fixture's golden output is unaffected
by construction.

**Rejected, size the PIC18 `words` array out to the config region directly (~1.5M entries) and
let the existing `to_hex` walk it.** Mechanically simpler, but pads roughly 1.5 million mostly-
zero words into memory and into the walk just to reach one 13-byte region, and produces
enormous zero-filled HEX output the existing chunking loop was never designed to skip.

### D-10: `EPIC_FOSC_HZ` is a preprocessor macro, resolved by a driver-side pre-scan, not a two-pass compile

**Decision.** `EPIC_FOSC_HZ` is a `#define`, forwarded to every clang invocation via `-D`
(the mechanism CC-1 already built), not a compiler-synthesized global constant. Resolving its
value happens before any clang invocation:

1. If no `EPIC_CONFIG(...)` override is found, `EPIC_FOSC_HZ` resolves entirely from the
   `--device`'s default fuse profile, known before any input file is read. This is the common
   case and costs nothing extra.
2. If an override might be present, the driver runs a small, string-literal-aware text scanner
   (not clang, not a preprocess-only `-E` pass) over the raw `.c` inputs, looking for exactly
   one top-level `EPIC_CONFIG(` invocation followed by a quoted string. Zero or more than one
   match across the whole program panics.
3. The extracted string resolves through the same field table and `locked` rule D-9 defines for
   the real config-word emission, so there is one resolution path, not two.
4. `-D EPIC_FOSC_HZ=<value>` is added to every clang invocation before compilation proceeds; the
   clang-per-file loop's shape is otherwise unchanged from CC-1.

**Rationale.** The embedded-toolchain precedent for a clock-frequency constant splits in two,
and the split matters here. AVR-GCC's `F_CPU` and XC8's `_XTAL_FREQ` are user-supplied
preprocessor constants, used almost entirely so a delay macro (`_delay_loop_2(F_CPU/4000*ms)`)
can unroll into a cycle-counted loop at compile time; a linker symbol cannot fill that role,
because C requires a constant *expression* there, not a value resolved at link time. The same
constraint applies to any compile-time-sized baud-divisor table. STM32's CMSIS splits the same
way for a different reason: `HSE_VALUE` (the raw crystal input) is a `#define`; `SystemCoreClock`
(the PLL-derived result, computed once at runtime) is a global. epic-hal's plausible uses of
`EPIC_FOSC_HZ`, a delay primitive or a baud-rate divisor, are the `F_CPU`/`_XTAL_FREQ` shape, not
the `SystemCoreClock` shape.

**The pre-scan is cheap because `EPIC_CONFIG`'s argument is a string literal we define, not
arbitrary C.** Finding it needs no semantic analysis and no clang invocation: a small hand-
written scanner that skips comments and string/char literals correctly (a naive `grep` would
misfire on a fuse string that happens to contain `/*`-looking text) is sufficient. This is not
the two-pass full clang compile the option looked like before checking: only the override case
touches the scanner at all, and even then it never invokes clang.

**Scope restriction for v1, stated plainly.** Exactly one unconditional, top-level
`EPIC_CONFIG(...)` invocation across the whole program is supported; one wrapped in the user's
own `#ifdef` is not scanned for and produces the "zero matches, using defaults" path silently
rather than an error, which is a real sharp edge worth flagging rather than discovering later.
`[VERIFY]` whether this silent-default behavior should instead be a diagnostic (e.g. "found
`EPIC_CONFIG` textually but could not confirm it is unconditional") once the scanner exists to
test it against.

**Rejected, compiler-synthesized global constant.** Simple to build (inject a new `Global` into
the merged IR after resolution, no driver-loop changes at all) and functionally insufficient:
not usable in `#if`, and not usable to size a compile-time array, which is the load-bearing use
case in the ecosystems this convention is borrowed from.

**Rejected, two-pass full clang compile** (preprocess or compile every unit once to discover
the override, then again with `-D` added). Would have worked, but is strictly more expensive
than the scanner for no additional correctness, since `EPIC_CONFIG`'s argument never needs
semantic analysis to extract.



---

## 3. Sub-projects

Fourteen, across three repositories. Each earns its own design and plan.

### epic-cc

| ID | Sub-project | Contents |
|---|---|---|
| **CC-1** | **Multi-TU front end** | Per D-7: conventional CLI, clang per file, `llvm-link` merge, `wholeprog` becomes a validator (unresolved externals, one `main`) and the symbol sanitizer. Replaces `crates/wholeprog/src/lib.rs:5`. |
| **CC-2** | **Freestanding libc subset** | Headers and implementations for `stdint.h` (94 uses in epic-hal), `string.h` (43), `stdbool.h` (31), `stddef.h` (17). `string.h` needs real code, not just declarations. |
| **CC-3** | **Silicon-real codegen** | Per D-8/D-9/D-10: config-word field tables (full bit set, both devices), `EPIC_AT`, multi-region HEX emission, `EPIC_FOSC_HZ` via a driver-side pre-scan, the resolved-config report, and the `epic-cc.h` header itself. No `EPIC_ISR`, superseded by D-8. |
| **CC-4** | **Inline assembly** | The D-3 ladder: naked functions and `.asm` inputs, then intrinsics, then opaque statement-level assembly, then memory operands if warranted. Conservative clobbering throughout, since clang cannot express PIC register clobbers. |
| **CC-5** | **PIC18 P4-P8** | `const` via `TBLRD`, two-vector interrupts, 32-bit `long` and hardware `MUL`, soft-float, differential fuzzing. Per `docs/29-pic18-port-design.md` §4. |
| **CC-6** | **Toolchain distribution** | Release bundles for Linux, macOS and Windows with clang 20.1.8 inside; today Linux only (`docs/30-distribution-design.md`). Plus size and map reporting, which PlatformIO expects. |

### epic-hal

| ID | Sub-project | Contents |
|---|---|---|
| **HAL-1** | **epic-cc build environment** | A fourth `include/` and `src/` variant beside `target/`, `host/` and `mdb/`. Add `EPIC_ISR_HIGH`/`EPIC_ISR_LOW` and `EPIC_CONFIG` macros, map all four onto epic-cc spellings. |
| **HAL-2** | **Build-system backend** | `epic_build.py` and `epic-hal.mk` learn epic-cc: no XC8, no device pack, no MPLAB X project. The python-outside-the-container workaround becomes unnecessary on this path, since epic-cc has no manifest resolution problem. |
| **HAL-3** | **Module conformance** | Every `epic-*` module and both HALs building and passing. Contains the two real decisions: `epic-math`'s assembly (see §5) and `epic-serial`'s `printf` retarget. Largest HAL item. |
| **HAL-4** | **Verification** | epic-cc's `crates/sim` replaces the `mdb` / MPLAB SIM gate on the epic-cc path. This is what makes a public CI job possible. |
| **HAL-5** | **Distribution flip** | `install.sh` and the bundles default to epic-cc: zero Microchip downloads, a scaffolded Makefile, no `.X` project. |

### platform-pic8 (new)

| ID | Sub-project | Contents |
|---|---|---|
| **PIO-1** | **Platform core** | `platform.json`, `builder/main.py` (SCons), board definitions for the supported parts. |
| **PIO-2** | **Package plumbing** | `toolchain-epiccc` cut from CC-6's release assets; `framework-epichal` cut from epic-hal releases. |
| **PIO-3** | **Registry and examples** | Publication, worked examples, documentation. |

---

## 4. Dependencies and sequencing

```
  CC-1 multi-TU  ─┬─────────────> HAL-1 ─> HAL-2 ─> HAL-3 ─> HAL-4 ─> HAL-5
  CC-2 libc      ─┤                                  ^
  CC-3 silicon   ─┤                                  │
  CC-4 asm       ─┘                                  │
                                                     │
  CC-5 PIC18 P4-P8 ──────> (PIC18 family parity) ────┘

  CC-6 distribution ──────> PIO-2 ──> PIO-1 ──> PIO-3
  HAL-5 ──────────────────> PIO-2
```

`CC-1` blocks every HAL sub-project. `CC-2` and `CC-3` block `HAL-3`. `CC-4` blocks `HAL-3`
only if `epic-math` keeps its assembly under epic-cc. `CC-5` blocks PIC18 family parity but
nothing on the PIC14 path.

### Chosen order: PIC14 first as the pathfinder

`CC-1` .. `CC-4`, then `HAL-1` .. `HAL-4` against `pic16f87xa-hal`, then `CC-5`, then PIC18
integration along the proven path, then `HAL-5` and the PlatformIO repo.

**Why.** epic-cc's PIC14 backend is already feature-complete: interrupts, `long`, soft-float,
`const` tables, structs and multi-page all have e2e fixtures with committed HEX in
`crates/driver/tests/fixtures/`. So through the entire integration phase, the backend is never
the suspect, and every failure is an integration failure. That is the same bisect discipline
the pipeline's diffable text boundaries exist to serve. The genuinely unknown work is
`CC-1` .. `CC-4`, not `CC-5`, and this order surfaces the unknowns first.

**Amendment.** Once `CC-1` and `CC-3` land, run a thin PIC18 smoke slice (a blink plus
`epic-tick` on the 4550) so the PIC18 path cannot drift silently while PIC14 integration
proceeds.

**Rejected, PIC18 first.** Matches current momentum and targets the roomier part, but puts a
new backend and a new integration layer under test simultaneously, so a miscompile is ambiguous
between them.

**Rejected, parallel tracks.** `CC-1` .. `CC-4` are target-independent and could run in a
worktree alongside `CC-5`. Fastest by wall-clock, but two moving fronts against a project built
on bisectability.

---

## 5. Open questions, to settle inside the sub-projects that own them

**`epic-math` under epic-cc, a measurement not a port (HAL-3).** The cycle benchmark in
epic-hal's `docs/experiments/math-cycle-benchmark/README.md` shows the hand assembly losing to
optimized native code on every operation on the 4550, and on mul and div on the 877A at `-O3`.
That assembly exists to beat XC8's licence-gated optimizer, and epic-cc has no licence gate.
`epic-math/src/host/` already implements the same four modules in C. So `HAL-3` should open by
building that C path under epic-cc and measuring, not by porting 400 assembly statements.
**Before believing this**, check whether anything depends on the *fixed-cycle* property the
assembly advertises, as opposed to only its speed: a C path is not cycle-deterministic.

**`epic-serial`'s `printf` retarget (HAL-3).** Depends on how much of `stdio` `CC-2` provides.
Variadic functions on a machine with no stack are their own design problem. Likely resolved by
a non-variadic formatting API on the epic-cc path rather than by implementing `printf`.

**RAM headroom on the 877A `[VERIFY]`.** epic-hal's modules fit under XC8 today. epic-cc's
overlay allocator is a different allocator, and 368 bytes leaves little room to be wrong. This
is a real risk of the chosen sequencing and is best discovered early, which is also an argument
for that sequencing.

**Section-attribute passthrough: confirmed 2026-08-20.** D-2 and D-4 rest on clang forwarding
`__attribute__((section("...")))` verbatim into the `.ll`. Probed against the pinned clang 20.1.8
with the driver's exact flags: a global carrying `EPIC_AT` emits
`@port = dso_local global i8 0, section ".epicat.0x0F80"`, a function carrying `EPIC_ISR` emits
`define ... @isr_hi() ... section ".epicisr.high"`, and a `used` dummy carrying the fuse string
emits `section ".epiccfg.osc=hspll, wdt=off, lvp=off"`. Globals, functions and arbitrary string
payloads all survive.

**PlatformIO manifest schema `[VERIFY]`.** The `platform.json` / package shape in D-6 is
working knowledge. Confirm against current PlatformIO documentation before `PIO-1`.

**Host simulation's future.** epic-hal's fast inner loop is a host build under gcc. epic-cc's
`crates/sim` runs actual target code, which is a stronger gate but a slower loop. `HAL-4`
decides whether the host build remains the inner loop or is replaced.
