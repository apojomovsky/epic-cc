# 44: Code-factoring spike findings (epic-cc#660)

Spike, not a production commitment: price code factoring (procedural
abstraction) on `hal-pic18-menu-demo-18f4550` without implementing the
backend pass, and turn the result into a verdict on the follow-up.
The technique is public: the XC8 user guide documents it as part of
`-Os`, and the literature has it since Fraser, Myers and Wendt (1984)
and Debray et al. (2000). Everything below comes from our own listing
(ADR-006).

Baseline: the `--emit asm` listing assembles to 10992 flash words
through `assemble_pic18`, matching the driver's own total.

## Method

Two transformations, priced together by one greedy selector over
repeated instruction sequences in the listing:

- **Outline:** a repeated straight-line sequence becomes one shared
  body ending in `RETURN`; every site becomes `CALL` (2 words) or
  `RCALL` (1 word, reach of 1024 words either way).
- **Tail merge:** a repeated sequence ending in `RETURN` keeps one copy;
  the other sites branch to it (`BRA` or `GOTO`). No stack cost.

A sequence is outlinable when it has no label inside, no branch, skip,
call, return or data, touches none of PCL, PCLATH, PCLATU, STKPTR or
TOS (including their access-bank aliases), and does not start right
after a skip. Matching is textual, which is exact here: allocation is
static, so identical text means identical effect. `CALL` and `RETURN`
preserve W, STATUS and BSR, so a body runs under its caller's state
exactly like the inline copy did.

A pick is priced `k*W - (k*c + W + 1)` for `k` sites of `W` words and
call cost `c`. The selector takes the best pick, marks its sites, and
repeats until nothing gains.

## Results

| variant | flash words | saved |
|---|---|---|
| baseline | 10992 | |
| every site `CALL`, bodies at the end | 9799 | 1193 (10.9%) |
| `RCALL`/`BRA` for picks local to a function up to 800 words, body placed after that function | 9505 | 1487 (13.5%) |
| every site 1 word (bound, does not assemble) | | about 1700 |

Both rows with numbers are exact: the rewritten listing assembled
through `assemble_pic18`. The 1-word bound fails assembly because
bodies at the end of the program are out of `RCALL` reach.

Where the saving comes from (the `CALL` row):

- 80% repeats inside a single function: inlined bodies and unrolled
  address arithmetic, which is also why local `RCALL` placement pays.
- By shape: FSR pointer arithmetic and indirect access 515 words,
  const-table (TBLPTR) access 154, `MOVFF` block copies 152, bitmask
  read-modify-write 69, other 269.
- Bodies of 9 or more instructions carry half the saving; 2 to 3
  instruction bodies still carry 170 words.

## Behaviour check

A layout-preserving variant pads each replaced site with `NOP`s to its
original size and puts the shared bodies after the code, so every
existing address, table and stored pointer stays put. Our PIC18
simulator ran it next to the original, comparing the ordered stream of
RAM writes (excluding PCL and the stack SFRs):

- no interrupts: both images reach their halt with the same 76005
  writes, no mismatch;
- interrupts requested every 500, 97 and 13 writes: no mismatch over
  540K to 1.2M writes across 3M instructions;
- 63 of the 87 outlined bodies executed, carrying 831 of their 1146
  words.

The unpadded images cannot be compared write for write: moving code
moves the constant tables, so TBLPTR values and const pointers stored
in RAM differ by design. They differ from the padded image only in the
call opcode and body placement: `RCALL` reach is checked by the
assembler, and every body placed between functions sits after an
unconditional `RETURN`, `BRA` or `GOTO`, checked on the listing.

## Costs

- **Stack:** bodies are leaves, so a site adds at most one return
  address. The deepest main-line chain is 7 and the ISR chain 3; the
  worst case stays far below the 31-entry PIC18 stack. PIC14's 8-level
  stack is a different story and is not priced here.
- **Cycles:** the full harness run executes 6.0% (`CALL` variant) to
  7.2% (local `RCALL`) more instructions, each added one a 2-cycle call
  or return.
- **Interrupt latency:** about 127 of the 1487 words sit in code
  reachable from the interrupt handler. Excluding that tree keeps
  latency unchanged for about 1360 words.
- **Debug info:** a shared body serves several source lines, so the
  address-to-line table (ADR-028) and the size map (ADR-025) need a
  rule for it.

## Verdict: green

About 1360 to 1490 words on the menu demo is the largest single win
measured so far, larger than bank tracking in docs/42, and it needs no
ABI or allocator change. Recommended follow-up:

- **A final-listing pass** between codegen and `asm`: listing in,
  listing out, so it keeps the diffable text boundary and applies to
  every PIC18 front half unchanged.
- Local placement with `RCALL`/`BRA` first, `CALL` only for picks that
  cross functions.
- Leave the interrupt tree alone by default; the call-depth check
  counts the added leaf level.
- An e2e gate built on this spike's padded differential: the same
  program with and without the pass, compared write for write in the
  simulator, with interrupts injected.
- Ordering with isel work: shrinking the repeated shapes at the source
  (the stride multiply in #625, the 16-bit FSR adds) lowers this pass's
  yield but never its correctness, so neither blocks the other.

## Residuals

- The listing analyzer and differential harness were scratch and did
  not ship. Reproduction: re-emit the listing with `make size-report`
  (it writes `scratch/size-report/menu-demo.asm`), then reimplement the
  scan from the rules in Method.
- The greedy selector is not optimal: overlapping candidates are
  resolved by gain order only. A better selector can only raise the
  numbers.
- The mdb UART gate was not run: no driver build carries the pass yet.
- PIC14 and PIC14E are unmeasured: their 8-level stack and `PCLATH`
  paging change both the pricing and the depth budget.
