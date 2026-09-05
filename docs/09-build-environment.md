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

## Environment variables the device data gate reads

`crates/device/tests/gputils_crosscheck.rs` verifies every device TOML against
gputils (ADR-021). It is the only oracle the device registry has, so running
without it takes two variables, not one.

| Variable | Meaning |
|---|---|
| `PIC8_GPUTILS_SHARE` | gputils data root, default `/usr/local/share/gputils` |
| `PIC8_ALLOW_NO_GPUTILS` | acknowledges the data root is absent. On its own the gate still **fails** |
| `PIC8_UNVERIFIED_DEVICE_DATA` | set to `i-accept-unverified-device-data`, and only with the variable above, to run with no device verification at all |

Both are read only when the data root is genuinely missing, so setting them
cannot switch off a gate that could have run. With both set the tests print a
`DEVICE DATA UNVERIFIED` banner and verify nothing; `scripts/ci-test.sh` also
records the disabled gate in the CI step summary.

## Caching

The clang build is the only expensive layer (~1 h first build). It is cached
via the buildx registry cache (`ghcr.io/<repo>-toolchain`) in CI, and via the
local layer cache + a ccache cache mount locally. Because the base image and
the LLVM tarball are digest-pinned, the clang layer is rebuilt only when the
Dockerfile or a pin changes.

## Cargo target cache is per worktree

`CARGO_TARGET_DIR` points at `~/.cache/epic-cc/target-<worktree path with /
replaced by ->`, so each worktree builds into its own cache. This is
load-bearing: every docker invocation mounts its worktree at the identical
in-container path (`/workspace`), and cargo fingerprints compilation units
partly by absolute path, so a shared target dir lets cargo silently replay a
different worktree's artifacts. `check-warnings` uses its own
`target-warncheck-<same key>` dir for the same reason.

Caches are never pruned automatically, and removing a worktree does not
remove its cache (the caches live outside the repo). To reclaim the space of
a removed worktree, delete the matching dirs under `~/.cache/epic-cc/`, whose
names carry the worktree path with slashes replaced by dashes. Correctness
first, disk second: a cold per-worktree build is the price of never replaying
a sibling branch's bytes.

## XC8 is not a build dependency

XC8 is a **test oracle only** ([`05-verification.md`](05-verification.md)). It
is proprietary and cannot be part of the image. It is detected at runtime via
`$PIC8_XC8_ROOT`; differential tests skip with a clear message when absent.

## Device TOML authoring (DFP -> TOML)

Per-device memory maps and config words live in `crates/device/devices/*.toml`
and are generated from Microchip DFPs, not hand-transcribed at device #3+:

```bash
python3 scripts/gen-device.py p16f887 --out crates/device/devices/p16f887.toml
python3 scripts/gen-device.py p16f887 --check   # CI gate: fails if TOML drifts
```

Source posture per `AGENTS.md` GPL boundary:

* **Primary:** the DFP's `xc8/pic/dat/{ini,cfgdata}` and `edc/*.PIC` XML
  (inside the `.atpack` zip). The pack itself is downloaded from
  `https://packs.download.microchip.com/` (`Microchip.PIC16Fxxx_DFP` for
  mid-range) and is never committed. The `.atpack`/`.atdf`/`.PIC` stays in
  `vendor/microchip/device-data/` (gitignored) or under `$PIC8_XC8_ROOT`; only
  the generated TOML is committed.
* **Oracle:** `gputils` headers (`share/gputils/header/p16f887.inc`) are the
  byte-for-byte oracle; XC8 headers are black-box oracle only.

The generator (`scripts/gen-device.py`, stdlib only) normalises field/value
names via a small alias table (`FOSC` -> `osc`, `WDTE` -> `wdt`, `INTRC` ->
`intosc`, etc.) documented in its header, and emits deterministically
formatted TOML (fields sorted by `byte_offset`/`shift`, values by `bits`).
The provenance `pack` name is derived from the source file's nearest
`*_DFP` ancestor directory; a file extracted outside its pack directory
needs `--pack <name>` passed explicitly, otherwise the generator refuses to
write the stanza rather than fabricate `pack = "unknown"` (ADR-021).
See `docs/adr/ADR-020-dfp-toml-generator.md` and `scripts/gen-device.py --help`.

`crates/device/provenance.rs` reaches the crate only via `include!`, never a
`mod` declaration, so `cargo fmt` (and `make fmt`) never visits it. The
pre-commit hook still catches drift, since it runs `rustfmt --check` on staged
files directly rather than through `cargo fmt`. Run `rustfmt` on that file by
hand after editing it.

## Release bundles

Tag-triggered [`release.yml`](../.github/workflows/release.yml) builds the
Linux bundle in the `release` stage and slices the official LLVM MSVC bundle
for Windows, smoke-tests each shipped binary, and attaches both zips to the
GitHub release. Layout and the driver's discovery contract are in
[`30-distribution-design.md`](30-distribution-design.md).
