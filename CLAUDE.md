# Working notes for agents on pic8_compiler

## Orientation

Read [`docs/08-status-and-next-steps.md`](docs/08-status-and-next-steps.md) before doing
anything else. It states what is done, what is next, and what is still unapproved.

The full context of the design conversation is captured across `docs/00-` … `docs/08-`.
It is written to be sufficient on its own — you should not need the original conversation.

## Ground rules established with the user

- **Approval gates are real.** The user works with an explicit brainstorm → design →
  approve → implement flow. Present a design and *stop* until you get a yes. This applies
  even to work that looks small.
- **Do not reverse-engineer or disassemble the XC8 binaries.** Its licence forbids it, and
  it is the slow path regardless. Use XC8 as a *black-box oracle*: compile the same source
  with `xc8-cc` and diff observable behaviour. See [`docs/05-verification.md`](docs/05-verification.md).
- **Do not copy the reference PDFs into this repo.** They are copyrighted. They live in
  `~/Downloads/`; see [`docs/06-environment.md`](docs/06-environment.md) for paths and
  reading instructions.
- **GPL boundary.** `gputils` and `gpsim` are GPL. Invoking them as external processes in
  a test harness is fine. Linking them into our compiler is not.

## Things that will waste your time if you do not know them

- **Muchnick's PDF text layer is fullwidth Unicode.** `pdftotext | grep 'Chapter'` returns
  nothing until you NFKC-normalize. Recipe in [`docs/06-environment.md`](docs/06-environment.md).
- **XC8's bundled `clang` is not a general-purpose clang.** It emits p-code for the
  `cgpic` backend, not usable LLVM IR. Install a stock clang.
- **XC8 does not use LLVM for mid-range PIC.** The clang binary is a front end only; the
  actual code generator for PIC14 is `cgpic`, the HI-TECH C lineage backend. Do not assume
  "XC8 is clang-based" means its codegen is LLVM.

## Implementation language

Rust. Rationale in [`docs/03-decisions.md`](docs/03-decisions.md) (ADR-005).
