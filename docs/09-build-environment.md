# 09 — Build environment

**Isolated, reproducible builds via a docker multi-stage toolchain. Nothing is
installed system-wide.** Decision recorded as [ADR-008](03-decisions.md)
(supersedes ADR-007).

## Getting in

Build the dev image (first build is slow — it compiles clang; see Caching):

```bash
docker build --target dev -t epic-cc-dev .
```

Then either an interactive shell:

```bash
docker run --rm -it -v "$PWD:/workspace" -w /workspace epic-cc-dev bash
```

or one-shot for automation:

```bash
docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-dev bash scripts/ci-test.sh
```

## What is pinned

| Tool | Version | Role |
|---|---|---|
| **clang** | **20.1.8** (source tarball, sha256-pinned) | IR producer — **deliberately pinned, see below** |
| rustc / cargo | 1.97.1 | via `rust-toolchain.toml` + rustup |
| gputils (`gpasm`) | 1.5.2 (source, sha256-pinned) | assembler cross-check oracle |
| csmith / creduce | 2.3.0 / 2.10.0 (apt) | fuzzing and reduction |
| cvise | 2.4.0 (apt) | reduction convenience (never gates tests) |

### Why clang is pinned, and built from source

We parse **LLVM IR text**, so the clang version is part of our input format.
Official LLVM stopped shipping Linux x86_64 binaries after 18.1.8, so the
release clang is built from the digest-pinned 20.1.8 source tarball in the
`clang-builder` stage. **Bumping this is a migration with a test pass, not
housekeeping.**

The build uses static LLVM libraries (the Linux default), so the produced
`clang` links only platform runtimes — no `libLLVM.so`, no rpath patching.

## Environment variables the images export

| Variable | Meaning |
|---|---|
| `PIC8_CLANG_UNWRAPPED` | `/opt/clang/bin/clang` (unwrapped — no wrapper-injected host flags) |
| `PIC8_CLANG_RESOURCE_DIR` | `/opt/clang/lib/clang/20` (builtin headers; **major version only**) |
| `PIC8_GPASM` | `/usr/local/bin/gpasm` |
| `PIC8_VENDOR_DIR` | `/workspace/vendor` |
| `PIC8_XC8_ROOT` | `/opt/microchip/xc8/v4.00` (override to relocate) |

The driver resolves clang from these env vars, or from the bundled `clang/`
directory next to the executable in release bundles, or fails with a clean
diagnostic (see `crates/driver/src/clang_discovery.rs`).

## Caching

The clang build is the only expensive layer (~1 h first build). It is cached
via the buildx registry cache (`ghcr.io/<repo>-toolchain`) in CI, and via the
local layer cache + a ccache cache mount locally. Because the base image and
the LLVM tarball are digest-pinned, the clang layer is rebuilt only when the
Dockerfile or a pin changes.

## XC8 is not a build dependency

XC8 is a **test oracle only** ([`05-verification.md`](05-verification.md)). It
is proprietary and cannot be part of the image. It is detected at runtime via
`$PIC8_XC8_ROOT`; differential tests skip with a clear message when absent.

## CI

GitHub Actions runs the full workspace test suite on every push/PR
([`.github/workflows/ci.yml`](../.github/workflows/ci.yml)): one job, `docker
run … epic-cc-ci:latest bash scripts/ci-test.sh`, so CI uses exactly the
Dockerfile's toolchain — the Dockerfile is the single source of truth,
nothing is installed on the runner. The loop itself lives in
[`scripts/ci-test.sh`](../scripts/ci-test.sh) (per-crate `cargo test` with a
PASS/FAIL summary table), the same script you can run locally to reproduce a
CI result exactly.

## Release bundles

Tag-triggered [`release.yml`](../.github/workflows/release.yml) builds the
Linux bundle in the `release` stage and slices the official LLVM MSVC bundle
for Windows, smoke-tests each shipped binary, and attaches both zips to the
GitHub release. Layout and the driver's discovery contract are in
[`30-distribution-design.md`](30-distribution-design.md).
