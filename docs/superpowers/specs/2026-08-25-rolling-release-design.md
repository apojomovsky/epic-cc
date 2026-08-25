# Rolling master release bundles for downstream CI -- Design

**Status:** draft (pending approval)
**Date:** 2026-08-25
**Parent:** `docs/30-distribution-design.md` (D1-D6), ADR-008 (docker toolchain)
**Ticket:** `epic-cc#118`
**Consumes:** `apojomovsky/epic-hal#80` (the epic-hal side of this gate)
**Pattern source:** `apojomovsky/epic-hal/.github/workflows/release-bundles.yml` (build -> gate -> publish, `workflow_dispatch` as dry run)

---

## 1. Goal and non-goals

**Goal:** every push to `master` publishes the existing Linux release bundle as a
consumable GitHub release, tagged and stamped so a job in another repository can
pin it, fetch it, run it as `epic-cc --target p16f887 ...` with no clang build, and
report which compiler it used.

**Non-goals (v1):**

- **Windows rolling bundles.** The tag-triggered `release.yml` already ships both
  platforms. Downstream CI (`epic-hal#80`) is Linux-only; a Windows rolling job
  would rebuild `windows-latest` per push for zero consumers. When a consumer
  appears, this workflow's shape extends unchanged.
- **A public GHCR runnable image** (issue option 2). The issue itself rejects it:
  it exercises a code path no user ever takes. Rejected here for the same reason.
- **`epic-hal#80` itself.** Publishing is this ticket; consuming is the blocked
  epic-hal ticket. This design's gate job *is* the consumer's shape, so `#80` can
  consume without rework.
- **A "latest" endpoint.** `releases/latest` only resolves non-prerelease
  releases; all rolling releases are prereleases. A downstream job pins the
  commit it wants; humans read the Releases page. A resolution helper is a
  revisit-if.

## 2. Ground truth (what exists today)

- `release.yml` builds Linux (docker release stage) and Windows (native) zips,
  but only on a `v*` tag, and no tag exists yet: nothing downloadable.
