# 03 — Architecture decision records

Each ADR records what was decided, why, what was rejected, and what would make us revisit.
Evidence for these lives in [`02-prior-art.md`](02-prior-art.md).

ADR-001 through ADR-008 live in this file. **Newer ADRs live one-per-file in
[`docs/adr/`](adr/)** (`ADR-00N-<topic>.md`, N continuing from 008). Each new ADR adds a
one-line index entry here, e.g.:

- ADR-009 — PIC18 pointer model: shared GEP fold, single FSR0, no PLUSWn, 2026-08-20
- ADR-010 — PIC18 const via TBLRD (DB-packed flash, per-byte TBLPTR re-setup), 2026-08-20
- ADR-011 — Multi-TU front end: llvm-link merge, sanitize in irparse, 2026-08-20
- ADR-012: CC-3 silicon-real codegen: EPIC_FOSC_HZ arithmetic, HEX regions, alloc placement, and simulator sizing, 2026-08-21
- ADR-013 — PIC18 interrupts: single-vector compat mode, MOVFF save area, 2026-08-20
- ADR-014 — PIC18 arithmetic routines: hardware MULWF schoolbook, branch-based divmod, 2026-08-20
---

## ADR-001 — clang as an out-of-process front end; custom PIC14 backend

**Status:** Accepted 2026-08-14 (user-approved)

### Decision

Use clang as a **separate process** that emits LLVM IR **as text** (`.ll`). Parse that
text into our own IR and implement a whole-program PIC14 backend ourselves.

We do **not** link libLLVM. We do **not** touch SelectionDAG, GlobalISel, TableGen, or
MCTargetDesc.

```
your .c files ──► clang -S -emit-llvm ──► .ll text ──► [our compiler] ──► .asm + .hex
                  (conformant C99 +                     parse IR
                   target-indep opts)                   whole-program call graph
                                                        overlay allocation
                                                        bank/page assignment
                                                        instruction selection
                                                        peephole
                                                        assemble + link
```

### What we get, and what we avoid

**Free:** a fully conformant C99 front end; mem2reg, SROA, inlining, GVN, DCE, and loop
passes.

**Avoided:** LLVM API churn (no rebuilding against LLVM 19→20→21), a multi-gigabyte build
dependency, a C++ plugin architecture, and — decisively — a permanent fork of LLVM's
target-independent core.

### Rationale

The naive argument "LLVM cannot target accumulator machines" is **false**, and should not
be repeated: llvm-mos disproves it. The correct argument is about cost:

- llvm-mos succeeded, at the price of a **22,421-line diff from upstream outside their own
  target directory**, including major surgery on Loop Strength Reduction.
- llvm-pic attempted **our exact target** (`PICMid`) with 3 people over ~18 months, with
  direct mentorship from the llvm-mos team, aiming at a language subset far below ours —
  and was archived in November 2025 without working `CALL`/`GOTO` and without having
  started on banking at all.

An unsupervised agent maintaining a perpetual rebase against a 30-million-line C++ tree is
close to a worst-case working environment. This weighs heavily given the autonomy
requirement in [`00-charter.md`](00-charter.md).

Text `.ll` in, text `.asm` out is also ideal for agent work: every stage boundary is a
diffable, snapshottable artifact.

### Rejected alternatives

**B — Fork SDCC's pic14 port.** Would inherit device headers, a library, a regression
suite, and users. Rejected: the port is unmaintained and fails its own regression tests;
it is architected around per-file compilation, which fights whole-program overlay
allocation; and understanding a large old C codebase costs roughly what writing fresh
costs.

**C — Write our own C front end (chibicc/lcc style), no LLVM at all.** Zero external
dependencies, total control, small enough to hold in context. Rejected *as a starting
point*: we would re-litigate integer promotions, bitfields, designated initializers,
constant expressions, and varargs indefinitely, and with no optimizer, code density would
lag badly. **Worth revisiting later** as a way to drop the clang dependency once the
backend is proven.

### Known risks

- clang bakes target ABI decisions into IR (`byval`, `sret`, varargs lowering, alloca
  patterns) that may fight a machine with no stack.
- LLVM IR assumes one flat address space; PIC is Harvard.
- The `.ll` surface we must parse could grow without bound.

**These risks are precisely what the feasibility spike is designed to test.** See
[`08-status-and-next-steps.md`](08-status-and-next-steps.md).

### Revisit if

The spike shows `.ll` text is an unworkable interface — in which case Approach C (own front
end) becomes the fallback, not the LLVM backend route.

---

## ADR-002 — Whole-program compilation; we own the toolchain down to HEX

