# 30 — Public binary distribution design

**Status:** Design approved 2026-08-19 (user-approved)

## Problem

`epic-cc` is under heavy development with no releases. We want public binary releases for
**Linux x86_64** and **Windows x86_64**. Open questions, resolved by this design:

1. Do released binaries have an external dependency on clang?
2. Can we embed clang in the release? Is that legal, and how?
3. What is the pragmatic build/distribution machinery, given the ultimate goal of
   first-class PlatformIO integration alongside `epic-hal`?

This document is the design; the implementation plan follows it.

## Decisions

| # | Decision |
|---|---|
| D1 | **Bundle clang in every release.** The bundled clang *is* the product's front end. clang's version is part of our input format (`docs/09-build-environment.md`), so a user's system clang would be a correctness risk. Bundling preserves the pin and gives a "one zip, compile" story. |
| D2 | **Single docker stack replaces the Nix flake.** One tech stack for local dev, CI, and release builds, using multi-stage images from a common base. `flake.nix` is retired after the migration. |
| D3 | **Linux clang is built from the LLVM 20.1.8 source tarball** (digest-pinned) inside the docker build. Rationale: official LLVM stopped shipping Linux x86_64 binaries after 18.1.8; distro packages drift from the pin; nix-extraction is moot once the flake is retired. |
| D4 | **Minimum supported Linux: Ubuntu 22.04 (glibc ≥ 2.35).** Everything is built on `ubuntu:22.04` (digest-pinned), so released binaries require glibc ≥ 2.35 — i.e. U22, Debian 12, or newer. This is a version *floor*, not an installable dependency. |
| D5 | **gputils 1.5.2 is built from source in the base image.** It is a test oracle, never a runtime dependency and never shipped. apt (jammy) has 1.4.0; the 14 byte-for-byte gpasm cross-checks are version-sensitive, so drift is not acceptable. |
| D6 | **Driver gains a clang discovery chain and ships as `epic-cc`.** Env overrides first (unchanged dev/CI path), then bundled `clang/` next to the executable, then a clean diagnostic — never a panic. |
| D7 | **Windows bundle slices the official `clang+llvm-20.1.8-x86_64-pc-windows-msvc.tar.xz`** (LLVM release asset, redistributable under Apache-2.0-with-LLVM-exception). No build needed; just `clang.exe` + DLLs + `lib/clang/20/`. |
| D8 | **PlatformIO integration is phase 2.** Standalone zips ship first; the same artifacts later become PIO toolchain packages, and `epic-hal` gains the platform glue. Nothing in this design is PIO-specific, so no rework. |

## Runtime dependency story

| Shipped artifact | Runtime dependencies | Notes |
|---|---|---|
| `epic-cc` (Rust, glibc target) | glibc ≥ 2.35 | Floor set by building on U22 |
| bundled `clang` | glibc ≥ 2.35, libstdc++ (gcc-11 era) | LLVM built with `BUILD_SHARED_LIBS=OFF` (the Linux default) → **no `libLLVM.so` exists**, nothing to bundle or rpath-patch |
| `.hex` output | **none** | ROM image for the PIC |

The one-line policy for release notes: *requires Ubuntu 22.04 / Debian 12 or newer on x86_64.*

Fully static clang is not attempted: glibc is hostile to static linking (dlopen-based NSS/iconv),
LLVM does not support it out of the box, and it buys nothing — glibc is the OS, not an install.

## Why docker removes the rpath problem

The rpath problem was a **Nix artifact**: nixpkgs builds LLVM with shared libs for store-path
dedup, baking `/nix/store/...` into every `.so`. A from-source cmake build on Linux defaults to
**static LLVM libraries**, so the produced `clang` links only platform runtimes and needs no
relocation surgery. No `patchelf`, no store paths, nothing to stage.

## Docker architecture

