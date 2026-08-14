# 10 — Feasibility spike: INTERIM findings

**Status: INCOMPLETE — paused 2026-08-14 partway through.**
Two of four questions have real answers. Codegen and the simulator were not built.

The spike code lives in `spike/` and is **gitignored and throwaway**. It is not the
product, must not be built on, and can be deleted freely. Only this findings document is
committed.

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
`in = 5` is `out == 48` (t reaches 304; 304 mod 256 = 48). **Not yet verified by
execution** — that needs the simulator.

Known-good invocation (note: **unwrapped** clang, see [`09`](09-build-environment.md)):

```bash
"$PIC8_CLANG_UNWRAPPED" -target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc \
    -resource-dir "$PIC8_CLANG_RESOURCE_DIR" -o probe.ll probe.c
```

---

## ✅ Q2 — How much `.ll` surface must we parse? **ANSWERED: tractable.**

Measured, not estimated. A hand-written parser covering **12 distinct opcodes** handles the
entire probe program:

```
binop x3   br x1   br.cond x2   call x1   icmp x3   load x1
phi x3     ret x2  select x1    store x1  trunc x1  zext x1
```

The parser is ~330 lines of Rust including attribute stripping, and it was written and
working in one pass. **This is not a tarpit.** The construct list will grow — pointers,
GEP, arrays, aggregates, varargs, intrinsics are all still ahead — but the shape of the
work is clearly bounded and incremental.

**Recommendation: `.ll`-text-as-interface is confirmed viable.** ADR-001 stands.

One design note that fell out: the parser must aggressively **strip attributes it does not
model** (`noundef`, `nsw`, `nuw`, `nneg`, `tail`, `fastcc`, `range(i16 -32768, 255)`, …).
That is mechanical, but it must be deliberate rather than incidental, because a silently
mis-parsed attribute is a silent miscompile.

## ⚠️ Q3 — Does common RAM survive as an imaginary register file? **PARTIAL: it is tight.**

Measured on the probe: **16 SSA values requiring 26 bytes** of storage under naive
one-slot-per-value allocation. Common RAM (0x70–0x7F) is **16 bytes**. It overflows by 10
bytes — on a program of eleven lines.

**Conclusion: liveness-based storage reuse is mandatory, not an optimization.** This is a
sharper constraint than the design currently assumes. Two consequences worth carrying into
the design:

1. Stage 6 (`alloc`) cannot be "assign, then optimize later." Interference-graph colouring
   has to be in the first working version.
2. Common RAM will need a **spill path into bank 0 GPR** from day one. Since bank 0 and
   common RAM are both reachable with `RP1:RP0 = 00`, the spike would not have exercised
   banking at all — worth knowing that a green spike would **not** have validated BANKSEL.

This does not invalidate the approach — llvm-mos's 32 bytes are also nowhere near enough
without reuse, which is why their greedy regalloc spills. It does mean 16 bytes buys us
less headroom than hoped.

## ❌ Q1 — Is `.ll` a good substrate w.r.t. ABI decisions? **NOT ANSWERED.**

Requires codegen. Partial evidence gathered:

- **At `-O0`**, every local is an `alloca` with load/store traffic — verbose but
  structurally trivial, and allocas map directly onto our static frames.
- **At `-O1`/`-O2`/`-Oz`, allocas vanish entirely** for this program. Everything is SSA
  values and `phi` nodes. The feared "clang bakes in stack assumptions that fight us"
  problem **did not appear** at optimized levels for straight-line and loop code.
- No `byval` / `sret` appeared, but the probe has no aggregates. Structs and unions are
  where that risk actually lives, and they were not tested.

## ❌ Q4 — How bad is Harvard `const` data? **NOT ANSWERED, NOT TESTED.**

The probe contains no `const` tables. This remains the least-derisked part of the design.

---

## Other findings worth keeping

### Optimization level changes the problem qualitatively

| Level | Character of the IR |
|---|---|
| `-O0` | allocas everywhere, load/store traffic, calls preserved, no phis |
| `-O1` / `-O2` | no allocas, SSA + `phi`, `if` lowered to `select`, **`static` functions fully inlined** |
| `-Oz` | as above, plus loop rewriting into closed form, **arbitrary-width integers (`i17`) and intrinsics (`@llvm.smax.i16`)** |

The `-Oz` observation is documented with the actual IR in
[`09-build-environment.md`](09-build-environment.md).

### `static` functions are inlined away at -O1+

The original probe had `static int add(...)` and **the call disappeared entirely** at `-O1`.
`__attribute__((noinline))` was required to retain a call at all. Anyone testing calling
convention must account for this or they will be testing nothing.

### `if` becomes `select`

`if (i & 1) ... else ...` was lowered to `select i1 %12, i16 100, i16 %9` rather than
control flow. Our isel needs a `select` lowering (branch-based, or `BTFSC`-based skip) from
the start; it is not an exotic construct.

---

## How to resume

Everything below is unstarted. `spike/src/ir.rs` (parser) and `spike/src/main.rs` (stats
driver) are working; `cargo build` inside `nix develop` succeeds.

1. **Storage allocation** — assign each SSA value a slot. Fill common RAM 0x70–0x7F first,
   overflow into bank 0 GPR at 0x20–0x6F. Naive allocation is sufficient *for the spike*
   (26 bytes fits in bank 0 + common); real liveness reuse is a design-phase concern.
2. **Phi elimination** — insert copies at the end of each predecessor block. Note the
   probe has a **critical edge** (entry block `%0` has two successors; `%6` has two
   predecessors), so a naive implementation is technically incorrect in general even though
   it happens to be safe here because the incoming values from `%0` are constants. Record
   this rather than solving it.
3. **Instruction selection** for the 12 opcodes. 16-bit ops decompose bytewise through `W`
   with carry via `STATUS,C`. A correct 16-bit add is:
   ```
   MOVF  b+0,W ; ADDWF a+0,W ; MOVWF d+0
   MOVF  b+1,W ; BTFSC STATUS,C ; ADDLW 1 ; ADDWF a+1,W ; MOVWF d+1
   ```
4. **Emit `.asm`, assemble with `gpasm`** — do *not* write our own assembler for the spike.
   `gpasm -p p16f877a probe.asm -o probe.hex` is verified working
   ([`09`](09-build-environment.md)).
5. **Simulator** — decode 14-bit words from the Intel HEX. Only the emitted subset is
   needed. Preset `in = 5`, run, assert `out == 48`.
6. **Then answer Q1 and Q4**, and add a `const` array to the probe for Q4.

## Recommendation so far

Nothing found so far argues against ADR-001. The IR-text interface looks tractable and the
front end is not fighting us the way the risk register feared. The one design update the
spike has already earned is on **storage pressure**: 16 bytes of common RAM is tighter than
assumed, and interference-graph colouring belongs in the first working allocator rather
than a later optimization pass.