**Status:** Accepted 2026-08-14 (user-approved)

### Decision

Compile all `.c` files in one invocation. Perform call-graph overlay allocation with full
program visibility. Emit `.asm` for human inspection **and** assemble/link to Intel HEX
ourselves. No external assembler or linker in the shipping product.

### Rationale

On PIC14 the **allocator is the compiler**. Since locals cannot live on a stack, every
local must be statically allocated and non-interfering frames overlaid using the whole call
graph. This requires whole-program visibility by construction. A traditional separate
compilation + relocatable linking model fights this hard — which is precisely the
architectural trap SDCC's pic14 port is in.

Owning the assembler is cheap: 35 instructions, fixed 14-bit encoding. Owning it removes an
external dependency and simplifies the correctness story, since allocation decisions and
encoding live in one place.

### Rejected alternatives

- **Emit `.asm`, hand off to gputils (gpasm/gplink).** gputils is actively maintained
  (v1.5.2, 2025-10-23 — an earlier assumption that it was abandoned was wrong). But
  relocatable linking makes whole-program overlay allocation awkward; we would end up
  bypassing `gplink` anyway. **We still use `gpasm` as a test-time cross-check oracle** —
  that is different from depending on it.
- **Emit `.asm`, hand off to Microchip `pic-as`.** Modern, correct, knows every device.
  Rejected: proprietary, non-redistributable, and it makes the project unusable for anyone
  without XC8 installed.

---

## ADR-003 — Steal llvm-mos's two techniques, not its implementation

**Status:** Accepted 2026-08-14

### Decision

Reimplement **static stack allocation** and **imaginary registers** in our own backend.

- *Static stack allocation:* whole-program call graph; non-recursive functions get
  statically allocated, overlaid frames. Recursion is a **compile error** on PIC14 (there is
  no addressable stack to fall back to, unlike the 6502). Interrupt handlers are analysed
  as a separate root; functions reachable from both the main and interrupt trees are
  identified and either duplicated or given non-overlapping storage.
- *Imaginary registers:* the 16-byte common RAM region at 0x70–0x7F, reachable from all
  banks without `BANKSEL`, serves as the register file.

### Rationale

These are the two techniques that made llvm-mos work, and both are a few hundred lines of
straightforward code when you own your IR. They are only expensive when expressed through
LLVM's target framework.

### Known risk

We have **16 bytes** where llvm-mos has 32, and a single `FSR`/`INDF` indirect pointer where
the 6502 has `(zp),Y` through any zero-page pair. Expect this to be the main code-quality
ceiling. Validating it is spike question 3.

---

## ADR-004 — Device support is data, not code

**Status:** Accepted 2026-08-14 (presented, pending final design approval)

### Decision

A `devices/pic16f877a.toml` describing memory map, bank layout, SFR names/addresses/bit
fields, config words, and hardware stack depth. Hand-authored for the 877A first; later
devices generated from gputils' `.inc` files.

### Rationale

Makes "I also want the 16F84A / 16F628A" a data change rather than a code change. gputils
being actively maintained makes its `.inc` files a durable upstream source.

---

## ADR-005 — Implement in Rust

**Status:** Accepted 2026-08-14 (user-approved)

### Decision

Rust.

### Rationale

Chosen for concrete properties that serve the autonomy requirement, not on general
preference:

- An unsupervised agent's strongest feedback signal is a compiler that rejects bad states
  at build time.
- `cargo test` needs no build-system babysitting.
- Exhaustive `match` over instruction and IR enums turns every unhandled case into a
  compile error rather than a silent miscompile — which matters enormously in a project
  whose failure mode *is* silent miscompilation.
- Snapshot testing (`insta`) fits a text-in/text-out pipeline exactly.

C++ would be the alternative if we ever wanted to link libLLVM — but ADR-001 says we never
do, which removes the main reason to choose it.

---

## ADR-006 — XC8 is a black-box oracle, never a reverse-engineering target

**Status:** Accepted 2026-08-14 (user-accepted after being raised as a concern)

### Decision

Never disassemble or reverse-engineer XC8 binaries. Use `xc8-cc` only by compiling source
and observing its output and behaviour.

### Rationale

The original project idea included disassembling XC8 binaries to reproduce their behaviour.
Two problems: the XC8 licence forbids reverse engineering, and it is the *slow* path
regardless. Black-box differential testing — compile the same C with both compilers, run
both on our simulator, compare observable state — is legally clean, faster, and yields a
permanent regression oracle, which is exactly what unsupervised agent work needs.

Everything genuinely load-bearing is public: datasheets, the ISA, the XC8 user guide's ABI
chapter, and SDCC/gputils source.

