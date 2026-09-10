# PIC baseline P4: const in flash via RETLW (issue #327)

Ephemeral implementation plan. Design of record: `docs/37` D-5 (approved
2026-09-09). Delete in the final commit (takeoff ritual).

## Shape (carried over from PIC14 `isel` + PIC14E P4, adapted)

Classic call site: `MOVLW PAGE(reader); MOVWF PCLATH; <index to W>;
CALL __read_N; park W; pclath restore; reload W`. Classic reader entry
(6 words): `MOVWF scratch / MOVLW HIGH(base) / MOVWF PCLATH /
MOVF scratch,W / ADDLW LOW(base) / MOVWF PCL`, then N x `RETLW k`.
Tables may straddle pages there (window reads); readers and functions
must each sit in one page (`reader_pages`, `window_align`, `.table`
asserts, `verify_page_fit`).

Baseline deltas (all confirmed in sim + encoder, P1):

- No PCLATH: page is STATUS PA0 (bit 5). CALL target = PA0:0:low8,
  PCL write = PA0:0:low8 (sim `write_f`/`exec_call_goto`).
- No ADDLW (D-6): `W += k` is `MOVLW k / ADDWF scratch,W`
  (`emit_add_w_const` exists).
- Reader + table must share one page's LOW half (CALL and PCL writes
  force PC<8> = 0). High halves are unreachable for tables and
  subroutines alike (D-5).
- 509 flash = 1024 words, 2 x 512-word pages. Code lives in page 0;
  tables pack page-0 low half first, spill to page-1 low half.

## Reader (4 words + N, numeric base, no directives)

`__read_N` at entry E, base B = E + 4, page P:

```
__read_N:
    MOVWF 0x07        ; stash index (scratch, common RAM)
    MOVLW 0xLL        ; LL = B & 0xFF (B in a low half, numeric)
    ADDWF 0x07, W     ; W = index + base (D-6 idiom)
    MOVWF PCL         ; PC = P:0:W
    RETLW b0 ... RETLW bN-1
```

Window safety: placement guarantees B + N <= low-half end and the
caller guarantees index < N, so index + B never carries into bit 8.

## Call site (uniform 3-word overhead, page-independent size)

```
<W = k + byte_off + terms>   ; new emit_const_index_w helper
<BSF|BCF> STATUS, 5          ; PA0 = table page (bit patched post-placement)
CALL __read_N
BCF STATUS, 5                ; always: restore caller page 0
; W = byte, no park (nothing clobbers W after CALL)
```

Uniform size breaks the layout cycle: emit once, measure code, place
tables, then patch the recorded PA0-set lines BCF->BSF in place for
page-1 tables (same length, no shift). `__start` gains `BCF STATUS, 5`
first (PA0 power-on state is not known-good, 1 word).

## Placement (in `select`, single pass + patch)

1. Emit prologue + funcs as today into buffers; count code words with a
   line classifier (labels/equ/org/blank/comments/list/radix/end = 0,
   else 1; every baseline emit is one word).
2. Code must fit the page-0 low half (cursor <= 0x100): every function
   is CALLed (`CALL main` included), and CALL cannot reach a high half.
   Else panic (loud, correct over silent).
3. Flash consts (`is_const`, not in `addrs`): N > 252 panics (no low
   half fits entry + table; no chunked shape on this core). First-fit:
   page-0 low half, else page-1 low half (`org 0x200`), else panic.
4. Append readers + RETLWs (numeric bases), then `end`.
5. Direct `load @const` diverts to the same path (constant index);
   memcpy src already flows through `emit_ptr_load_byte`. Stores to
   const keep panicking. Const call args keep the RAM-copy rule
   (alloc, unchanged).

## `verify_page_fit` (real, replaces the no-op stub)

Scan final asm (org/label tracking): every non-`__read_` label must sit
below 0x100; every `__read_N` + 4 + N must end inside one page low
half, with exactly N RETLWs following; every CALL target must be in a
low half whose page equals the nearest preceding PA0 set (default 0);
total <= flash_words. Driver wires it into the PicBaseline arm (today
only the e2e harness calls it).

## Fixtures (isel-pic-baseline e2e + sim)

- `const_table.c`: ~96-byte table, runtime index reads + one u16 const
  read, all placed page 0. Proves the read path end to end.
- Spill fixture (~200-byte table + normal code): code + table exceeds
  256 words, table relocates to page-1 low half with PA0
  set/restore. Proves relocation.
- Regression (`should_panic` through the harness): 300-byte table fits
  neither low half. Proves loud rejection, never silent miscompile.
- gpasm HEX-identity over the emitted page-0 asm (`-p p12f509`) plus
  sim execution, mirroring `gpasm_const_table.rs`.

## Non-goals

Chunked (>252B) tables, multi-page code, page-1 subroutines, FSR-flash
mapping (same class of deferred follow-up as docs/33 D-5). Inline
`module_asm` with multi-word pseudo-ops miscounts the classifier;
fixtures do not use it.
