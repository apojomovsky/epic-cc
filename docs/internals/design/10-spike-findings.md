# 10 — Feasibility spike: FINDINGS

**Status: COMPLETE — all four questions answered. Success criterion met.**

The spike code lives in `spike/` and is **gitignored and throwaway**. It is not the
product, must not be built on, and can be deleted freely. Only this findings document is
committed.

On 2026-08-14 the spike was completed end-to-end: a throwaway Rust pipeline parses the
probe `.ll`, does storage allocation, phi elimination, and instruction selection, emits
`.asm`, assembles it with `gpasm`, and runs the resulting Intel HEX in a throwaway PIC14
simulator. **The probe compiles and runs correctly: `out == 48` for `in == 5`.**

---

## The probe program

`spike/probe.c` — contains a loop, an `if`, a function call, and 16-bit arithmetic:

```c
volatile unsigned char in;
volatile unsigned char out;

__attribute__((noinline)) static int add(int a, int b) { return a + b; }

void main(void) {
    int n = in;
    int t = 0;
    for (int i = 0; i < n; i++) {
        if (i & 1) t = add(t, i);
        else      t = add(t, 100);
    }
    out = (unsigned char)t;
}
```

`in` and `out` are `volatile` so the optimizer cannot fold the program away and so the
result is an observable side effect a simulator can assert on. Expected result for
`in = 5` is `out == 48` (t reaches 304; 304 mod 256 = 48). **Verified by execution** — the
simulator halted at `SLEEP` after 420 instructions with `out == 48`.

Known-good invocation (note: **unwrapped** clang, see [`09`](09-build-environment.md)):

```bash
"$PIC8_CLANG_UNWRAPPED" -target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc \
    -resource-dir "$PIC8_CLANG_RESOURCE_DIR" -o probe.ll probe.c
```

---

## ✅ Q1 — Is `.ll` text a good substrate w.r.t. ABI decisions? **ANSWERED: yes.**

The feared failure mode — "clang bakes in ABI decisions (`byval`, `sret`, varargs, alloca
patterns) that fight a machine with no stack" — **did not materialize.** Measured on
scalar code and, this time, on aggregates too:

- **Scalar code at `-O1`/`-O2`:** allocas vanish entirely; everything is SSA values and
  `phi` nodes. No `byval`/`sret` appears. Straight-line and loop code carry no stack
  assumptions at all.
- **Struct return (`sret`):** `struct pair make(...)` lowers to
  `make(ptr sret(%struct.pair) %0, …)` — a *hidden pointer to caller-allocated space*.
  This is exactly what a static-frame backend wants: the caller reserves the struct in its
  own frame and passes the address. Friendly, not hostile.
- **By-value struct parameter (`byval`):** `sum(struct pair p)` lowers to
  `sum(ptr byval(%struct.pair) %0)` — the caller copies the struct into its frame and
  passes a pointer. Same shape: a pointer into a frame.
- **Struct locals (`alloca`):** a static frame slot. Direct.
- **`llvm.lifetime.start/end`:** appear around `alloca`s at `-O1`. These are optimization
  hints only; the backend strips them as no-ops.

**Conclusion:** clang's aggregate ABI is *pointer-based*, and pointers into a statically
allocated frame are precisely the primitive a no-stack PIC14 backend needs. The real work
is not ABI-fighting — it is the **pointer layer itself** (`getelementptr`, `load`/`store`
through pointers → `FSR`/`INDF` codegen), which the spike did not exercise because the
probe has no pointers or arrays. That is the largest *unexercised* codegen risk, not the
ABI.

One caveat: **varargs were not tested.** They are the one place clang's ABI is genuinely
stack-shaped, and they deserve a dedicated probe before the design treats them as solved.

---

## ✅ Q2 — How much `.ll` surface must we parse? **ANSWERED: tractable.**

Measured, not estimated. A hand-written parser covering **12 distinct opcodes** handles the
entire probe program:

```
binop x3   br x1   br.cond x2   call x1   icmp x3   load x1
phi x3     ret x2  select x1    store x1  trunc x1  zext x1
```

The parser is ~330 lines of Rust including attribute stripping, written and working in one
pass. **This is not a tarpit.** The construct list grows incrementally — pointers, GEP,
arrays, aggregates, varargs, intrinsics — but the shape of the work is bounded.

**Recommendation: `.ll`-text-as-interface is confirmed viable.** ADR-001 stands.

One design note that fell out: the parser must aggressively **strip attributes it does not
model** (`noundef`, `nsw`, `nuw`, `nneg`, `tail`, `fastcc`, `range(i16 -32768, 255)`, …).
That is mechanical, but it must be deliberate rather than incidental, because a silently
mis-parsed attribute is a silent miscompile. The spike parser does this, and it is the one
place where the parser's "fail loudly on anything unsupported" rule must be preserved.

---

## ✅ Q3 — Does common RAM survive as an imaginary register file? **PARTIAL→ANSWERED: tight.**

Measured on the probe: **16 SSA values plus 2 params require 26 bytes** of storage under
naive one-slot-per-value allocation. Common RAM (0x70–0x7F) is **16 bytes**. It overflows
by 10 bytes — on a program of eleven lines.

**Conclusion: liveness-based storage reuse is mandatory, not an optimization.** Two
consequences worth carrying into the design:

1. Stage 6 (`alloc`) cannot be "assign, then optimize later." Interference-graph colouring
   has to be in the first working version.
2. Common RAM needs a **spill path into bank 0 GPR** from day one. Since bank 0 and common
   RAM are both reachable with `RP1:RP0 = 00`, the spike's codegen ran entirely in bank 0
   and therefore **did not exercise banking at all** — a green spike would not have
   validated BANKSEL.

### A subtlety the naive allocator exposed

