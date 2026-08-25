# PIC14 signed min/max/abs intrinsics and s16 mul -- Design

**Status:** approved (2026-08-25) / implemented in PR #136
**Date:** 2026-08-25
**Parent:** `docs/03-decisions.md` ADR-002 (whole-program compilation), ADR-003 (static allocation)
**Ticket:** `epic-cc#133`
**Scope:** `epic-cc` compiler only; the HAL stub removal is tracked in `apojomovsky/epic-hal#98`.

---

## 1. Goal and non-goals

**Goal:** the C clamp idiom (`if (v < lo) v = lo; if (v > hi) v = hi;`) and a
16x16 -> 32 signed multiply feeding an i16 truncate compile and run on
`p16f877a` when clang folds them into `llvm.smax` / `llvm.smin` /
`llvm.abs` intrinsics and `mul nsw i32`. This is the compiler-side half that
unblocks removing the `#ifdef __EPIC_CC__` stub in `epic-pid/src/pid.c`
(`epic-hal#98`).

**Non-goals (v1):**

- `llvm.umax` / `llvm.umin` (unsigned counterparts): the HAL clamps are signed;
  the unsigned fold does not appear in the target sources. Lowered the same way
  if a future program emits it, but nothing in this ticket needs it.
- `llvm.abs` on `i32`: pid's abs is i16 (the operand of `epic_math_mul_s16`);
  the i16 emitter is the only one exercised. The lowering is width-generic in
  the IR, so i32 comes free, but no fixture drives it.
- The epic-hal stub removal itself: that is `epic-hal#98`, which this ticket
  unblocks. Its acceptance is re-recorded there, against the merged compiler.

---

## 2. Empirical ground truth (clang 20.1.8, `-target msp430 -O1`, as pinned)

Captured from the real un-stubbed `epic_pid_update` body (this session) and
from probe fixtures.

**The pid clamp body emits intrinsic calls, not icmp/select:**

```llvm
%35 = tail call i16 @llvm.smax.i16(i16 %34, i16 %22)
%36 = tail call i16 @llvm.smin.i16(i16 %35, i16 %26)
...
%42 = tail call i32 @llvm.smax.i32(i32 %40, i32 %24)
%43 = tail call i32 @llvm.smin.i32(i32 %42, i32 %28)
```

**The abs idiom (`(a < 0) ? 0-a : a`) folds to:**

```llvm
%16 = tail call i16 @llvm.abs.i16(i16 %13, i1 false)
```

**The s16 mul folds to `mul nsw i32`** (no intrinsic; verified already
lowered via the `__mul_u32` runtime routine):

```llvm
%5 = mul nsw i32 %4, %3
```

**Failure surface, stage by stage** (reproduced this session):

1. `irparse` panics at `parse_call_arg` (`call arg must carry a value`) on the
   `i1 false` immarg of `llvm.abs`; `true`/`false` are not call-arg tokens.
2. `wholeprog::check_calls_resolved` rejects `llvm.smax.i16`,
   `llvm.smin.i16`, `llvm.smax.i32`, `llvm.smin.i32` as undefined symbols
   (they are `declare`d, never defined).

**Verified already green** (probe fixtures compiled and produced hex this
session):

- The clamp pattern on volatile inputs (icmp `slt`/`sgt` + `select`), i16 and
  i32, both widths.
- `mul nsw i32` lowers to `CALL __mul_u32` (recipe exists).
- `lshr i32` const count, `trunc i32 to i16`, `sext i16 to i32`.
- The sret path (struct return fixtures) with no abort.

So the entire delta is the three intrinsics reaching `legalize` unlowered and
the parse/wholeprog gates in front of them.

---

## 3. Design

### 3.1 `irparse`: parse `i1 true` / `i1 false` call args

`parse_call_arg` gains `true`/`false` tokens, mapping to `Val::Const(1)` /
`Val::Const(0)` (the same mapping `parse_val` already uses). This is a pure
parse fix: `i1` constant immargs are part of the clang text surface and every
consumer (legalize lowering) reads them as constants.

### 3.2 `wholeprog`: intrinsic calls are not undefined symbols

