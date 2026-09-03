# ADR-023 -- Rolling master release bundles

**Status:** Accepted 2026-08-25<br>
**Decides:** `epic-cc#118`<br>
**Parent:** `docs/30-distribution-design.md` (D1-D6), `docs/superpowers/specs/2026-08-25-rolling-release-design.md`

## Decision

* Every push to master runs `.github/workflows/rolling-release.yml`: it builds
  the existing Linux bundle with `EPIC_CC_VERSION=0.0.0-master-<sha>`, proves the zip
  on a bare runner (the binary reports the commit, and it compiles a fresh C
  file for `p16f887` using only its bundled clang and llvm-link), then creates
  a prerelease tagged `ci-<sha>` carrying the zip, its sha256 sums, and
  real-newline notes. `workflow_dispatch` runs build+gate as a dry run without
  publishing.
* The release bundle ships `llvm-link` and `opt` next to `clang` in
  `clang/bin/`, because the driver resolves both from the same directory and
  always runs them (llvm-link: multi-unit merge, single-unit no-op; opt: the
  whole-program cleanup over the merged module).
* The driver gains a version stamp: a `build.rs` reads the `EPIC_CC_VERSION`
  build ARG (an environment variable of the docker release stage's cargo RUN)
  and exports `EPIC_CC_STAMP`; `epic-cc --version` (and `-V`) prints
  `epic-cc <stamp>`, falling back to `CARGO_PKG_VERSION` when the ARG is unset.
* Tag releases (`release.yml`) pass the same ARG, so one stamping path covers
  both release kinds. Windows stays tag-only: rolling bundles serve
  `epic-hal#80` (Linux) and a Windows rolling job would rebuild per push for no
  consumer.

## Rationale

* The issue's dogfooding argument: the rolling artifact is byte-identical to
  what HAL-5 ships to users, so a downstream job consuming it tests the
  distribution path itself, not a private code path.
* Per-commit prerelease tags make the pin a pin: `ci-<sha>` always resolves to
  the same compiler, run history is replayable, and a downstream failure names
  the exact commit. A single moving `master` release would give a constant URL
  with a changing artifact, which no job can pin.
* The gate is the consumer: it runs the downloaded zip with no clang
  environment, so it proves exactly what `epic-hal#80` will do. Publish depends
  on gate, so a misstamped bundle cannot be published.
* The `llvm-link` fix is not optional: without it the shipped bundle fails its
  own "bundled discovery" smoke the moment a tag fires (the driver needs
  llvm-link even for a single translation unit).

## Alternatives rejected

* **Single moving `master` prerelease.** The download URL stays constant while
  the artifact changes; a pinned job is unreproducible and asset churn races
  in-flight downloads. Not a pin at all.
* **Public runnable GHCR image.** Rejected by the issue: layer caching is
  cheaper per job, but it exercises a code path no user ever takes, and the
  bundle is the artifact HAL-5 must prove anyway.
* **Version from git plumbing (`git describe`).** The docker build context
  carries no `.git`, and the ARG already flows through the stage; a build.rs
  env read is the minimal wiring.

## Known trade-offs

* **One docker + cargo release build per push.** The clang layer comes from the
  existing GHCR registry cache, but the release stage runs the driver
  `cargo build --release` from scratch: the `EPIC_CC_VERSION` ARG changes
  every push and busts that RUN layer, and no cargo target cache is mounted.
  Cost is minutes per push, not hours. Every rolling release is also a
  release asset: acceptable, prereleases stay out of `releases/latest`.
* **Prereleases do not resolve through `releases/latest`.** A consumer must
  read the tag (`ci-<sha>`); a "latest" resolution endpoint is a follow-up if a
  consumer needs it.

## Revisit if

* A consumer needs Windows rolling bundles (add a windows job, same shape).
* A consumer needs a moving "latest" (a resolution helper or a periodic
  stable-marked release).
