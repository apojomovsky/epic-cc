# 37: PIC baseline core port design

> **Approval status:** approach and phasing approved by the user on
> 2026-09-09. This document exists to unblock the "revisit if a real
> consumer wants one" condition set in
> [issue #229](https://github.com/apojomovsky/epic-cc/issues/229)
> (closed, tracking-only) and restated in `docs/08-status-and-next-steps.md`.
> **No real consumer has appeared yet** (no PlatformIO board JSON, no
> epic-hal baseline-core crate); this document was requested directly,
> ahead of that condition, as a deliberate choice to have the design
> ready rather than wait. An independent review pass ran on 2026-09-09
> (per this repo's review-gate norm) and caught one substantive error:
> D-2's original wording claimed its ordering-hazard fix followed the
> same pattern `crates/banking` already uses for RP1:RP0/BSR, which is
> false (`crates/banking` never touches `INDF`/`FSR`); D-2 has been
> corrected in place to point at the real precedent, PIC14's
> unconditional IRP handling inside `isel`. §4 lists what is still
> open: D-2's corrected resolution is a paper argument from the
> datasheet's bit-level semantics and the existing PIC14 code, not yet
> simulator-proven (P1's acceptance criteria are written to close that
> gap first). `docs/33-pic14e-port-design.md`'s own history remains the
> standing caution that a first pass, even a reviewed one, can still
> miss things: its revision 1 and 2 were held over an uncited claim
> about OPTION/TRIS that turned out to be wrong. Every architectural
> claim below cites a `DS41236E` page number except where marked
> `[VERIFY]`.

**Target:** the PIC12F508/PIC12F509 family (PIC16F505 shares the same
core (33-instruction baseline ISA, 12-bit instruction word, 2-level
stack) with more I/O pins and a `PORTC`, noted but not scoped here).
Chosen over PIC10F2xx (smaller, more marginal: 24 bytes RAM, 256 words
flash on PIC10F200) and PIC16F54/57/59 (larger, but PIC12F508/509 is
the more commonly cited hobbyist baseline part and the smaller of the
two is the harder, more informative case to design against first).

**Datasheet:** `vendor/microchip/datasheets/DS41236E.pdf` (PIC12F508/
509/16F505 Data Sheet, 2009) plus its errata, `DS80000190H.pdf`, both
vendored for this document. No baseline-specific family reference
manual equivalent to `DS33023` (PIC14) or `DS41364E` (PIC14E) appears
to exist (searched twice, see §4); baseline's core architecture lives
entirely in the per-family datasheet, used as the sole source
throughout.

---

## 1. Why this port is a different shape, not a smaller one

Every existing port (`docs/29` for PIC18, `docs/33` for PIC14E) was
framed as "smaller than a second compiler" because each *relaxes* a
PIC14 constraint. Baseline does the opposite: it is a step down from
PIC14 in almost every dimension that made PIC14 hard, plus one entirely
new problem PIC14 doesn't have at all.

| Constraint | PIC14 (shipped) | PIC12F508/509 baseline | Consequence |
|---|---|---|---|
| Hardware call stack | 8 levels | **2 levels**, DS41236E §4.8, page 27 | Existing `stack_depth`-based rejection (already in `device::Device`, already enforced by `callgraph`) generalizes mechanically, but a ceiling of 2 rejects almost any non-trivial call graph. This is a real usability question, not a technical blocker, see D-4. |
| Bank selection | `RP1:RP0` bits in `STATUS`, `BANKSEL` pseudo-op | **`FSR<5>` (509) / `FSR<6:5>` (16F505)**, and, unlike every other core, **the bank bits apply to *direct* addressing too**, not just indirect (DS41236E Figure 4-7/4-8, page 28-29) | There is no `BANKSEL`-equivalent instruction at all. Selecting a bank for a plain `MOVWF 0x15` requires first writing `FSR`'s high bits, and `FSR` is *also* the only indirect-addressing pointer register. Direct-access bank switches and pointer dereferences share one register. New problem, not present on any shipped core, see D-2. |
| Const/jump table reach | `RETLW` table anywhere in a 2048-word page (511-byte practical ceiling, `docs/33` D-5 precedent) | `GOTO` reaches a full 512-word page (9-bit literal), but **`CALL` and any PCL-modifying instruction (`RETLW` included) are hard-limited to the low 256 words of a page**: PC bit 8 is forced to 0 by every such instruction except `GOTO` (DS41236E §4.7, page 27, and Note 1 under Table 8-2) | Subroutines and `RETLW` jump/const tables must be placed in the bottom half of whichever page they're linked into; `GOTO`-only straight-line code can use the full page. Tighter than PIC14's ceiling, and on a much smaller total flash (512/1024 words total, not 8K). |
| Interrupts | One vector, SFR-driven | **None**: no interrupt vector, no `RETFIE`, DS41236E §7.0 lists no interrupt feature | A whole phase (P5 in both prior ports) simply does not exist here. Net simplification. |
| Instruction count | 35 | **33**, missing `ADDLW`, `SUBLW`, `RETURN` and `RETFIE`, adding `OPTION` and `TRIS` (compare Table 8-2 here against PIC14's ISA; the "missing exactly SUBLW and RETURN" wording of an earlier draft was wrong, gpasm 1.5.2 rejects `ADDLW` on p12f509, confirmed 2026-09-10) | `w := k - f` needs a scratch-register idiom (`MOVWF`/`MOVLW`/`SUBWF`) instead of one `SUBLW`; `w := w + k` needs the same idiom (`MOVWF`/`MOVLW`/`ADDWF`) instead of one `ADDLW`; every subroutine return needs a literal (`RETLW k`, even `RETLW 0` for void) since there is no bare `RETURN`. Peephole/isel-level, not architectural. |
| RAM (GPR only, excludes SFRs) | 368 bytes (16F877A) | **25 GPR bytes total** on the 508 (9 unbanked, `0x07-0x0F`, + 16 more at `0x10-0x1F`, no banking, `FSR<7:5>` unimplemented) or **41 GPR bytes total** on the 509 (the same 9 unbanked + 16 in bank 0 `0x10-0x1F` + 16 more in bank 1 `0x30-0x3F`) (DS41236E §4.3, page 18, Figure 4-3/4-4) | Even smaller working set than PIC14E's per-bank 128 bytes. Whole-program static allocation is still the right model; there is simply very little to allocate. |

**The one genuinely new problem is D-2** (bank-select-via-FSR
colliding with pointer dereference). Everything else is either a
mechanical narrowing of a decision the existing `device`/`callgraph`
plumbing already makes device-parametric, or a net simplification
(no interrupts).

### What carries over unchanged

- Whole-program compilation, `clang -target msp430` as the datalayout
  proxy, out-of-process clang front end (ADR-001, ADR-002, unaffected
  by target core).
- `Slot::Direct`-only static allocation (`docs/29` §2 D-2's `Slot`
  abstraction already anticipates this; baseline never needs
  `Slot::Frame` any more than PIC14 does, and less so given the 2-level
  stack makes a software frame pointer even less attractive than on
  PIC14).
- `device::Device` and the TOML registry shape (ADR-019); baseline
  needs new fields exactly the way PIC14E didn't (no `linear_ram`,
  might need an `fsr_bank_bits: u8`, see D-3).
- The gpasm byte-for-byte oracle. Confirmed (§3): `gpasm-1.5.2` in the
  pinned dev image ships `p12f508.inc`, `p12f509.inc`, and
  `p16f505.inc`.

### What is adapted

- `asm`/`sim`: new encoder and simulator core for the 12-bit word and
  the 33-instruction set (this is the same shape of work `docs/29` P1
  and `docs/33` P1 did for their cores).
- `callgraph`: no code change needed. Confirmed by reading the actual
  call site, not just the field's existence:
  `crates/driver/src/main.rs:301` calls
  `callgraph::check_depth(&cg, device.stack_depth as usize)`, already
  fully device-parametric. A baseline device just sets `stack_depth`
  to 2 and the existing rejection path applies unchanged.
- `driver`: a third backend-selection arm, same shape as adding
  PIC14E's arm was in `docs/33` D-3/D-4.

`crates/banking` likely needs **no** baseline-specific change at all,
corrected from an earlier draft of this section that proposed a
fourth banking regime there (an independent review caught that this
contradicted D-2's actual resolution, see D-2). PIC14 itself already
splits bank-select handling in two: `crates/banking` tracks RP1:RP0
for *direct* accesses only, while the IRP bit for *indirect*
(FSR-based) accesses is re-emitted unconditionally, with no tracking,
entirely inside `isel` (`crates/isel/src/lib.rs:809-870`). Baseline
has no such split; one register, `FSR`, carries the bank bits for
both addressing modes, so D-2 resolves it uniformly and
unconditionally inside `isel-pic-baseline`, the same place PIC14
already puts its own unconditional case. `crates/banking`'s role, if
any, is not yet known and should be revisited once P1/P2 show whether
anything is left for it to do on this core.

### What is new

- `Core::PicBaseline` (new enum variant in `crates/device/src/lib.rs`).
- `isel-pic-baseline`, a new crate, mirroring `isel-pic18` and
  `isel-pic14e`'s relationship to `iselcore` (shares `Slot`, `ssa_key`,
  `Base`, `resolve_pointers`; does not share instruction-emission code
  with either, same as the existing three backends don't share it with
  each other). Owns D-2's unconditional `FSR` bank-bit reassertion,
  the same way `isel` owns PIC14's unconditional IRP reassertion.
- A device profile for `p12f509` (and, if scoped in, `p12f508`,
  `p16f505`).

---

## 2. Decisions

### D-1: A fourth parallel backend crate, `isel-pic-baseline`

Same reasoning as `docs/33` D-1 for PIC14E: the ISA is different enough
(different opcode encodings entirely, this is not the classic-PIC14
encoding at all, unlike PIC14E which kept PIC14's byte/bit/literal
opcode families verbatim) that sharing `isel`'s instruction-emission
code would mean scattering baseline special cases through PIC14's
backend. A new crate isolates the failure mode structurally, the same
argument `docs/33` D-1 made for `isel-pic14e`. **Resolved in P2
(epic-cc#325)**: `isel-pic-baseline` shares the integer-spine
*structure* with classic `isel` (the `Gen` struct, the
`emit_inst`/`emit_terminator`/`emit_func_body` skeleton, the
`MOVF`/`MOVWF`/`ADDLW`/`ADDWF`/`BTFSC`/`GOTO` idiom vocabulary) but owns
its instruction-emission code, exactly as `isel-pic14e` does. It does
not share emission with `isel-pic14e` either: the two differ in the
bank mechanism (baseline `FSR<5>` vs PIC14E `MOVLB`/`BSR`), the return
idiom (baseline `RETLW 0` vs PIC14E `RETURN`), and the FSR machinery
(baseline's single 6-bit FSR vs PIC14E's 16-bit FSR0/1 with linear
alias). The four ISA deltas from classic `isel` are `ADDLW` (absent,
D-6), `SUBLW` (absent, D-6), `RETURN` (absent, D-6), and the d=1
destination default; the FSR-bank quirk is layered on top as D-2's
unconditional reassertion.

### D-2: Bank selection via `FSR` and pointer dereference share a register, resolved via bit-granular writes

This is the load-bearing new problem this port has that none of the
other three do. On baseline, `FSR<4:0>` is the indirect-addressing
offset (used with `INDF`) **and** `FSR`'s high bits (`FSR<5>` on the
509, `FSR<6:5>` on the 16F505) select which bank *both* direct and
indirect addressing land in (DS41236E Figure 4-7/4-8). There is no
second, dedicated bank register the way PIC18/PIC14E have `BSR`
separate from `FSR0/1/2`.

**Resolved, this revision.** The apparent collision dissolves once the
instruction encoding is read precisely rather than assumed by analogy
to `BANKSEL`/`MOVLB`:

1. **`BCF`/`BSF` are single-bit operations that touch nothing else.**
   Table 8-2: `BCF f,b` is `0 → (f<b>)`, `Status Affected: None`,
   same for `BSF`. `BCF FSR,5` / `BSF FSR,5` (and `,6` on the 16F505)
   changes exactly that one bank bit and leaves `FSR<4:0>`, a live
   pointer's offset if any, completely untouched. The banking pass
   never needs a masked read-modify-write; the ISA already provides
   the bit-granular instruction. This is the same pattern `docs/33`
   used for RP1:RP0 on PIC14 (`BCF`/`BSF STATUS,5/6/7`), just moved to
   a different register.
2. **A pointer's natural representation on this core is FSR's full
   value, bank bits and offset together, loaded in one shot.** When
   the compiler materializes a pointer (e.g. `&x` or a computed
   address) it does `MOVLW <addr>` / `MOVWF FSR`, which sets bank and
   offset atomically as a single 5/6/7-bit flat address; there is no
   separate "set the bank, then set the offset" step for a pointer,
   because a compiled pointer value already encodes both. This mirrors
   how `docs/33` D-2 makes the enhanced-core linear region "an
   addressing-mode choice, not a placement pool": here, baseline's
   flat `FSR` address space *is* the addressing mode, for free, with
   no separate linear-region concept needed at all.
3. **The remaining hazard is ordering, not clobbering**, and it is
   the same shape of problem the existing PIC14 `banking` pass already
   solves: `FSR<6:5>` is one piece of "last known bank" state read by
   *both* addressing modes, so a direct access to bank B followed by
   an indirect access through a pointer that expects bank A needs bank
   A's bits re-asserted (via `BCF`/`BSF`) before the `INDF` touch.
   **Corrected in this pass, an independent review caught the original
   wording:** the precedent for this is not `crates/banking`'s
   RP1:RP0/BSR tracking (`crates/banking` never touches `INDF`/`FSR`
   at all, checked directly). The real precedent is classic PIC14's
   IRP handling, entirely inside `isel`
   (`crates/isel/src/lib.rs:809-870`, `emit_fsr_to`/
   `emit_fsr_indirect`): IRP is re-emitted **unconditionally on every
   FSR setup**, direct or indirect, with no dataflow tracking at all
   ("IRP is set on EVERY FSR setup", per that code's own comment).
   Baseline should copy that strategy exactly, inside
   `isel-pic-baseline`, not invent a new tracked/lattice mechanism in
   `banking`: emit `BCF`/`BSF FSR,5` (and `,6` on the 16F505) before
   every direct access to a non-zero bank and before every `INDF`
   touch, unconditionally, the same way PIC14 never bothers proving a
   reassert was redundant. This is simpler and lower-risk than the
   tracked approach the previous wording proposed, and it is what both
   existing cores that already solved an equivalent problem actually
   do, not an analogy to what they do.

Net effect: `isel-pic-baseline` never emits a masked FSR write, never
needs new interference analysis, and never needs a new tracked-state
mechanism in `crates/banking` at all. `crates/banking` for baseline
may end up doing nothing D-1's "adapted" framing implied; revisit
that framing once P1/P2 show whether `banking` has any role here
beyond what `isel-pic-baseline` handles inline, PIC14's own
IRP-in-isel precedent suggests it might not.
**Not yet re-verified against a working prototype**: this is a
paper resolution from the datasheet's bit-level operation semantics
and the existing PIC14 precedent, not a simulator-confirmed one;
treat as high-confidence, not proven, until P1's hand-written `.asm`
tests exercise the sequence.

### D-3: Device profile sketch, corrected as datasheet facts, not final

```toml
name = "p12f509"
core = "pic-baseline"
flash_words = 1024           # DS41236E §4.1, page 17: 1K x 12, physically implemented
ram_banks = [ ... ]          # 0x07-0x0F unbanked (9 GPR); 0x10-0x1F bank 0 / 0x30-0x3F
                              # bank 1 (16 GPR each), DS41236E Figure 4-4, page 19
common_ram = [0x07, 0x0F]    # RESOLVED in P0 (epic-cc#323): the sketch's
                              # [0x00, 0x06] would mark SFR space
                              # allocatable; the field means the
                              # allocatable BANKSEL-free window, which on
                              # the 509 is the 9 shared GPR at 0x07-0x0F
                              # (gputils' unprotected SHAREBANK
                              # `gprnobnk`). The SFR mirror at 0x00-0x06
                              # is core architecture, modelled in
                              # `bank_of`'s mirrored block like PIC14E's.

`fsr_bank_bits` (name settled in P0) is a new field:
how many of `FSR`'s high bits are bank-select (0 for 508, 1 for 509, 2
for 16F505); this is a real per-device fact the way `stack_depth` and
`ram_banks` are, not a derived quantity.

**[VERIFY] items resolved in P0 (epic-cc#323).** The config word's
address: DS41227B section 2.3 places it at 0x7FF in the 509's config
memory space (0x3FF for the 508), unreachable at runtime; the HEX
convention of MPASM and gputils (`p12f509.inc`'s
`_CONFIG EQU 0FFFh`, `.config 0xFFF` in the `.lkr`, confirmed byte for
byte against a gpasm 1.5.2 probe) is word 0xFFF, byte 0x1FFE, which is
what `base_byte_addr` models, exactly as word 0x2007 models the PIC14's.
Both sources and the bit layout (DS41227B Figure 4-1) are cited in the
TOML's comments. The `ram_banks` counting question dissolved the same
way: the shipped convention is allocatable GPR only, and gputils' own
`.lkr` draws the same three regions the datasheet does.

### D-4: Call-depth 2, reject not inline, matching existing policy

Every shipped core already rejects call graphs deeper than
`stack_depth` (recursion on PIC14, and by construction anywhere
recursion or excess depth would overflow the hardware stack) rather
than inlining to fit. Baseline should do the same: **no new inliner,
v1 just inherits the existing rejection path with `stack_depth = 2`.**
This is a policy call worth stating explicitly rather than leaving
implicit, because 2 is so much smaller than PIC14's 8 that "most
non-trivial C programs get rejected" is a real, foreseeable outcome,
not an edge case. Whether that makes the target worth shipping at all
is a product question for the user, not resolved by this document.

### D-5: `RETLW`/`CALL` 256-word half-page ceiling

Carries over the shape of `docs/33` D-5 (const-in-flash via `RETLW`,
a follow-up owns anything fancier) but the ceiling is a fixed 256
words per page here, not `docs/29`/`docs/33`'s per-family figures,
because it comes from the instruction encoding itself (PC bit 8 forced
to 0 by every PCL-modifying instruction except `GOTO`, DS41236E §4.7)
rather than from a device-specific flash size. Applies identically to
508 (1 page) and 509/16F505 (2 pages via the `STATUS` `PA0` bit).

### D-6: `ADDLW`/`SUBLW`/`RETURN` gaps are peephole/isel-level, not architectural

`w := k - f` lowers to a 3-instruction idiom (`MOVWF tmp` / `MOVLW k` /
`SUBWF tmp,W`) needing one scratch register instead of PIC14's single
`SUBLW k`; `w := w + k` lowers to the same idiom (`MOVWF tmp` /
`MOVLW k` / `ADDWF tmp,W`) instead of PIC14's single `ADDLW` (baseline
has neither literal-add op, gpasm 1.5.2 rejects `ADDLW` on p12f509,
confirmed 2026-09-10); every `RETLW` needs an explicit literal even for
a void-returning function (`RETLW 0` as the idiom, same as any core
without a bare `RETURN`). None needs a design decision beyond noting
them, unlike D-2.

---

## 3. Phases

With D-2 resolved (§2), this table can follow the same shape `docs/29`
and `docs/33` used, front-loaded on de-risking:

| # | Phase | Depends on |
|---|---|---|
| P0 | Device TOML (`p12f509`, `[[VERIFY]]`-cleared per D-3), firewall-only `Core::PicBaseline` | Nothing |
| P1 | `asm` encoder + `sim` core for the 33-instruction, 12-bit-word ISA, hand-written `.asm` only, including a P1 acceptance test that exercises D-2's `BCF`/`BSF FSR,5/6` sequencing directly (a direct access to bank 1 followed by an indirect access through a live pointer into bank 0, and the reverse) before any codegen depends on it | P0 |
| P2 | Integer spine + `isel-pic-baseline`'s unconditional `FSR` bank-bit reassertion (D-2) | P1 |
| P3 | Pointers/arrays, natural `FSR`-as-flat-address representation per D-2 item 2 | P2 |
| P4 | `const` in flash via `RETLW`, respecting the 256-word ceiling (D-5) | P2 |
| P5 | ~~Interrupts~~: does not exist on this core, phase dropped entirely | n/a |
| P6 | 32-bit `long`, mul/div runtime (third or fourth copy of the same routines, per the existing per-core-copy precedent) | P2 |
| P7 | Soft-float, if scoped in at all. **[VERIFY]**: with 25-41 bytes of RAM total, ask whether IEEE-754 single float has anywhere to live before committing to this phase; may be a documented non-goal instead | P6 |
| P8 | Fuzz gate, device-threaded differential runner, `PicBaseline` arm in `run_pic` | P1-P7 |

**gputils oracle coverage confirmed** (open question #3, formerly
unresolved): `gpasm-1.5.2` in the pinned dev image ships
`p12f509.inc`, `p12f508.inc`, and `p16f505.inc`
(`/usr/local/share/gputils/header/`, checked directly against the
image on 2026-09-09). All three device targets in this document have
an assembler oracle ready.

No SDCC-parity phase is proposed. SDCC has no baseline PIC support to
compare against at all; `docs/35-sdcc-parity-design.md`'s whole
premise (SDCC as a second oracle) does not apply here; XC8 remains the
only differential oracle if one is wanted, per the existing
black-box-only rule (ADR-006).

---

## 4. Open questions

Two of the original four are resolved as of this pass:

- ~~D-2~~: resolved in §2. `BCF`/`BSF` give bit-granular bank writes,
  a pointer's natural representation is `FSR`'s full value, and the
  remaining hazard is ordering, handled by the same live-bank-tracking
  shape the PIC14 `banking` pass already has. Not yet simulator-proven.
- ~~Family reference manual~~: searched again this pass (2026-09-09),
  nothing found beyond individual device datasheets and the memory
  programming spec. No standalone baseline-core FRM appears to exist;
  the per-part datasheet is the primary source, same as this document
  already treats it.
- ~~gputils coverage~~: confirmed against the pinned dev image
  (`gpasm-1.5.2`, 2026-09-09): `p12f508.inc`, `p12f509.inc`, and
  `p16f505.inc` are all present under
  `/usr/local/share/gputils/header/`. All three targets have an
  assembler oracle ready.

Still open:

1. Product question, not technical: is a target whose hardware call
   stack rejects most call graphs deeper than 2 worth shipping at all,
   absent a concrete consumer? (D-4 states the technical policy;
   this asks whether it's worth it.)
2. D-1's crate-sharing question (does `isel-pic-baseline` share more
   with `isel` or is it closer to clean-room), not resolved, needs a
   prototype or closer read, not blocking approval to start P0/P1.
3. ~~D-3's device-profile `[VERIFY]` items~~: resolved in P0
   (epic-cc#323). Config word: 0x7FF in the 509's config memory space per
   DS41227B section 2.3, HEX word 0xFFF per the MPASM/gputils convention
   (gpasm-verified); `common_ram` keeps its allocatable-window meaning
   (0x07-0x0F shared GPR), and the SFR mirror is core architecture in
   `bank_of`, not device data. Resolutions cited in the TOML and D-3.
4. P7's `[VERIFY]`: with 25-41 bytes of GPR total, does IEEE-754
   single-precision soft-float have anywhere to live at all? This is
   a real, unresolved feasibility question the phase table raises and
   does not answer; P7 may turn out to be a documented non-goal
   instead of a phase. Should be settled by P6, not deferred to P7
   itself.
5. `docs/33`'s revision history is the cautionary precedent for this
   whole document: two revisions were held on independent review
   before approval, for a document written by the same process this
   one just went through once. This document's own first independent
   review pass (2026-09-09) already caught one such analogy error in
   D-2 (corrected in place, see D-2); budget for at least one more
   pass before treating anything above as settled.