`check_calls_resolved` skips call targets whose function name starts with
`llvm.`. They are `declare`d by the front end and lowered by `legalize`, not
user functions missing a definition. The existing `declare` is what
`llvm-link` leaves in place; `legalize` replaces the call before `isel` sees
it. Skipping them is safe because `legalize` (3.3) panics loudly on any
`llvm.*` call it does not know, so a new intrinsic surfaces as a clear error
rather than an assembler label hole.

### 3.3 `legalize`: lower the intrinsics to icmp/select trees

The lowering is a pure `Inst::Call -> Vec<Inst>` rewrite, the same shape as
`lower_fcmp`. Fresh SSA names come from the existing `FreshNames`.

| intrinsic | lowering |
|---|---|
| `llvm.smax.${w}(a, b)` | `%c = icmp sgt $w %a, %b; %r = select i1 %c, $w %a, $w %b` |
| `llvm.smin.${w}(a, b)` | `%c = icmp slt $w %a, %b; %r = select i1 %c, $w %a, $w %b` |
| `llvm.abs.${w}(a, flag)` | `%c = icmp slt $w %a, 0; %n = sub $w 0, %a; %r = select i1 %c, $w %n, $w %a` |

The `abs` second arg (`is_int_min_poison`, here `false`) is read from the
parsed constant and ignored: `INT_MIN` maps to `-INT_MIN = INT_MIN` under the
lowering, which is the conforming value when the flag is `false`, and poison
when `true` (any value is conforming). One lowering serves both.

Widths: the icmp/select/sub emitters in `isel` are byte-generic (i8/i16/i32
already tested), so the tree is width-parametric. The `mul` in the pid body is
already a `mul nsw i32` and needs no new lowering (`__mul_u32`), so there is
no mul work in this ticket beyond the fixture gate.

### 3.4 `isel` and the sret spike

No isel change is required: the icmp/select shapes lower today (verified).
The "sret null isel abort" recorded in epic's PR #96 was the pre-#73
indirect-call panic: the spike build that hit it used a dispatch through a
function pointer, which is exactly the gap #73 closed. The sret surface is
covered by committed fixtures (`struct_return` / float e2e) and compiles
clean today. This ticket adds no sret work; the issue's "close it either way"
answer is: closed by reference, no reproducer exists on current master.

---

## 4. Tests

- **legalize unit tests** (crates/legalize): each intrinsic lowers to the
  exact icmp/select tree (structural equality on the emitted `Vec<Inst>`),
  plus an unknown-`llvm.*` call panics loudly.
- **driver e2e fixture** (`fixtures/pid_clamp.c`): the acceptance program
  (clamps + 16x16 -> 32 mul + trunc), committed HEX + simulator run with a
  hand-computed expected value, following the `muldiv_e2e.rs` shape. The
  inputs are non-volatile so clang emits the intrinsics (the volatile form
  would test the icmp path, which already passes).
- **probe e2e**: the un-stubbed `epic_pid_update` body + C-path math
  (`epic_math_mul.c`/`epic_math_addsub.c`) compile through the driver and run
  in the simulator, which is the exact program `epic-hal#98` will build.

---

## 5. Acceptance (from #133)

- [x] A fixture with the pid clamp pattern (signed min/max on i16, then a
      16x16 -> 32 signed multiply feeding an i16 trunc) compiles and runs on
      the `p16f877a` simulator, generated HEX in the usual shape (driver
      fixtures generate HEX in-test; asm fixtures commit HEX).
- [ ] `epic-cc` builds the un-stubbed `epic_pid.c` + C-path math sources
      (no `__EPIC_CC__` body) into a runnable program.
- [ ] The sret/spike question answered: closed by reference (pre-#73
      indirect-call panic; sret e2e green today), no new reproducer.
- [ ] Full workspace test suite green.

## 6. Out of scope / follow-ups

- `epic-hal#98` removes the stub and re-records RAM/flash.
- `llvm.umax`/`llvm.umin`/`i32 abs` stay as easy follow-ups if a program needs
  them (same lowering, one-line dispatch).