- `ghcr.io/apojomovsky/epic-cc-toolchain` is a buildx *cache* ref, not a runnable
  image (ADR-008's layer cache).
- The bundle is real and self-contained: `epic-cc` + bundled `clang/` + LLVM
  license, discovered via the driver's bundled fallback (D6, no env needed).
- `EPIC_CC_VERSION` is a docker ARG that names the bundle directory only; it
  never reaches the binary. The driver has no `--version` output at all, so the
  acceptance criterion "artifact prints an identifying version or commit" has no
  implementation today.
- `p16f887` is a first-class device (`crates/device/devices/p16f887.toml`),
  so the acceptance command `epic-cc --target p16f887 ...` resolves.
- epic-hal's `release-bundles.yml` is the in-repo precedent for release
  engineering: build everything, prove the artifacts from a scratch location
  (checksums, smoke), and only then publish; `workflow_dispatch` runs the whole
  pipeline as a dry run with `publish` gated on the push event.

## 3. Approaches considered

### A -- Per-commit rolling release, one Linux zip (chosen)

A new `rolling-release.yml`: on every `push` to `master`, build the existing
Linux bundle with `EPIC_CC_VERSION=master-<sha>`, then:

1. **build** -- docker buildx `--target release` (clang layer from the existing
   GHCR cache), extract, zip.
2. **gate** -- download the *zip* on a bare runner, unzip, assert the shipped
   binary reports the commit, then run it as a consumer would: no clang env, a
   fresh C file, `--target p16f887`, HEX produced. Nothing about the compile is
   allowed in this job; only the artifact's own bundled clang.
3. **publish** -- `if: github.event_name == 'push'`, create a prerelease tagged
   `ci-<sha>` with the zip + `SHA256SUMS` + notes (built from which commit, how
   to use it). `workflow_dispatch` runs 1+2 as a dry run, exactly like epic-hal.

*Pros:* the artifact is byte-identical to what a user gets under HAL-5 (the
dogfooding argument from the issue); immutable per-commit tags make the pin
deterministic (a HAL job pinned to `ci-<sha>` always runs the same compiler,
and history stays reproducible); the gate on the bare runner is literally the
downstream job's shape, so `epic-hal#80` is a thin add; no new infrastructure.

*Cons:* one docker build + cargo release build per push (clang layer cached,
minutes not hours); release count grows with commits (GitHub handles it;
prereleases stay out of `latest`).

### B -- Single moving `master` prerelease (issue wording, option 1a)

One release tagged `master`, deleted and recreated each push.

*Rejected:* the download URL stays constant while the artifact changes, so a
pin is unreproducible: a CI job that ran yesterday pulled a different compiler
than the same URL gives today, and the run history can never be replayed. Asset
churn (delete + re-upload per push) also races with in-flight downloads. The
issue's intent, "a version or commit stamp the consumer can pin", is served
*better* by per-commit tags, which are pins by construction.

### C -- Public runnable image (issue option 2)

*Rejected by the issue*: cheaper per job via layer caching, but it exercises a
code path no user ever takes; the bundle is the artifact HAL-5 has to prove
anyway. Dogfooding wins.

---

## 4. Design

### 4.1 Workflow: `.github/workflows/rolling-release.yml`

```
on: push: branches: [master]; workflow_dispatch
permissions: contents: write, packages: write

build (ubuntu-latest)
  - buildx --target release --build-arg EPIC_CC_VERSION=master-<sha>
      --cache-from/to ghcr.io/...-toolchain (the existing cache ref)
  - extract bundle from the image, zip as epic-cc-master-<sha>-x86_64-linux.zip
  - upload artifact

gate (ubuntu-latest, needs build)
  - download the zip, unzip
  - ./epic-cc --version          assert it equals `epic-cc master-<sha>`
  - cat add.c | ./epic-cc --target p16f887 -o out.hex (no PIC8_* env)
    assert out.hex non-empty
  - sha256sum the zip, emit SHA256SUMS

publish (ubuntu-latest, needs build + gate, if: push)
  - download artifacts
  - gh release create ci-<sha> --prerelease --notes-file (real newlines)
    attach epic-cc-*.zip SHA256SUMS
```

The gate asserts the artifact's stamp against `${{ github.sha }}`, so a
misstamped bundle cannot be published: publish depends on gate.

### 4.2 The stamp reaches the binary

The Dockerfile `release` stage already receives `EPIC_CC_VERSION` as an ARG;
ARGs are environment variables in the `RUN` that invokes `cargo build`, so a
`build.rs` in the `driver` crate can pick it up:

```rust
// driver build.rs
fn main() {
    let stamp = std::env::var("EPIC_CC_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=EPIC_CC_STAMP={stamp}");
}
```

and `epic-cc --version` prints `epic-cc <stamp>`:
`epic-cc master-<sha>` in a rolling bundle, `epic-cc 0.1.0` in dev and tag
releases (where `EPIC_CC_VERSION` is unset, `CARGO_PKG_VERSION` is the
fallback). The stamp is also what `release.yml` passes today (the plain
version), so tag releases and rolling releases share one code path.

`--version` is handled before argument parsing (like any compiler's), exits 0,
and prints only the one line.

### 4.3 Bundle layout

The zip keeps the layout `docs/30` defines, plus the llvm-link this PR ships
next to clang: `epic-cc-<ver>-x86_64-linux/` containing `epic-cc`,
`clang/bin/clang`, `clang/bin/llvm-link`, `clang/lib/clang/20/`,
`LICENSE.clang.txt`. The rolling ver is `master-<sha>`.

### 4.4 Cache reuse

The workflow uses the same `ghcr.io/${{ github.repository }}-toolchain` cache
ref as `ci.yml`, so the clang layer is shared across both workflows. The
release stage's `cargo build --release` runs per push: the
`EPIC_CC_VERSION` ARG changes every push and busts that RUN layer, and no
cargo target cache is mounted. Cost is minutes per push, not hours.

## 5. Acceptance mapping

| Acceptance | Where |
|---|---|
| push to master publishes an artifact another job can fetch and run as `epic-cc --target p16f887 ...` with no clang build | `rolling-yml` build + gate (gate IS the consumer shape) |
| artifact prints an identifying version or commit | `--version` = `master-<sha>`, asserted by the gate |
| `apojomovsky/epic-hal#80` can consume it | consume the `ci-<sha>` prerelease, assert `--version` in the job log, run `make epiccc-build`; pin = the tag |

## 6. Testing

- The gate is the test for the artifact: version-stamp equality + a from-scratch
  `p16f887` compile via the bundled clang only.
- A unit/integration test in the driver crate for `--version` output shape
  (`epic-cc <stamp>`, exit 0).
- `make release-bundle VERSION=...` keeps working (same docker stage; local
  bundles simply have a different stamp).

## 7. Out of scope / follow-ups

- Windows rolling bundles (section 1).
- "latest" resolution endpoint (section 1).
- epic-hal#80 consumption.
- A future tagged release (`release.yml`) is unaffected and now shares the
  stamp code path.

## 8. Docs

- This design stays in `docs/superpowers/specs/` (design docs are not plans;
  ADR-022's parent spec persists the same way).
- The final commit distills this into ADR-023 with an index line in
  `docs/03-decisions.md`, and adds a "rolling master bundles" subsection to
  `docs/30-distribution-design.md`.