```
Dockerfile (multi-stage, buildx, digest-pinned ubuntu:22.04)
├── base          → build tools, cmake 3.22, ninja, zlib, ccache,
│                    gputils 1.5.2 (source), csmith 2.3.0 + creduce 2.10.0 (apt)
├── clang-builder → ADD llvmorg-20.1.8 source tarball (digest-pinned)
│                    → cmake -DCMAKE_BUILD_TYPE=Release -DLLVM_ENABLE_PROJECTS=clang
│                    → static-lib clang + lib/clang/20 builtin headers
├── dev           → clang-builder + rustup (honors rust-toolchain.toml → 1.97.1)
│                    → interactive shell, replaces `make shell  # docker`
├── ci            → dev + scripts/ci-test.sh (what CI runs; script unchanged)
└── release       → clang-builder + cargo → assembles the distribution bundle
```

### Caching (the requirement that makes this viable)

The clang build is the only expensive layer (~1–2 h first build, on the order of 20 GB).
It is also the most stable: the tarball is digest-pinned and we do not plan to bump clang.
Caching strategy, in order of stickiness:

1. **Layer ordering.** apt packages live in `base`; the clang build lives in its own stage
   (`clang-builder`) fed by a digest-pinned `ADD`. An apt-level change can never invalidate
   the clang layer.
2. **Registry cache.** CI builds with
   `--cache-to type=registry,ref=ghcr.io/apojomovsky/epic-cc-clang` /
   `--cache-from ...` (or pushes the clang-builder image to GHCR directly). Local dev reuses
   the local layer cache; CI reuses the registry cache. Clang builds effectively **once
   ever per tarball bump**.
3. **ccache cache mount** (`RUN --mount=type=cache,target=... ninja`) as a second line of
   defense — even a forced rebuild is incremental, not from scratch. Same technique for
   cargo's registry dir.

### Dev tool pins vs apt (verified against jammy)

| Tool | Previous flake pin | apt (jammy) | Action |
|---|---|---|---|
| gputils | 1.5.2 | 1.4.0 | **build from source** (oracle pin must hold) |
| csmith | 2.3.0 | 2.3.0 | apt |
| creduce | 2.10.0 | 2.10.0 | apt |
| cvise | 2.12.0 | 2.4.0 | apt (dev convenience only; never gates tests) |

## Bundle layout (the contract the driver discovers)

```
epic-cc-<ver>-x86_64-linux/          epic-cc-<ver>-x86_64-windows/
├── epic-cc                        ├── epic-cc.exe
├── clang/                         ├── clang/
│   ├── bin/clang                  │   ├── bin/clang.exe
│   └── lib/clang/20/              │   └── lib/clang/20/   ← builtin headers
└── LICENSE (LLVM Apache-2.0)      └── LICENSE (LLVM Apache-2.0)
```

License note: LLVM is Apache-2.0-with-LLVM-exception, explicitly redistributable; the
bundle ships its LICENSE alongside, as every clang vendor does.

## Driver rework (the only pipeline code change)

Replace the `expect()`s on `PIC8_CLANG_UNWRAPPED` / `PIC8_CLANG_RESOURCE_DIR`
(`crates/driver/src/main.rs`) with a resolution chain:

1. `PIC8_CLANG_UNWRAPPED` / `PIC8_CLANG_RESOURCE_DIR` env vars (dev/CI path — unchanged,
   all existing tests keep working untouched);
2. bundled fallback: `<exe_dir>/clang/bin/clang` + `<exe_dir>/clang/lib/clang/<ver>/`
   (same `-resource-dir` semantics);
3. neither → clean diagnostic: "no clang front end found; set PIC8_CLANG_UNWRAPPED or ship
   the `clang/` directory next to the executable" — never a panic.

Rename the shipped binary from `driver` to `epic-cc` via a `[[bin]]` entry in the driver
crate's `Cargo.toml` (`cargo run -p driver` keeps working). No pipeline-stage changes.

## Release pipeline

New `release.yml`, tag-triggered:

- **Linux job:** `docker buildx build --target release` → bundle zip. The clang layer comes
  from the registry cache (see above).
- **Windows job:** `windows-latest`, build the Rust binary natively; download the official
  LLVM 20.1.8 MSVC bundle, slice `clang.exe` + DLLs + `lib/clang/20/`, assemble the zip.
- **Per-platform smoke:** run the *actual shipped binary* from the zip against a fixture
  (e.g. `add.c`) and byte-diff its HEX against the dev-shell build of the same fixture.
  The compiler is deterministic, so this proves the bundle's clang loads and the driver
  finds it — end to end.
- Attach both zips + sha256 sums + LLVM license to the GitHub release.

Existing CI (`.github/workflows/ci.yml`) continues to gate every commit with the full test
suite, now running inside the `ci` image.

## Testing

- The discovery chain is exercised by the release smoke test (bundled path) and the
  existing suite (env path). A unit test for the discovery function with a fake layout is
  optional.
- The 14 gpasm byte-for-byte cross-checks and the full workspace suite run unmodified
  inside the `ci` image — the container migration must pass the suite as-is before the
  flake is retired.

## Out of scope / deferred

- **PlatformIO packaging and `epic-hal` glue** (phase 2; consumes the same zips).
- Fully static clang binaries (rejected above).
- macOS/ARM64 releases (not requested; the design's docker stages generalize later if needed).
- Updating `docs/09-build-environment.md`'s Nix content and adding the ADR superseding
  ADR-007 — implementation-time follow-ups, tracked in the plan.

## Migration checklist (implementation)

- [ ] Dockerfile: base / clang-builder / dev / ci / release stages
- [ ] Spike: `-DLLVM_TARGETS_TO_BUILD=MSP430` sufficient for `-emit-llvm` (else default targets)
- [ ] Verify gputils 1.5.2 source build + the 14 cross-checks pass in the `ci` image
- [ ] Driver: discovery chain + `epic-cc` `[[bin]]`
- [ ] `release.yml` with cache wiring + per-platform smoke
- [ ] Retire `flake.nix` / `rust-toolchain.toml`-driven nix path after CI is green on docker
- [ ] Docs: `09-build-environment.md` rewrite; ADR superseding ADR-007; README release section
