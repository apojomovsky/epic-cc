# PIC baseline P3: pointers/arrays via FSR flat addressing

Ticket: epic-cc#326. Design of record: docs/37-pic-baseline-port-design.md
(§2 D-2 item 2). Depends on P2 (epic-cc#325, merged).

## Goal

Pointers and arrays using `FSR`'s natural representation as a flat
address (bank bits + offset together), per D-2 item 2. Array indexing and
struct field access through `FSR`/`INDF`, with D-2 item 3's ordering
discipline. Structs via whole-program static allocation.

## What P2 already provides

P2 forked the full pointer machinery from classic `isel` into
`isel-pic-baseline`:

- `emit_ptr_setup` -> `Addr::Direct|Indirect`: a pointer reg resolves to
  `(base, k, terms)` via `iselcore::resolve_pointers`; a static base with
  no terms is a direct file-register access, a dynamic base sets FSR and
  goes through INDF.
- `emit_fsr_to` / `emit_fsr_indirect`: load the full 6-bit flat address
  (bank + offset) into FSR in one MOVLW/MOVWF, no FSR0H/FSR0L split, no
  linear alias, no IRP (D-2 item 2).
- `emit_ptr_load_byte` / `emit_ptr_store_byte` / `emit_ptr_store_w`:
  byte accesses through INDF.
- `emit_accum_terms`: `scratch = Σ scale×%reg` for multi-term GEPs.
- `emit_move_addr_to_slot` / `emit_select`: pointer-value materialization
  and pointer selects.
- `emit_call_args`: byval/sret/plain-ptr arg passing.
- `emit_load_byte` / `emit_move_val_to_slot`: value copies.

The P2 e2e `banked.c` already exercises a genuinely runtime pointer deref
through a volatile pointer global, interleaving direct bank-1 accesses
with an INDF touch into bank 0 (and vice versa).

## The P3 gap: dynamic memcpy

`MemLen::Reg` panics ("dynamic memcpy not supported in P2"). Classic
`isel`'s `emit_memcpy_dynamic` (crates/isel/src/lib.rs:704-795) lowers a
runtime-length copy to a byte loop: countdown = len, idx = 0, per byte
re-set FSR to `base + k + terms + idx`, copy src[i] -> hold -> dst[i],
idx++, countdown--. It uses `retval_lo` (0x71) for the 16-bit countdown
and free common bytes 0x7E/0x7F for idx/hold.

Baseline's common RAM is only 0x07-0x0F (9 bytes): scratch 0x07, retval
0x08-0x0B, scratch2 0x0C, leaving 0x0D-0x0F (3 bytes) free. The dynamic
memcpy needs 4 temp bytes (cnt_lo, cnt_hi, idx, hold). Adapt: use
scratch2 (0x0C) as one temp and 0x0D-0x0F as the other three, or reuse
the retval region (dead at a memcpy) for the countdown like classic isel
does. The countdown is 16-bit (len is i16); idx is 1 byte (bounded by
the 6-bit FSR space, so idx <= 0x3F).

The classic loop uses `ADDWF FSR, F` to add idx to FSR, valid on baseline
(FSR is a file register). But the FSR re-set per byte must
re-assert the bank bit: `emit_fsr_to` already loads the full flat
address, so the idx add happens after, keeping the bank bit. The
countdown decrement uses `MOVLW 1; SUBWF lo,F; BTFSS STATUS,0; SUBWF
hi,F`; all common RAM, no bank reassertion needed.

## P3 fixtures (from the sibling backends, adapted to the 509's 41-byte GPR budget)

- `ptr_probe.c`: a runtime RAM pointer (FSR/INDF) with a volatile index.
  The const-table (RETLW) read is P4, so the fixture drops the `table[i]`
  read and keeps only the RAM pointer path: `*p = i+1; out = *p`.
- `array.c`: a non-const array written and read at a runtime index, the
  pure FSR/INDF path. `in` is a 16-bit volatile so clang keeps the index
  mask as an i16 `and`.
- `structs.c`: byval calls (sum/pick) plus a dynamic array-in-struct
  (arr.v[arr.n]), sized to the 509's budget (struct Pair = 4 bytes,
  struct A = 5). The sret call and nested Outer struct of the PIC14E
  fixture are dropped: their return/pass-through structs would exceed
  the budget.
- The bank/interleave case comes from the P2 `banked.c` e2e, which
  interleaves bank-1 direct stores with flat-loaded FSR derefs into bank
  0 and back, with asm-level BSF/BCF assertions; P3's fixtures add the
  runtime-index array paths on top.

## Acceptance

E2e fixtures for pointer arithmetic, array indexing, and struct field
access via FSR/INDF, compiled through the real pipeline and matched
against the `PicBaseline` sim, including a case that interleaves a
direct access to one bank with an indirect access through a pointer into
the other bank.

## Verification

- `make test CRATE=isel-pic-baseline` in the container.
- `make check-warnings` clean.
- The fixtures fail if the FSR flat-address materialization or the D-2
  ordering discipline is wrong.

## Out of scope

- Const in flash via RETLW, 256-word ceiling (P4).
- i32 (`long`), mul/div/shift runtime routines (P6).
- Soft-float (P7), fuzz gate (P8).
