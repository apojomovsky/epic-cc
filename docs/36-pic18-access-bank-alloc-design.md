# 36: PIC18 float-routine access-bank frame allocation

> **Approval status:** proposal, pending review (ADR-015's *"Revisit if"
> clause, docs/adr/ADR-015-pic18-softfloat.md). Ticket: PIC18 parity
> sub-epic #267, the `%f` printf gap. Supersedes neither ADR-015 nor
> docs/29; this is the allocator-side half ADR-015 deferred.

**Problem:** on PIC18 a program that calls a float runtime routine
(`__add_f32`, `__mul_f32`, `__cmp_f32`, ...) only compiles when the routine's
frame happens to land inside the access-bank GPR region (`0x10-0x5F` on
p18f4550). `float.c` passes because its call chain is shallow; the standard
library's `printf` `%f` path (`printf -> vprintf -> out_float -> out_dec`
then the float routines) pushes `__cmp_f32`'s frame to `0xCC-0xD4`, past the
`0x5F` ceiling, and `isel-pic18` aborts:

```
float routine __cmp_f32 frame exceeds the access-bank GPR region (0x0CC > 0x05F)
```

The same fixture compiles cleanly on p16f877a (no access bank; routines land
in bank 0 where the recipes' `assert_bank0` holds). The root cause is that
**`crates/alloc` is access-bank-unaware**: it lays out every frame by
call-graph chain depth with only the "whole frame in one GPR bank"
(`routine_base`, issue #6) rounding. PIC18's `ram_banks` is one giant bank
`0x0010-0x07FF`, so that rounding never pins a float routine into `0x10-0x5F`:
whether a float routine's frame fits the access bank is left to chance.

## 1. Why the access-bank constraint exists (the load-bearing fact)

ADR-015 ports PIC14's float recipes to PIC18 by substituting every file
operand with the `a=0` access-bank form (no `MOVLB`). The recipes are
skip-sensitive: a `MOVLB` inserted by a later banking pass between `BTFSC`
and its skip target, or inside a same-skip carry idiom (`INCFSZ f,W` →
`ADDWF g,F`), would change the skip targets. So every float routine slot
(`a`/`b`/`val` + `__scr` + retval) must sit at `<= access_bank_hi` and be
reached with `a=0` unconditionally. `emit_routine` asserts this at
emission (isel-pic18 `select_with_locs` reads `device.access_bank`).

This is not an arbitrary choice: it is the direct PIC18 analogue of PIC14's
`assert_bank0` for the same recipes. The constraint is real and will not
go away; the allocator must honor it, not the backend give up on it.

## 2. Measured frame budgets

Access-bank GPR region on p18f4550: `device.access_bank = (0x0000, 0x005F)`,
`device.fixed_retval = (0x0000, 0x000F)`, `gpr_start() = 0x0010`. The
float-routine window is therefore `0x10-0x5F`, **80 bytes**.

Float runtime routine frame footprints (param slots + `__scr`, from
`crates/legalize/src/lib.rs` `routine_func`):

| routine | `a`/`b`/`val` | `__scr` | total |
|---|---|---|---|
| `__add_f32` `__sub_f32` `__mul_f32` | 4+4 | 14 | 22 |
| `__div_f32` | 4+4 | 12 | 20 |
| `__cmp_f32` | 4+4 | 6 | 14 |
| `__uitofp_f32` `__sitofp_f32` | 4 | 8 | 12 |
| `__fptoui_f32` `__fptosi_f32` | 4 | 8 | 12 |

Float routines never call each other in the IR (each legalized float op
lower is a single call to one routine, which does its arithmetic in its own
frame). They are siblings, never co-live, and the allocator already packs
sibling frames contiguously (`routine_base`, "sibling routines pack
contiguously"). The **maximum single-live demand is 22 bytes** - the whole
set of used float routines fits in 80 bytes with the overlay free to pack
them one after another exactly as it does today. The window is not tight on
demand; it is only unreachable because nothing reserves it.

## 3. Decision

Teach `crates/alloc` the PIC18 access-bank GPR window as a **dedicated
placement region for float runtime routines**. When the module uses any
float routine and the device declares `access_bank` (PIC18), and only then:

1. **Reserve `[gpr_start..=access_bank_hi]` (`0x10-0x5F`, 80 bytes on
   p18f4550) for float runtime routine frames**, which the overlay packs
   contiguously there (sibling float routines never co-live and never call
   each other, so one shared span holds all of them).
2. **Globals and all ordinary function frames start at
   `access_bank_hi + 1`** (`0x60`) instead of `gpr_start()`. This is what
   makes the window reliably available, and it is required by the overlay
   model: globals are live for the whole program and a caller's frame is
   live during the float call, so neither may share bytes with a float
   routine frame. Moving them out is unambiguous and always fits.
3. **Each float routine's frame is placed via `float_routine_base`**, a new
   allocator step mirroring `routine_base` (single-window rounding) but
   bound to the access-bank high limit: it packs the routine's `a`/`b`/
   `val` + `__scr` slots into the window and asserts the end
   `<= access_bank_hi`. The retval already lives in `fixed_retval`
   (`0x00-0x0F`), inside the window, untouched.
4. Keep `round_if_routine`'s single-GPR-bank behavior for non-float
   (integer) routines unchanged: those use `operand()`'s `MOVLB` and may
   live anywhere in banked RAM.

### Why reserve the whole window and move globals, not pack around them

The alternative (place globals in the window's low part and pack float
routines after them) is unsound when a global is large: the `printf_va`
fixture's `g_buf[80]` spans `0x10-0x60`, consuming the entire window, so
no float routine could fit even though the program does float work. Only a
global-free reservation makes the window's 22-byte peak demand (section 2)
guaranteed. This is not a tightness problem with the window; it is an
allocation-order problem, and reserving it wholesale is the only robust
answer.

### Cost and unchanged-addresses invariant

The allocator's module doc promises "every program that already succeeds
keeps unchanged addresses." This change preserves that invariant by
construction for every program that does NOT use a float routine on PIC18:
the reservation and the `access_bank_hi + 1` base only activate when a
float routine is present. Programs that do use a float routine on PIC18
today only compile if their frames happen to sit in the window; they are
rare (float.c, the printf fixtures), and giving them the reservation moves
their globals above `0x60` - a deliberate, correct change, not a regression.
p18f4550 has 2048 bytes of GPR; the 80-byte window is 4%.

## 4. Alternatives considered

**A. Relax the access-bank rule (allow `MOVLB` outside skip windows).**
ADR-015's own revisit clause names this. But it means rewriting every float
recipe to use `operand()` (banked) for the file operands that are not
inside a skip window and `a=0` only for the skip-sensitive ones - a
substantial, error-prone re-derivation of the machine-verified recipes, for
a saving (80 bytes on PIC18) far in excess of the complexity. It also
un-homogenizes the recipes across PIC14/PIC18 (PIC14 still needs bank 0),
fragmenting the "machine-verified on PIC14, ported line-for-line" story that
ADR-015 preserves. Rejected; the window is not tight, so the complexity
buys nothing.

**B. Pack float routines into free access-bank holes between existing
globals/frames.** Maximizes the window's utility but couples float-routine
placement to the ordinary layout's final addresses, forcing an extra
placement pass and risking non-minimal layouts. Violates the allocator's
preserve-existing-addresses invariant when a float routine is added to a
program whose frames currently use the window. Rejected for the same
simplicity reason as reserving wholesale (section 3).

**C. Raise the ceiling by splitting the frame so only some slots sit in
the access bank.** The recipes' skip-sensitive operations use many slots
throughout their bodies; splitting them in half still needs every file
operand that any skip touches to be `a=0`, which is effectively all of them.
Same failure mode as A. Rejected.

## 5. Scope and verification

Files touched (implementation PR, after this design is approved and
reviewed): `crates/alloc/src/lib.rs` (`allocate`, `bank0_start`, new
`float_routine_base`); `crates/alloc` unit tests; `crates/isel-pic18` and
its `--map` output remain unchanged (the addresses isel reads just come out
lower for float routines).

Acceptance:

- `printf` `%f` fixture and the existing `printf_va` fixture both compile
  and run on p18f4550 (regression: `printf_va_runs_on_p18` currently fails
  with the formatter linked; it must pass). New `printf_f_e2e` + fixture
  verifies `%f` output on both cores.
- `float.c` / `float_e2e` still passes on p18f4550 and p16f877a; the float
  routine addresses may move but the program's observable floats do not.
- Every existing p18 test keeps passing; the allocator's
  unchanged-addresses invariant holds for programs with no float routine
  (only the window reserve, which such programs do not use, is added).
- `cargo test` full suite green; takeoff ritual (`make pre-pr-check`).

## 6. Out of scope

- PIC14 (no access bank; already correct via `assert_bank0`).
- Rewriting the float recipes (rejected in section 4).
- The `%f` formatter itself (already written and verified on p16, branch
  `feat/267-printf-fmt`, commit `6f13906`); it becomes shippable to p18 once
  this allocation fix lands.
- ISR-context float routine copies: they use their own `_isr` slots and
  derive from the disjoint ISR region; the same access-window packing
  applies if an ISR lowers a float op (rare, but the same rule holds).