The spike's first allocator keyed SSA values by their **bare LLVM name** (`%1`, `%3`, …).
That is wrong: SSA names are *function-local*. Two functions both have a `%1`, and the
collision silently merged their storage (`add`'s `%1`/`%3` overwrote or shared `main`'s
slots). It happened to be benign for this probe (the collided values were dead at the point
of overlap), but it is a latent miscompile in general.

The spike fixed this by keying slots on `(function, name)`, after which the measured
demand is the honest **26 bytes (16 common + 10 bank-0)**. The lesson for the design is
concrete: **allocation is function-scoped, and "assign every SSA value a slot" must mean
"every (function, value)", not "every bare name".** This does not invalidate the approach;
it sharpens the already-known conclusion that a correct allocator (colouring + function
scoping + spill) is first-version work, not a later optimization.

---

## ✅ Q4 — How bad is Harvard `const` data? **ANSWERED: real, and the IR gives no help.**

Tested with a `const` array probe:

```c
static const unsigned char table[4] = {10, 20, 30, 40};
out = table[in & 3];
```

clang emits (at `-O1`):

```llvm
@table = internal unnamed_addr constant [4 x i8] c"\0A\14\1E(", align 1
…
%4 = getelementptr inbounds nuw [4 x i8], ptr @table, i16 0, i16 %3
%5 = load i8, ptr %4, align 1
```

Two consequences:

1. **The `constant` marker is the *only* signal** distinguishing a flash-resident table
   from a RAM variable. There is no separate address space in the IR; the backend must
   recognise `constant` globals and place them in program memory itself.
2. **Reads from a `constant` global are just `load`s through a `getelementptr`.** The
   backend must lower those to a *program-memory* read. The PIC16 has no `TBLRD` (that is
   PIC18); the standard technique is a **`RETLW` lookup table** — a computed `GOTO` into a
   table of `RETLW k` entries — with `PCLATH` management for tables that cross page
   boundaries.

This is the least-derisked part of the design and the one place the 6502 offers **no prior
art** (the 6502 is von Neumann; `llvm-mos` never solves Harvard const). It needs its own
design spike before the pointer/data path is considered settled.

---

## Other findings worth keeping

### Optimization level changes the problem qualitatively

| Level | Character of the IR |
|---|---|
| `-O0` | allocas everywhere, load/store traffic, calls preserved, no phis, `sret`/`byval` for aggregates |
| `-O1` / `-O2` | no allocas (scalars), SSA + `phi`, `if` lowered to `select`, `static` functions fully inlined |
| `-Oz` | as above, plus loop rewriting into closed form, arbitrary-width integers (`i17`) and intrinsics (`@llvm.smax.i16`) |

The `-Oz` observation is documented with the actual IR in
[`09-build-environment.md`](09-build-environment.md).

### `static` functions are inlined away at -O1+

The original probe had `static int add(...)` and **the call disappeared entirely** at `-O1`.
`__attribute__((noinline))` was required to retain a call at all. Anyone testing calling
convention must account for this or they will be testing nothing.

### `if` becomes `select`

`if (i & 1) ... else ...` was lowered to `select i1 %12, i16 100, i16 %9` rather than
control flow. Our isel needed a `select` lowering (branch-based / `BTFSC`-based skip) from
the start; the spike lowered it to a branch, confirming it is not an exotic construct.

### The gpasm HEX cross-check passed

The simulator independently decodes gpasm's Intel HEX and agrees with its instruction
encodings. Format: **two little-endian bytes per 14-bit word, at byte address `word*2`**,
with an `04` (extended linear address = 0) record up front. This validates both our codegen
(`.asm` semantics) and our simulator (independent ISA semantics) against a third party.

### The naive phi elimination has a latent critical-edge bug (recorded, not solved)

The entry block `%0` has two successors, and block `%6` has two predecessors — a critical
edge. Naive phi elimination (insert copies at predecessor ends) is *technically incorrect
in general* there. It is safe in this probe only because the incoming values from `%0` are
constants. Recorded; the real backend needs proper critical-edge handling (splitting or
copy-in-predecessor with the correct liveness discipline).

### 16-bit arithmetic via bytewise carry is correct

The 16-bit add idiom used throughout —

```
MOVF  b+0,W ; ADDWF a+0,W ; MOVWF d+0
MOVF  b+1,W ; BTFSC STATUS,C ; ADDLW 1 ; ADDWF a+1,W ; MOVWF d+1
```

— ran correctly under simulation, including the carry from the low byte (`ADDLW 1` guarded
by `BTFSC STATUS,C`).

---

## Recommendation

ADR-001 stands: `.ll`-text-as-interface is confirmed viable. The IR surface is tractable,
clang's ABI is pointer-based and friendly to a no-stack backend, and the front end is not
fighting us.

The design updates the spike earned, in order of importance:

1. **Storage allocation is first-version-hard.** 16 bytes of common RAM is tighter than
   assumed; interference-graph colouring, function-scoped allocation, and a bank-0 spill
   path all land in the first working allocator.
2. **Harvard `const` is the least-derisked and most bespoke part.** It needs a dedicated
   spike (RETLW lookup tables + PCLATH), because `llvm-mos` offers no prior art and the IR
   gives no help beyond the `constant` marker.
3. **The pointer layer (`FSR`/`INDF`) is the largest unexercised codegen risk.** `sret`,
   `byval`, `alloca`, and array/struct access all funnel through it, and the probe had no
   pointers. The integer spine must land with a real pointer story, not defer it.

The next design artifact should be a **pointer/`const`-in-flash design spike** covering GEP
lowering, `FSR`/`INDF` addressing, and RETLW-table codegen, since those are the two places
the spike could not exercise and where the remaining risk concentrates.
