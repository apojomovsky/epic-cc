# 11 — Pointer / `const`-in-flash spike: FINDINGS

**Status: COMPLETE — both lowering paths demonstrated running in simulation.**

The spike code lives in `spike/` (gitignored, throwaway) and extends the backend-spine
spike: the IR parser gained `getelementptr`, pointer operands on `load`/`store`, and
array/`constant` globals; the codegen gained `FSR`/`INDF` indirect access and `RETLW`
const-table reads; the simulator gained `INDF`/`FSR` aliasing, `PCL`/`PCLATH`, and `RETLW`.

The probe (a runtime RAM pointer **and** a `const`-table read) compiles and runs correctly:
**`out == 20` for `in == 1`**, cross-checked against `gpasm`.

---

## The probe

```c
volatile unsigned char in;
volatile unsigned char out;
static const unsigned char table[4] = {10, 20, 30, 40};
volatile unsigned char ram[8];

void main(void) {
    unsigned char i = in & 3;
    volatile unsigned char *p = ram + i;   /* runtime pointer into RAM */
    *p = table[i];                         /* const read (flash) -> store via ptr */
    out = *p;                              /* load via ptr */
}
```

`volatile` is essential: without it, `-O1` folds the pointer and the entire RAM array away
(see "optimization folds static pointers" below). Expected `out` for `in = 1` is `20`.

At `-O1` clang emits:

```llvm
@ram = global [8 x i8] zeroinitializer
@table = constant [4 x i8] c"\0A\14\1E("        ; 10, 20, 30, 40

%4 = getelementptr inbounds nuw i8, ptr @ram, i16 %3      ; &ram[i]
%5 = getelementptr inbounds nuw [4 x i8], ptr @table, i16 0, i16 %3  ; &table[i]
%6 = load i8, ptr %5                                     ; table[i]
store volatile i8 %6, ptr %4                            ; ram[i] = table[i]
%7 = load volatile i8, ptr %4                           ; ram[i]
store volatile i8 %7, ptr @out
```

---

## The core finding: the IR has one address space; the target has two

LLVM IR gives us **one flat address space**. A `load`/`store` through a pointer does not
distinguish RAM from flash. The PIC14 has two genuinely different memory systems:

| Kind | Access mechanism | Cost |
|---|---|---|
| RAM (`global`) | `FSR`/`INDF` indirect | cheap, `MOVF INDF,W` / `MOVWF INDF` |
| Flash (`constant`) | `RETLW` lookup table | a `CALL` + computed jump per read |

The only signal that separates them is the **`constant` marker** on the global. The
backend must therefore classify globals by that marker and lower a `load` through a
pointer-to-`constant` differently from a `load` through a pointer-to-`global`. This is the
one genuinely new thing the pointer spike surfaced, and it is cleanly identifiable — not a
tarpit.

---

## `FSR`/`INDF` lowering — works

A runtime pointer `p = base + offset` lowers to:

```asm
MOVF  offset, W    ; W = index
ADDLW base         ; W = base + index   (8-bit: bank 0 fits in FSR)
MOVWF FSR          ; point FSR at the byte
MOVF  INDF, W      ; load              (or: MOVWF INDF  for a store)
```

`FSR` is 8-bit, which covers bank 0 (and, with the `IRP` bit in `STATUS`, the full 512
bytes across all four banks). The spike only needed bank 0, so `IRP` was not exercised.

## `RETLW` const read — works, but it is a CALL

A `const` array read `table[i]` lowers to a **call into a computed-jump table**:

```asm
    MOVF  idx, W
    CALL  __read_table        ; RETLW *returns*, so the read must be a CALL

__read_table:
    ADDLW LOW(table)          ; W = idx + low-byte(table)
    MOVWF PCL                 ; computed jump into the table
table:
    RETLW 0x0A               ; 10
    RETLW 0x14               ; 20
    RETLW 0x1E               ; 30
    RETLW 0x28               ; 40
```

Three consequences the design must absorb:

1. **`RETLW` is a return**, so a const read is not a plain `load` — it is a `CALL`/return
   round-trip. Every dynamic `const` read costs a hardware-stack level and a jump.
2. **Page-crossing.** `ADDLW LOW(table); MOVWF PCL` writes only the low byte of `PC`. If the
   table crosses a 256-word page boundary, the carry is silently lost. The real backend must
   either pin tables off page boundaries or emit the `PCLATH` + carry fix-up.
3. **`PCLATH` must hold the table's page.** The spike ran entirely in page 0 (program ≪ 256
   words), so `PCLATH = 0` was the reset default. A real backend sets `PCLATH` explicitly
   and restores it after the read.

This is the least-derisked part of the whole design (the 6502 never had it, so `llvm-mos`
offers no prior art), and it now has a concrete, working shape.

---

## Optimization folds static pointers away

Measured across `-O0`/`-O1`/`-O2` on the same program:

- **`-O0`:** pointers are explicit — `alloca ptr`, `store ptr @s`, `load ptr`, `store
  through ptr`. Struct fields surface as constant-offset `getelementptr`.
- **`-O1`/`-O2`:** a pointer to a *known* global (`&s.a`, `&ram[i]` where the array is
  non-volatile) is folded into direct access or the memory is store-forwarded away. The
  only things that survive are **genuinely runtime pointers** (index from volatile input)
  and **`const`-table reads** (which cannot be folded).

Consequence: the `FSR`/`INDF` path only matters for pointers whose value is not statically
resolvable. `const`-in-flash always survives, because it is the one thing the optimizer
cannot eliminate.

---

## The IR surface is a small, bounded extension

The pointer/const spike added exactly three constructs to the parser beyond the 12 opcodes
of the first spike:

- `getelementptr` — address arithmetic (`base + offset`); both the single-index and
  array-index forms reduce to this.
- pointer operands on `load`/`store` (`ptr %n` vs `ptr @name`).
- array / `constant` globals (`[4 x i8] c"…"`, `[8 x i8] zeroinitializer`).

The first spike's conclusion holds, extended: **`.ll`-text-as-interface is tractable for
pointers and `const` too.** The one new piece of machinery is the RAM/flash split.

---

## Recommendation

The pointer layer is **tractable and demonstrably lowerable**. Two things need real design
attention before implementation, both now de-risked to a concrete shape:

1. **The RAM/flash split** must be a first-class part of the IR/lowering design. The
   backend tracks which globals are `constant` and routes loads through `FSR`/`INDF` or a
   `RETLW` helper accordingly. GEP on a pointer to a `constant` is a *flash address*, not a
   RAM address — the two must never be conflated.
2. **`RETLW`-table codegen** needs a PCLATH/page-crossing story. The spike used the
   page-0-only idiom; the real backend must handle tables that do not fit in a page and
   must manage `PCLATH` correctly (this is a known, solvable problem — XC8 and SDCC both
   emit this idiom).

Neither finding argues against ADR-001. The front end is not fighting us; the pointer ABI
(`sret`, `byval`, `alloca`) is pointer-based and friendly, and the two lowering mechanisms
(`FSR`/`INDF`, `RETLW`) are small and confirmed working end-to-end.

The next design step remains presenting the allocator/banking core and remaining sections
(2–4) for approval — now with the pointer/`const` risk de-risked rather than open.
