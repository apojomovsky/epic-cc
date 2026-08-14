# 09 — Build environment

**Isolated, reproducible builds via a Nix flake + direnv. Nothing is installed
system-wide.** Decision recorded as [ADR-007](03-decisions.md).

## Getting in

```bash
cd /home/alexis/projects/pic8_compiler
direnv allow      # one time; afterwards the shell activates on `cd`
```

Or without direnv:

```bash
nix develop                      # interactive shell
nix develop --command cargo test # one-shot, scriptable — prefer this in automation
```

> **Flakes only see git-tracked files.** A new file that has never been `git add`ed is
> invisible to `nix develop`, which fails with *"Path 'flake.nix' … is not tracked by
> Git."* Stage new files before building.

## What is pinned

| Tool | Version | Role |
|---|---|---|
| **clang** | **20.1.8** (`llvmPackages_20`) | IR producer — **deliberately pinned, see below** |
| rustc / cargo | 1.97.1 | via `rust-toolchain.toml` + oxalica `rust-overlay` |
| gputils (`gpasm`) | 1.5.2 | assembler cross-check oracle |
| cvise / creduce / csmith | 2.12.0 / 2.10.0 / 2.3.0 | test-case reduction and fuzzing |
| poppler-utils | 26.06.0 | `pdftotext` / `pdfinfo` for the vendored books |

### Why clang is pinned, and not to the nixpkgs default

We parse **LLVM IR text**, so the clang version is part of our input format. nixpkgs'
default is currently clang 21; we deliberately pin 20 so a nixpkgs bump cannot silently
change what our parser sees. **Bumping this is a migration with a test pass, not
housekeeping.**

## Environment variables the shell exports

| Variable | Meaning |
|---|---|
| `PIC8_CLANG` | Wrapped clang. Convenient, but see the cross-targeting warning below |
| `PIC8_CLANG_UNWRAPPED` | Unwrapped clang. **This is the one to drive.** |
| `PIC8_CLANG_RESOURCE_DIR` | Builtin headers (`stddef.h`, `stdint.h`) |
| `PIC8_GPASM` | gpasm binary |
| `PIC8_VENDOR_DIR` | `vendor/` root |
| `PIC8_XC8_ROOT` | XC8 install, default `/opt/microchip/xc8/v4.00`. Override to relocate |

## ⚠️ Three environment gotchas, all verified the hard way

### 1. Use the UNWRAPPED clang for cross-targeting

Nix's `cc-wrapper` injects host hardening flags. Driving the **wrapped** clang at a
non-host target fails outright:

```
Warning: supplying the --target msp430 != x86_64-unknown-linux-gnu argument to a
nix-wrapped compiler may not work correctly - cc-wrapper is currently not designed
with multi-target compilers in mind. You may want to use an un-wrapped compiler instead.
clang: error: unsupported option '-fzero-call-used-regs=used-gpr' for target 'msp430'
```

**Always use `$PIC8_CLANG_UNWRAPPED`** with an explicit `-resource-dir`. The known-good
invocation:

```bash
"$PIC8_CLANG_UNWRAPPED" -target msp430 -Oz -S -emit-llvm -ffreestanding -nostdinc \
    -resource-dir "$PIC8_CLANG_RESOURCE_DIR" -o out.ll in.c
```

### 2. `clang -print-resource-dir` lies under nixpkgs

nixpkgs splits clang's builtin headers into the `.lib` output. `-print-resource-dir`
reports a path that **does not exist**. Trust `$PIC8_CLANG_RESOURCE_DIR`, which the flake
computes as `clang-unwrapped.lib/lib/clang/<MAJOR>` — note **major version only** (`20`,
not `20.1.8`).

### 3. `poppler_utils` was renamed to `poppler-utils`

Trivial, but it is an eval error that blocks the whole shell.

## Verified working end-to-end

Confirmed on 2026-08-14:

- **clang emits usable IR at a PIC-appropriate datalayout.**
  `-target msp430` yields
  `target datalayout = "e-m:e-p:16:16-i32:16-i64:16-f32:16-f64:16-a:8-n8:16-S16"` —
  16-bit pointers, byte alignment, native 8/16-bit. This validates the datalayout-proxy
  plan in [`04-pipeline-design.md`](04-pipeline-design.md).
- **gpasm assembles for the 877A and emits valid Intel HEX.**
  `gpasm -p p16f877a t.asm -o t.hex` works. It also emits
  *"RAM Bank undefined in this chunk of code"* — a preview of the banking problem.
- **XC8 is auto-detected** at `/opt/microchip/xc8/v4.00` and reported by the shell hook.

### 🔎 An unplanned but important finding

Environment verification produced a real signal about the front end. Compiling this at
`-Oz`:

```c
int add(int a, int b) { return a + b; }
int sum(int n) { int t = 0; for (int i = 0; i < n; i++) t = add(t, i); return t; }
```

clang collapsed the loop to closed form and emitted:

```llvm
%3 = zext nneg i16 %2 to i17
%6 = mul i17 %3, %5
%7 = lshr i17 %6, 1
%2 = tail call i16 @llvm.smax.i16(i16 %0, i16 0)
```

Two things we must plan for:

1. **Arbitrary-precision integer types.** `i17` is not a typo — LLVM IR permits any bit
   width, and the optimizer *will* produce them. Our legalizer cannot assume 8/16/32; it
   needs a general widening/narrowing story. A `mul i17` on a core with **no hardware
   multiply** is a particularly unpleasant lowering.
2. **Intrinsics.** `@llvm.smax.i16` must be lowered. There will be many more.

This is direct evidence for the open question about which clang passes to enable —
aggressive optimization trades loop structure for IR constructs that are expensive for us.
It does **not** settle the question; the feasibility spike should measure it properly.

## Known gaps

| Gap | Status |
|---|---|
| **gpsim** | Not in nixpkgs. Needs its own derivation (autotools + GTK). **Deferred** — we still have three oracles (our simulator, gpasm, XC8). Belongs to the fuzzing phase |
| **YARPGen** | Not in nixpkgs. Needs a derivation. Deferred to the fuzzing phase; `csmith` is packaged as an interim option |

## XC8 is not a build dependency

XC8 is a **test oracle only** ([`05-verification.md`](05-verification.md)). It is
proprietary and cannot be a flake input without breaking `nix develop` for anyone without a
licensed install, including CI.

It is therefore detected at runtime via `$PIC8_XC8_ROOT`. **Differential tests must skip
with a clear message when it is absent**, and the build plus the core test suite must pass
without it. The same rule applies to anything else under `vendor/`.

## Vendored material

See [`../vendor/README.md`](../vendor/README.md) for the layout and what to put where.
Contents are gitignored; only the README is tracked.

## If you ever need containers

The flake can generate an OCI image via `pkgs.dockerTools` from the same pinned inputs, so
Docker/podman remain available for CI portability without a second source of truth. Not
built — add only if a real need appears.
