# PIC baseline P2: isel-pic-baseline integer spine + D-2 FSR bank-bit reassertion

Ticket: epic-cc#325. Design of record: docs/37-pic-baseline-port-design.md
(§2 D-1, D-2; §1 "What is new"/"What is adapted"). Depends on P1
(epic-cc#324, merged).

## Goal

- New `isel-pic-baseline` crate (D-1): the integer spine (i8/i16
  add/sub/and/or/xor, the ten `icmp` predicates, `call`/`ret`, `phi`
  elimination, direct + FSR/INDF load/store) plus D-2's unconditional
  `FSR` bank-bit reassertion, owned entirely inside the crate (no
  `crates/banking` role).
- `driver`: third backend-selection arm, firewall lifted for this core.
- An e2e fixture exercising basic integer arithmetic and both direct and
  indirect accesses to both banks, compiled through the real pipeline and
  matched against `sim`.
- D-1's crate-sharing question answered in writing.

## D-1 answer (recorded in the design doc)

Baseline's ISA is a strict subset of classic PIC14's: the same
byte/bit/literal three-way field vocabulary (`d`/`f`/`b`/`k`), missing
exactly `SUBLW` and `RETURN` (docs/37 §1). So `isel-pic-baseline` shares
the integer-spine *structure* with classic `isel` (the `Gen` struct,
`emit_inst`/`emit_terminator`/`emit_func_body`, the `MOVF`/`MOVWF`/
`ADDLW`/`ADDWF`/`BTFSC`/`GOTO` idiom vocabulary) but owns its
instruction-emission code, exactly as `isel-pic14e` does. It does NOT
share emission with `isel-pic14e` either: the two differ in the
destination default (baseline d=1, PIC14E d=1 too but PIC14E has the
enhanced FSR0/1 machinery baseline lacks), the bank mechanism (baseline
`FSR<5>` vs PIC14E `MOVLB`/`BSR`), and the return idiom (baseline
`RETLW 0` vs PIC14E `RETURN`). The clean-room framing in D-1's open
question resolves to: fork classic `isel`'s integer spine, adapt the
three ISA deltas, and add the FSR bank-bit reassertion inline.

## ISA deltas from classic isel (all confirmed in P1)

1. **No `SUBLW`** (D-6): `w := k - f` lowers to the 3-instruction idiom
   `MOVWF tmp` / `MOVLW k` / `SUBWF tmp,W` needing one scratch register,
   instead of PIC14's single `SUBLW k`. `emit_sub_const_lhs` is adapted
   to this idiom.
2. **No `RETURN`** (D-6): every function return is `RETLW 0` (void) or
   `RETLW k` (valued, after copying the value to the retval slots).
3. **Destination default d = 1** (file), matching gpasm on this core
   (P1 confirmed). Classic isel defaults to W; baseline must default to F.
4. **No PCLATH**: baseline's PC<9> comes from STATUS PA0 (bit 5), and
   CALL/RETLW are hard-limited to the low 256 words of a page (D-5).
   For P2 the fixtures fit in page 0, so PA0 stays 0 and no page
   management is emitted. `verify_page_fit` is a no-op for P2 (the
   page model is P4's concern with const tables).
5. **No interrupts**: no ISR emission, no `RETFIE`, no INTCON.

## D-2: FSR bank-bit reassertion (the load-bearing new code)

On baseline, `FSR<5>` is the bank select for BOTH direct and indirect
addressing, and `FSR` is also the only indirect pointer. The resolution
(docs/37 §2 D-2, corrected by review): emit `BCF`/`BSF FSR,5`
unconditionally before every direct access to a non-zero bank and before
every `INDF` touch, the same way classic PIC14 unconditionally re-emits
IRP inside `isel` (`crates/isel/src/lib.rs:809-870`) rather than
tracking state in `crates/banking`.

Concretely, in `isel-pic-baseline`:

- A direct operand `f` whose physical address is in bank 1 (0x30-0x3F)
  is preceded by `BSF FSR,5`; a direct operand in bank 0 (0x10-0x1F) is
  preceded by `BCF FSR,5`. The shared GPR (0x07-0x0F) and SFR block
  (0x00-0x06) are bank-independent and need no reassertion.
- Every `INDF` access (a pointer deref through FSR) is preceded by
  `BCF FSR,5` (the pointer's own bank bits are loaded with the pointer
  value, so the reassertion before the INDF touch is what guarantees the
  pointer's bank, not a stale direct-access bank).
- The reassertion is unconditional: no dataflow tracking, no
  interference analysis, exactly like PIC14's IRP. This is simpler and
  lower-risk than a tracked mechanism, and it is what the design doc
  mandates.

The bank of a physical address is determined by `device.ram_banks`
(0x10-0x1F bank 0, 0x30-0x3F bank 1). A helper `emit_bank_select(addr)`
emits the `BCF`/`BSF FSR,5` when `addr` is a banked GPR, and is called
before every direct file-register operand and every INDF access.

## Crate structure (mirrors isel-pic14e)

- `crates/isel-pic-baseline/Cargo.toml`: deps `ir`, `device`, `iselcore`
  (no `banking`, no `peephole`; baseline needs neither). Dev-deps
  `pic14-sim`, `asm`, `irparse`, `wholeprog`, `legalize`, `callgraph`,
  `alloc`. `[[bin]]` CLI like the others.
- `src/lib.rs`: `Gen` struct (m, addrs, device, resolved, scratch,
  retval_lo, cur_func, tmp, w_holds, cur_loc, out, locs), `emit`,
  `emit_w_store`/`emit_w_load`, `emit_bank_select`, `emit_inst`,
  `emit_terminator`, `emit_func_body`, `select`/`select_with_locs`,
  `verify_page_fit` (no-op for P2), `pub use iselcore::parse_map`.
- `src/bin/isel-pic-baseline.rs`: CLI.
- `tests/e2e.rs`: the C-through-pipeline acceptance.
- `tests/fixtures/`: `add.c`, `scalar.c`, `banked.c` (the P2 set).

## Integer spine scope (what to fork from classic isel)

Fork these from `crates/isel/src/lib.rs`, adapting the ISA deltas:

- `emit_inst` dispatch: `Load`/`Store` (direct + FSR/INDF), `Gep`/
  `Alloca` (virtual), `Memcpy` (const length), `Bin` (i8/i16
  add/sub/and/or/xor), `Zext`/`Sext`/`Trunc`, `Icmp` (via
  `emit_cmp_eq`/`emit_cmp_c`/`emit_materialize`), `Select`, `Call`/
  `emit_indirect_call`, `Freeze`, `IntToPtr`. i32 and mul/div/shift
  panic (P6). Float panics (P7).
- `emit_terminator`: `Br` -> GOTO, `BrCond` -> `emit_cond_branch`, `Ret`
  -> copy to retval slots then `RETLW 0` (void) / `RETLW k` (valued).
- `emit_func_body`: block walk, phi-copy elimination, no ISR prologue.
- `emit_routine`: NOT in P2 (mul/div/shift routines are P6). A
  legalize-injected routine call panics with a clear message.
- `emit_cond_branch`, `emit_cmp_eq`, `emit_cmp_c`, `emit_materialize`,
  `emit_select`, `emit_add16`, `emit_commutative`, `emit_sub8`,
  `emit_sub16`, `emit_sub_const_lhs` (adapted to the no-SUBLW idiom),
  `emit_move_val_to_slot`, `emit_load_byte`, `emit_xor_byte`,
  `emit_call_args`, `emit_call`, `emit_indirect_call`.
- Pointer lowering: `emit_ptr_setup` -> `Addr::Direct|Indirect`,
  `emit_ptr_load_byte`/`emit_ptr_store_byte`, `emit_fsr_to`,
  `emit_fsr_indirect`. Baseline's FSR is 6 bits (bank + offset), so
  `emit_fsr_to` loads the full flat address into FSR in one MOVLW/MOVWF
  (no FSR0H/FSR0L split, no linear alias, no IRP). A pointer's natural
  representation is FSR's full value (D-2 item 2).

## Driver changes

- `crates/driver/src/main.rs:72-76`: remove the PicBaseline firewall.
- `crates/driver/src/main.rs:330-336`: add
  `device::Core::PicBaseline => isel_pic_baseline::select_with_locs(...)`.
- `crates/driver/src/main.rs:341-371`: baseline runs NO schedule/banking/
  peephole (D-2 is inline in isel-pic-baseline; there is no banking
  pass). The `PicBaseline` arm returns `asm` directly like Pic18.
- `crates/driver/tests/device_flag.rs:312-338`: the
  `device_flag_refuses_pic_baseline_at_the_firewall` test must be
  updated: the driver now compiles p12f509 instead of refusing it.
- `scripts/sanity.sh` greps for "no backend yet": update if it still
  expects the baseline firewall message.

## Acceptance test (tests/e2e.rs)

`compile(c_path)` runs clang -> irparse -> wholeprog -> legalize ->
callgraph -> alloc -> `isel_pic_baseline::select` -> `assemble_words` ->
`PicBaseline::with_device(&PIC12F509, words)`, seeds RAM by global name,
runs, asserts RAM values and `halted()`. Fixtures:

- `add.c`: basic i8/i16 add/sub/and/or/xor arithmetic.
- `scalar.c`: the PIC14 scalar fixture (i8/i16 ops, icmp, branches).
- `banked.c`: a fixture with globals in both banks plus a pointer
  (FSR/INDF) deref, exercising D-2's `BCF`/`BSF FSR,5` sequencing
  end-to-end through real codegen. The `.asm` is inspected to assert the
  reassertion instructions are present before bank-1 direct accesses and
  before INDF touches.

## Verification

- `make test CRATE=isel-pic-baseline` in the container.
- `make check-warnings` clean.
- The e2e `banked.c` test fails if D-2's reassertion is missing or wrong
  (the sim would resolve the wrong bank).

## Out of scope

- i32 (`long`), mul/div/shift runtime routines (P6).
- Pointers/arrays beyond the FSR/INDF deref needed for the D-2 test (P3).
- Const in flash via RETLW, 256-word ceiling (P4).
- Soft-float (P7), fuzz gate (P8).
- `p12f508`/`p16f505` TOMLs.