---

## ADR-007 — Nix flake + direnv for isolated, reproducible builds

**Status:** Accepted 2026-08-14 (user-approved)

### Decision

All build and test dependencies come from a Nix flake dev shell. Nothing is installed
system-wide. `direnv` activates it on `cd`. Full detail in
[`09-build-environment.md`](09-build-environment.md).

**clang is pinned to 20.1.8**, deliberately *not* tracking the nixpkgs default (currently
21).

### Rationale

The decisive argument is specific to this project: **we parse LLVM IR text, so the clang
version is part of our input format.** A silent clang bump can change what our parser sees.
Nix pins it exactly in `flake.lock`. `apt` and conda-forge both drift; a Docker tag drifts
unless pinned by digest with rebuild discipline. Of the options considered, only Nix closes
this properly.

Secondary: `nix develop --command <cmd>` is a one-line, daemon-free, scriptable entry
point, which serves the autonomy requirement in [`00-charter.md`](00-charter.md).

The host already had `nix` (2.34.8, flakes enabled) and `direnv` (2.37.1) installed, and
nixpkgs carries `gputils` at exactly 1.5.2 — the current upstream release — plus `cvise`,
`creduce`, and `csmith`.

### Rejected alternatives

All were already installed on the host, so availability was not the differentiator:

- **Docker / podman** — familiar, and trivially handles proprietary vendor blobs. Rejected
  as primary: reproducibility is only as good as base-image and `apt` pinning, which drift.
  The flake can still emit an OCI image via `dockerTools` if CI portability is ever needed,
  keeping one source of truth.
- **Pixi / conda-forge** — good lockfile story and much gentler than Nix. Rejected:
  `gputils` and `gpsim` are not on conda-forge, and conda is a poor fit for GTK-linked
  system tooling like gpsim.

### Consequences

- **XC8 must not become a flake input.** It is proprietary; making it a build dependency
  would break `nix develop` for anyone without a licensed install, including CI. It is
  detected at runtime via `$PIC8_XC8_ROOT` and its tests skip when absent.
- Two packages are missing from nixpkgs and need their own derivations eventually:
  **gpsim** and **YARPGen**. Both are deferred to the fuzzing phase; neither blocks the
  spike.
- New files must be `git add`ed before `nix develop` can see them.

### Revisit if

Nix evaluation becomes a recurring obstacle for unsupervised agent work, or a dependency
we need proves genuinely impractical to package.

---

## ADR-008 — Docker multi-stage toolchain replaces the Nix flake

**Status:** Accepted 2026-08-19 (user-approved); supersedes ADR-007

### Decision

Replace the Nix flake dev shell with a single docker multi-stage build:
`base` → `clang-builder` → `dev` / `ci` / `release`, all from a digest-pinned
`ubuntu:22.04` base. The Linux release clang is built from the digest-pinned
LLVM 20.1.8 source tarball with static LLVM libraries. Full detail in
[`09-build-environment.md`](09-build-environment.md) and
[`30-distribution-design.md`](30-distribution-design.md).

### Rationale

- **One tech stack** for local dev, CI, and release builds; the flake's
  rust-overlay indirection is replaced by an explicit `1.97.1` pin in
  `rust-toolchain.toml`.
- **The rpath problem was a Nix artifact.** nixpkgs builds LLVM with shared
  libraries for store-path dedup, baking `/nix/store/...` into every `.so`.
  A from-source cmake build defaults to static LLVM libraries, so the bundled
  clang links only platform runtimes and needs no `patchelf` surgery.
- **Official LLVM stopped shipping Linux x86_64 binaries after 18.1.8**, so
  source is the only pin-faithful option for the release clang.
- **Caching makes the cost one-time:** digest-pinned base + tarball, layer
  ordering (apt in `base`, clang in its own stage), buildx registry cache in
  CI, ccache cache mount locally. The clang layer is rebuilt only when a pin
  changes.
- **Minimum supported Linux is Ubuntu 22.04 (glibc ≥ 2.35)**, set by the
  base image — a version floor, not an installable dependency.

### Rejected alternatives

- **Nix-built clang + patchelf into the bundle** — two toolchains, store
  paths, relocation surgery.
- **Distro clang (apt)** — drifts from the pin; clang's version is part of
  our input format.
- **Fully static clang** — glibc is hostile to static linking (dlopen-based
  NSS/iconv), LLVM does not support it out of the box, and it buys nothing:
  glibc is the OS, not an install.

### Revisit if

The clang build cost becomes a bottleneck (mitigated by the cache), or a
second platform (macOS/ARM64) needs the same toolchain — the stage layout
generalizes.
