# Distribution: bundled clang, docker toolchain, release pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship public Linux x86_64 and Windows x86_64 release bundles that embed a pinned clang 20.1.8 front end, built and tested from a single docker toolchain stack.

**Architecture:** A multi-stage Dockerfile (`base` → `clang-builder` → `dev`/`ci`/`release`) replaces the Nix flake. The Linux clang is built from the digest-pinned LLVM 20.1.8 source tarball with static LLVM libraries (no `libLLVM.so`, no rpath work); the Windows clang is sliced from the official LLVM MSVC bundle. The driver gains a clang discovery chain (env vars → bundled `clang/` next to the executable → clean diagnostic) and ships as `epic-cc`. A tag-triggered release workflow assembles both zips with per-platform smoke tests; CI runs the full suite inside the `ci` image.

**Tech Stack:** Docker (buildx, BuildKit cache mounts, GHCR registry cache), LLVM/Clang 20.1.8 (cmake/ninja), Rust 1.97.1 (rustup), gputils 1.5.2 (autotools), GitHub Actions.

**Spec:** `docs/30-distribution-design.md` — the plan argues from the spec; executors read both.

## Global Constraints

- **Bundle clang in every release** (spec D1). The bundled clang *is* the product's front end.
- **Single docker stack** for dev/CI/release (D2). The flake is retired only after CI is green on docker (Task 7).
- **Linux clang: LLVM 20.1.8 source tarball**, sha256 `6898f963c8e938981e6c4a302e83ec5beb4630147c7311183cf61069af16333d` (D3). Never a distro clang in the product — clang's version is part of our input format.
- **Minimum supported Linux: Ubuntu 22.04 (glibc ≥ 2.35)**. Base image `ubuntu:22.04@sha256:79676deb51ebb02885b0b9d33788e78a37cf1045ad79d1bb04c6a222c3556b3d` (D4).
- **gputils 1.5.2 built from source**, sha256 `62a215e7d5575cd488a5ada66e5708ff402634abe86a9b39e4dbdb19c986ab7e`; test oracle only, never shipped (D5). Tarball lives under SourceForge's `1.5.0` directory.
- **Driver discovery order:** env vars (both-or-neither) → bundled `clang/` → clean error, never a panic; binary named `epic-cc` (D6).
- **Windows: slice the official `clang+llvm-20.1.8-x86_64-pc-windows-msvc.tar.xz`** (D7).
- **PlatformIO integration is phase 2** — out of scope for this plan (D8).
- **gputils/gpasm are GPL:** external-process test-only, never linked or shipped.
- **Conventional commits**, single line, ≤ 3 lines (repo convention).
- Until Task 7, the nix shell remains a valid local dev entry point; docker commands are additive.

---

### Task 1: Driver clang discovery + `epic-cc` binary name

**Files:**
- Create: `crates/driver/src/clang_discovery.rs`
- Modify: `crates/driver/src/main.rs`
- Modify: `crates/driver/Cargo.toml`
- Modify: all 22 files under `crates/driver/tests/` that use `CARGO_BIN_EXE_driver`

**Interfaces:**
- Produces: `clang_discovery::resolve_clang(env: &HashMap<String, String>, exe_dir: &Path) -> Result<(PathBuf, PathBuf), String>` returning `(clang_binary, resource_dir)`; binary `epic-cc` (built by `cargo build -p driver` / run by `cargo run -p driver`); test env var `CARGO_BIN_EXE_epic-cc` (cargo sets it for the `epic-cc` bin target — verified).
- Consumes: nothing from other tasks.

- [ ] **Step 1: Write the failing tests** — create `crates/driver/src/clang_discovery.rs` with the function signature stubbed via `unimplemented!()` and four tests:

```rust
//! clang front-end discovery for the epic-cc driver.
//!
//! The driver needs two things from clang: the binary and the resource dir
//! (builtin headers). Resolution order:
//!
//! 1. `PIC8_CLANG_UNWRAPPED` + `PIC8_CLANG_RESOURCE_DIR` env vars (dev/CI
//!    path — the docker images export these).
//! 2. Bundled: `<exe_dir>/clang/bin/clang` with the first subdirectory of
//!    `<exe_dir>/clang/lib/clang/` as the resource dir (release bundles).
//! 3. A clean error naming both options — never a panic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resolve `(clang_binary, resource_dir)`.
pub fn resolve_clang(
    env: &HashMap<String, String>,
    exe_dir: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn fake_bundle(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("epic-cc-discovery-{tag}"));
        let bin = dir.join("clang").join("bin");
        let res = dir.join("clang").join("lib").join("clang").join("20");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(bin.join("clang"), "#!/bin/sh\n").unwrap();
        dir
    }

    #[test]
    fn env_vars_win() {
        let env = env_of(&[
            ("PIC8_CLANG_UNWRAPPED", "/usr/bin/clang"),
            ("PIC8_CLANG_RESOURCE_DIR", "/usr/lib/clang/20"),
        ]);
        let (clang, resdir) = resolve_clang(&env, Path::new("/nonexistent")).unwrap();
        assert_eq!(clang, PathBuf::from("/usr/bin/clang"));
        assert_eq!(resdir, PathBuf::from("/usr/lib/clang/20"));
    }

    #[test]
    fn bundled_fallback() {
        let dir = fake_bundle("bundled");
        let (clang, resdir) = resolve_clang(&HashMap::new(), &dir).unwrap();
        assert_eq!(clang, dir.join("clang").join("bin").join("clang"));
        assert_eq!(resdir, dir.join("clang").join("lib").join("clang").join("20"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundled_windows_exe_name() {
        let dir = fake_bundle("windows");
        std::fs::rename(
            dir.join("clang").join("bin").join("clang"),
            dir.join("clang").join("bin").join("clang.exe"),
        )
        .unwrap();
        let (clang, _) = resolve_clang(&HashMap::new(), &dir).unwrap();
        assert_eq!(clang, dir.join("clang").join("bin").join("clang.exe"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn neither_errors() {
        let err = resolve_clang(&HashMap::new(), Path::new("/nonexistent")).unwrap_err();
        assert!(err.contains("PIC8_CLANG_UNWRAPPED"), "err: {err}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `nix develop --command cargo test -p driver clang_discovery::`
Expected: FAIL — all four tests panic on `unimplemented!()`.

- [ ] **Step 3: Implement `resolve_clang`** — replace the stub body:

```rust
    // 1. Env vars, both-or-neither.
    let env_clang = env.get("PIC8_CLANG_UNWRAPPED");
    let env_resdir = env.get("PIC8_CLANG_RESOURCE_DIR");
    if let (Some(clang), Some(resdir)) = (env_clang, env_resdir) {
        return Ok((PathBuf::from(clang), PathBuf::from(resdir)));
    }

    // 2. Bundled clang next to the executable.
    let bundled_clang = ["clang", "clang.exe"]
        .iter()
        .map(|name| exe_dir.join("clang").join("bin").join(name))
        .find(|p| p.is_file());
    if let Some(clang) = bundled_clang {
        let res_root = exe_dir.join("clang").join("lib").join("clang");
        if let Ok(entries) = std::fs::read_dir(&res_root) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    return Ok((clang, entry.path()));
                }
            }
        }
        return Err(format!(
            "bundled clang found at {} but no resource dir under {}",
            clang.display(),
            res_root.display()
        ));
    }

    // 3. Clean diagnostic.
    Err("no clang front end found: set PIC8_CLANG_UNWRAPPED and \
         PIC8_CLANG_RESOURCE_DIR, or ship the clang/ directory next to the \
         executable"
        .to_string())
```

- [ ] **Step 4: Run to verify it passes**

Run: `nix develop --command cargo test -p driver clang_discovery::`
Expected: PASS — 4 tests.

- [ ] **Step 5: Wire the discovery into `main.rs`** — replace the header and the clang invocation block:

Current header:
```rust
use std::collections::HashMap;
use std::process::Command;
```
Replace with:
```rust
mod clang_discovery;

use clang_discovery::resolve_clang;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
```

Current block (lines ~19-21):
```rust
    // 1. clang: .c -> .ll (text on stdout)
    let clang = std::env::var("PIC8_CLANG_UNWRAPPED").expect("PIC8_CLANG_UNWRAPPED");
    let resdir = std::env::var("PIC8_CLANG_RESOURCE_DIR").expect("PIC8_CLANG_RESOURCE_DIR");
```
Replace with:
```rust
    // 1. clang: .c -> .ll (text on stdout). Resolved from the env vars, or
    // from the bundled clang/ directory next to the executable, or a clean
    // error (see clang_discovery).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let (clang, resdir) = match resolve_clang(&std::env::vars().collect(), &exe_dir) {
        Ok(pair) => pair,
        Err(msg) => {
            eprintln!("epic-cc: {msg}");
            std::process::exit(1);
        }
    };
```

The rest of `main.rs` is unchanged.

- [ ] **Step 6: Add the `[[bin]]` to `Cargo.toml`** — append to `crates/driver/Cargo.toml`:

```toml
[[bin]]
name = "epic-cc"
path = "src/main.rs"
```

(Verified: with one explicit `[[bin]]`, cargo builds only `epic-cc`; `cargo run -p driver` runs it; no `autobins = false` needed.)

- [ ] **Step 7: Rename the test binary env var in all driver tests**

Run:
```bash
grep -rl "CARGO_BIN_EXE_driver" crates/driver/tests | xargs sed -i 's/CARGO_BIN_EXE_driver/CARGO_BIN_EXE_epic-cc/g'
grep -rn "CARGO_BIN_EXE_driver" crates/driver/tests || echo "no remaining references"
```
Expected: second grep prints nothing (or the `||` echo fires).

- [ ] **Step 8: Run the full driver test suite**

Run: `nix develop --command cargo test -p driver`
Expected: PASS — all e2e tests (they now invoke `epic-cc`) plus the 4 discovery unit tests.

- [ ] **Step 9: Manual smoke — positive and negative**

Run:
```bash
nix develop --command cargo run -p driver -- crates/driver/tests/fixtures/add.c /tmp/add.hex
```
Expected: exit 0, `/tmp/add.hex` written.

Run:
```bash
nix develop --command env -u PIC8_CLANG_UNWRAPPED -u PIC8_CLANG_RESOURCE_DIR cargo run -p driver -- crates/driver/tests/fixtures/add.c /tmp/add2.hex
```
Expected: exit 1, stderr starts with `epic-cc: no clang front end found: set PIC8_CLANG_UNWRAPPED and PIC8_CLANG_RESOURCE_DIR, or ship the clang/ directory next to the executable` — a clean diagnostic, **not a panic**.

- [ ] **Step 10: Commit**

```bash
git add crates/driver
git commit -m "feat(driver): bundled clang discovery and epic-cc binary name"
```

---

### Task 2: Dockerfile — `base` and `clang-builder` stages

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

**Interfaces:**
- Produces: docker image targets `base` (build tools + oracles) and `clang-builder` (clang 20.1.8 at `/opt/clang/bin/clang` + `/opt/clang/lib/clang/20`); `gpasm` 1.5.2 at `/usr/local/bin/gpasm`.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Create `.dockerignore`**

```
.git
target
vendor
.direnv
.omp
```

- [ ] **Step 2: Create the `Dockerfile` with `base` and `clang-builder` stages**

```dockerfile
# syntax=docker/dockerfile:1
#
# epic-cc toolchain images. Single source of truth for the build/test/release
# environment (docs/30-distribution-design.md, ADR-008).
#
# Stages:
#   base          — ubuntu:22.04 (digest-pinned; glibc 2.35 = the minimum
#                   supported Linux) + build tools + test oracles
#   clang-builder — LLVM 20.1.8 from the digest-pinned source tarball, static
#                   LLVM libs (no libLLVM.so, no rpath work). The expensive
#                   layer; cached in GHCR via the buildx registry cache.
#   dev           — clang-builder + rustup 1.97.1 (rust-toolchain.toml) + env
#   ci            — dev; runs scripts/ci-test.sh (what CI executes)
#   release       — dev; builds epic-cc and assembles the distribution bundle

FROM ubuntu:22.04@sha256:79676deb51ebb02885b0b9d33788e78a37cf1045ad79d1bb04c6a222c3556b3d AS base

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        ninja-build \
        zlib1g-dev \
        ccache \
        python3 \
        git \
        curl \
        ca-certificates \
        xz-utils \
        file \
        csmith \
        creduce \
        poppler-utils \
    && rm -rf /var/lib/apt/lists/*

# gputils 1.5.2 — test oracle (gpasm byte-for-byte cross-checks). Built from
# source: apt (jammy) has 1.4.0 and the cross-checks are version-sensitive.
# Note the tarball lives under the 1.5.0 directory on SourceForge.
RUN curl -fsSL -o /tmp/gputils.tar.gz \
        https://downloads.sourceforge.net/project/gputils/gputils/1.5.0/gputils-1.5.2.tar.gz \
    && echo "62a215e7d5575cd488a5ada66e5708ff402634abe86a9b39e4dbdb19c986ab7e  /tmp/gputils.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/gputils.tar.gz -C /tmp \
    && cd /tmp/gputils-1.5.2 \
    && ./configure --prefix=/usr/local \
    && make -j"$(nproc)" \
    && make install \
    && rm -rf /tmp/gputils-1.5.2 /tmp/gputils.tar.gz

FROM base AS clang-builder

# LLVM 20.1.8 source, digest-pinned. clang's version is part of our input
# format (we parse .ll text) — bumping is a migration, not housekeeping.
RUN curl -fsSL -o /tmp/llvm.tar.xz \
        https://github.com/llvm/llvm-project/releases/download/llvmorg-20.1.8/llvm-project-20.1.8.src.tar.xz \
    && echo "6898f963c8e938981e6c4a302e83ec5beb4630147c7311183cf61069af16333d  /tmp/llvm.tar.xz" | sha256sum -c - \
    && mkdir -p /src/llvm \
    && tar -xJf /tmp/llvm.tar.xz -C /src/llvm --strip-components=1 \
    && rm /tmp/llvm.tar.xz

# Static LLVM libraries (the Linux default): the produced clang links only
# platform runtimes — no libLLVM.so, nothing to bundle or rpath-patch.
# MSP430-only targets: we only need the datalayout-proxy target registered
# for `-S -emit-llvm`. If the spike in Step 5 fails, drop
# -DLLVM_TARGETS_TO_BUILD (default = all targets).
RUN --mount=type=cache,target=/ccache \
    cmake -S /src/llvm/llvm -B /build/llvm -G Ninja \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/opt/clang \
        -DLLVM_ENABLE_PROJECTS=clang \
        -DLLVM_TARGETS_TO_BUILD=MSP430 \
        -DLLVM_INCLUDE_TESTS=OFF \
        -DLLVM_INCLUDE_BENCHMARKS=OFF \
        -DLLVM_INCLUDE_EXAMPLES=OFF \
        -DLLVM_CCACHE_BUILD=ON \
        -DLLVM_CCACHE_DIR=/ccache \
    && cmake --build /build/llvm --target install -j"$(nproc)" \
    && rm -rf /build/llvm /src/llvm
```

(Stages `dev`/`ci`/`release` are added in Tasks 3 and 4 — the file is incomplete until then, which is fine for this task's verification.)

- [ ] **Step 3: Build `base` and verify the oracles**

Run:
```bash
docker build --target base -t epic-cc-base .
docker run --rm epic-cc-base gpasm --version
docker run --rm epic-cc-base csmith --version
docker run --rm epic-cc-base creduce --version
```
Expected: gpasm reports **1.5.2** (not 1.4.0), csmith 2.3.0, creduce 2.10.0.

- [ ] **Step 4: Build `clang-builder`** (the long one)

Run: `docker build --target clang-builder -t epic-cc-clang .`
Expected: build succeeds. First build is ~30-60 min with MSP430-only targets (longer with default targets); subsequent builds hit the layer cache.

- [ ] **Step 5: MSP430 spike — `-emit-llvm` with MSP430-only targets**

Run:
```bash
docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-clang \
  /opt/clang/bin/clang -target msp430 -O1 -S -emit-llvm -ffreestanding -nostdinc \
  -resource-dir /opt/clang/lib/clang/20 -o /tmp/add.ll \
  crates/driver/tests/fixtures/add.c
```
Expected: exit 0; `/tmp/add.ll` contains `target datalayout = "e-m:e-p:16:16-i32:16-i64:16-f32:16-f64:16-a:8-n8:16-S16"`.

**If clang errors** (e.g. target not registered): remove `-DLLVM_TARGETS_TO_BUILD=MSP430` from the Dockerfile (default = all targets), rebuild `clang-builder`, and re-run this step. Document the fallback in the Dockerfile comment.

- [ ] **Step 6: Verify the rpath-free claim**

Run: `docker run --rm epic-cc-clang ldd /opt/clang/bin/clang`
Expected: only `libstdc++.so.6`, `libgcc_s.so.1`, `libc.so.6`, `libm.so.6`, `linux-vdso.so.1` — **no `libLLVM.so`**, no absolute store paths.

- [ ] **Step 7: Commit**

```bash
git add Dockerfile .dockerignore
git commit -m "build(docker): base and clang-builder stages"
```

---

### Task 3: Dockerfile — `dev` and `ci` stages; pin rustc 1.97.1

**Files:**
- Modify: `Dockerfile` (append `dev` and `ci` stages)
- Modify: `rust-toolchain.toml`

**Interfaces:**
- Consumes: Task 2's `clang-builder` stage.
- Produces: image targets `dev` and `ci`; env vars `PIC8_CLANG_UNWRAPPED=/opt/clang/bin/clang`, `PIC8_CLANG_RESOURCE_DIR=/opt/clang/lib/clang/20`, `PIC8_GPASM=/usr/local/bin/gpasm`, `PIC8_VENDOR_DIR=/workspace/vendor`, `PIC8_XC8_ROOT=/opt/microchip/xc8/v4.00`; rustc 1.97.1.

- [ ] **Step 1: Pin rustc explicitly in `rust-toolchain.toml`**

Replace the whole file:
```toml
[toolchain]
# Pinned explicitly: the docker dev image installs exactly this version, and
# rustup honors this file on first `cargo` use. Bumping is a deliberate
# migration (see docs/09-build-environment.md).
channel = "1.97.1"
components = ["rustfmt", "clippy", "rust-src"]
profile = "default"
```

Verify the nix shell still resolves it (nix is still the local dev env until Task 7):
Run: `nix develop --command rustc --version`
Expected: `rustc 1.97.1`.

- [ ] **Step 2: Append the `dev` and `ci` stages to the `Dockerfile`**

```dockerfile
FROM clang-builder AS dev

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# rustc 1.97.1, pinned in rust-toolchain.toml (kept in sync deliberately).
RUN curl -fsSL https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.97.1 \
    && rustup component add rustfmt clippy rust-src \
    && rm /tmp/rustup.sh

ENV PIC8_CLANG_UNWRAPPED=/opt/clang/bin/clang \
    PIC8_CLANG_RESOURCE_DIR=/opt/clang/lib/clang/20 \
    PIC8_GPASM=/usr/local/bin/gpasm \
    PIC8_VENDOR_DIR=/workspace/vendor \
    PIC8_XC8_ROOT=/opt/microchip/xc8/v4.00

WORKDIR /workspace

FROM dev AS ci
# ci-test.sh runs from the mounted workspace; nothing extra to install.
```

- [ ] **Step 3: Build `dev` and verify the toolchain**

Run:
```bash
docker build --target dev -t epic-cc-dev .
docker run --rm epic-cc-dev rustc --version
docker run --rm epic-cc-dev clang --version
docker run --rm epic-cc-dev printenv PIC8_CLANG_UNWRAPPED PIC8_CLANG_RESOURCE_DIR PIC8_GPASM
```
Expected: `rustc 1.97.1`, clang `20.1.8`, and the three env vars print the paths above.

- [ ] **Step 4: Run the driver and asm test suites inside the container**

Run:
```bash
docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-dev bash -c "cargo test -p driver && cargo test -p asm"
```
Expected: PASS — driver e2e tests (clang via env vars) and the 14 gpasm byte-for-byte cross-checks (gpasm 1.5.2 via `PIC8_GPASM`). First run downloads the crates.io registry and compiles deps — a few minutes.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile rust-toolchain.toml
git commit -m "build(docker): dev and ci stages; pin rustc 1.97.1"
```

---

### Task 4: Dockerfile — `release` stage with in-image bundle smoke test

**Files:**
- Modify: `Dockerfile` (append `release` stage)

**Interfaces:**
- Consumes: Task 1 (`epic-cc` binary), Task 3 (`dev` stage).
- Produces: image target `release`; bundle at `/out/epic-cc-<ver>-x86_64-linux/` containing `epic-cc`, `clang/bin/clang`, `clang/lib/clang/20/` (builtin headers), `LICENSE.clang.txt`.

- [ ] **Step 1: Append the `release` stage to the `Dockerfile`**

```dockerfile
FROM dev AS release

ARG EPIC_CC_VERSION=dev
COPY . /workspace
WORKDIR /workspace

# Build the release binary and assemble the bundle. The smoke test runs the
# shipped binary twice — once via env vars, once via bundled discovery — and
# requires byte-identical HEX, proving the bundle's clang loads and the
# driver finds it.
RUN cargo build --release -p driver \
    && mkdir -p "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/clang/bin" \
    && cp target/release/epic-cc "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/" \
    && cp /opt/clang/bin/clang "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/clang/bin/" \
    && mkdir -p "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/clang/lib" \
    && cp -r /opt/clang/lib/clang "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/clang/lib/" \
    && cp /opt/clang/LICENSE.TXT "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/LICENSE.clang.txt" \
    && cd "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux" \
    && ./epic-cc /workspace/crates/driver/tests/fixtures/add.c /tmp/env.hex \
    && env -u PIC8_CLANG_UNWRAPPED -u PIC8_CLANG_RESOURCE_DIR \
       ./epic-cc /workspace/crates/driver/tests/fixtures/add.c /tmp/bundled.hex \
    && cmp /tmp/env.hex /tmp/bundled.hex
```

- [ ] **Step 2: Build the release stage and export the bundle**

Run:
```bash
docker buildx build --target release --build-arg EPIC_CC_VERSION=0.1.0-test \
  --output type=local,dest=/tmp/dist .
```
Expected: build succeeds — which means the in-image smoke test passed (env-path HEX == bundled-discovery HEX, byte for byte). The build log shows the smoke `RUN` step completing.

- [ ] **Step 3: Verify the bundle layout**

Run:
```bash
find /tmp/dist/epic-cc-0.1.0-test-x86_64-linux -type f | sort
```
Expected:
```
.../epic-cc
.../clang/bin/clang
.../clang/lib/clang/20/include/stddef.h
.../clang/lib/clang/20/include/stdint.h
.../LICENSE.clang.txt
```
(plus the rest of the builtin headers under `include/`).

- [ ] **Step 4: Commit**

```bash
git add Dockerfile
git commit -m "build(docker): release stage with bundle smoke test"
```

---

### Task 5: CI workflow → docker `ci` image

**Files:**
- Modify: `.github/workflows/ci.yml` (rewrite)
- Modify: `scripts/ci-test.sh` (error message)
- Modify: `crates/fuzz/src/lib.rs` (error messages)

**Interfaces:**
- Consumes: Task 3's `ci` image.
- Produces: CI that runs the full suite inside the docker `ci` image with GHCR registry cache.

- [ ] **Step 1: Update the `ci-test.sh` error message**

In `scripts/ci-test.sh`, replace:
```bash
  echo "::error::cargo not on PATH; run inside \`nix develop\` (see docs/09-build-environment.md)" >&2
```
with:
```bash
  echo "::error::cargo not on PATH; run inside the dev container (see docs/09-build-environment.md)" >&2
```

- [ ] **Step 2: Update the fuzz crate's error messages**

In `crates/fuzz/src/lib.rs` (lines ~2959-2962), replace both `"PIC8_CLANG_UNWRAPPED is not set (run inside \`nix develop\`)"` and `"PIC8_CLANG_RESOURCE_DIR is not set (run inside \`nix develop\`)"` with the same text ending `(run inside the dev container)`.

- [ ] **Step 3: Rewrite `.github/workflows/ci.yml`**

```yaml
name: ci

# The repo's one CI workflow: run the full Rust workspace test suite inside
# the docker `ci` image (Dockerfile). The Dockerfile is the single source of
# truth for the toolchain — clang 20.1.8 built from the digest-pinned source
# tarball (the IR producer, deliberately pinned, see
# docs/09-build-environment.md), rustc 1.97.1 (rust-toolchain.toml), gpasm
# 1.5.2 (built from source) — so CI never installs a toolchain of its own
# and cannot drift from what the dev container gives locally.
#
# Single job, deliberate: the suite is one `cargo test` invocation's worth
# of work (~30s on a runner), there is nothing to split, and a one-job
# workflow keeps the PR check list to a single entry. The per-crate PASS/FAIL
# table inside the job (scripts/ci-test.sh) is what makes failures
# attributable. When real split points exist (XC8 oracles, release bundles,
# a simulator gate), split here the way epic-hal's family-check.yml does.
#
# Image caching: the clang-builder stage is the expensive layer (~1h first
# build). It is cached in GHCR (ghcr.io/<repo>-toolchain) via the buildx
# registry cache, keyed on the Dockerfile content — rebuilt only when the
# Dockerfile or a pinned tarball changes. actions/cache warms the cargo
# target dir and the cargo home across runs.

on:
  push:
    branches: [master]
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Build ci image (cached)
        uses: docker/build-push-action@v6
        with:
          target: ci
          load: true
          tags: epic-cc-ci:latest
          cache-from: type=registry,ref=ghcr.io/${{ github.repository }}-toolchain
          cache-to: type=registry,ref=ghcr.io/${{ github.repository }}-toolchain,mode=max

      - name: Cache cargo target dir
        uses: actions/cache@v5
        with:
          path: target
          key: cargo-${{ runner.os }}-${{ github.workflow }}-${{ github.sha }}
          restore-keys: |
            cargo-${{ runner.os }}-${{ github.workflow }}-

      - name: Cache cargo home
        uses: actions/cache@v5
        with:
          path: /tmp/cargo-home
          key: cargo-home-${{ runner.os }}-${{ hashFiles('Cargo.lock') }}

      - name: Test every workspace crate
        run: docker run --rm -v "$PWD:/workspace" -w /workspace -v /tmp/cargo-home:/usr/local/cargo epic-cc-ci:latest bash scripts/ci-test.sh
```

- [ ] **Step 4: Verify locally — the same commands the workflow runs**

Run:
```bash
docker buildx build --target ci --load -t epic-cc-ci:latest .
docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-ci:latest bash scripts/ci-test.sh
```
Expected: the per-crate PASS/FAIL table — **all crates PASS** (this is the full workspace suite: 354+ tests, including the gpasm cross-checks and all e2e tests).

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml scripts/ci-test.sh crates/fuzz/src/lib.rs
git commit -m "ci: run the test suite in the docker ci image"
```

---

### Task 6: Tag-triggered release workflow (Linux + Windows bundles)

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: Task 1 (`epic-cc` binary), Task 4 (`release` stage).
- Produces: on tag `v*` (or manual dispatch), two zips `epic-cc-<ver>-x86_64-linux.zip` and `epic-cc-<ver>-x86_64-windows.zip` + `SHA256SUMS`, attached to a GitHub release.

- [ ] **Step 1: Create `.github/workflows/release.yml`**

```yaml
name: release

# Tag-triggered: builds the Linux bundle in the docker release stage and the
# Windows bundle by slicing the official LLVM MSVC release, smoke-tests each
# shipped binary, and attaches both zips + checksums to the GitHub release.
# workflow_dispatch is enabled so the pipeline can be exercised before the
# first real tag.

on:
  push:
    tags: ["v*"]
  workflow_dispatch:

permissions:
  contents: write

jobs:
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Build release bundle (cached clang layer)
        run: |
          ver="${GITHUB_REF_NAME#v}"
          docker buildx build --target release \
            --build-arg EPIC_CC_VERSION="$ver" \
            --cache-from type=registry,ref=ghcr.io/${{ github.repository }}-toolchain \
            --cache-to type=registry,ref=ghcr.io/${{ github.repository }}-toolchain,mode=max \
            --output type=local,dest=dist .

      - name: Zip
        run: |
          ver="${GITHUB_REF_NAME#v}"
          (cd dist && zip -qr "../epic-cc-${ver}-x86_64-linux.zip" "epic-cc-${ver}-x86_64-linux")

      - uses: actions/upload-artifact@v4
        with:
          name: linux-bundle
          path: epic-cc-*.zip

  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5

      - uses: dtolnay/rust-toolchain@1.97.1
        with:
          components: rustfmt, clippy, rust-src

      - name: Build epic-cc
        run: cargo build --release -p driver

      - name: Download and extract LLVM 20.1.8 MSVC bundle
        run: |
          curl.exe -fsSL -o llvm.tar.xz "https://github.com/llvm/llvm-project/releases/download/llvmorg-20.1.8/clang%2Bllvm-20.1.8-x86_64-pc-windows-msvc.tar.xz"
          tar -xJf llvm.tar.xz

      - name: Assemble bundle
        shell: pwsh
        run: |
          $ver = $env:GITHUB_REF_NAME -replace '^v',''
          $src = "clang+llvm-20.1.8-x86_64-pc-windows-msvc"
          $bundle = "epic-cc-$ver-x86_64-windows"
          New-Item -ItemType Directory -Force -Path "$bundle\clang\bin", "$bundle\clang\lib" | Out-Null
          Copy-Item "$src\bin\clang.exe" "$bundle\clang\bin\"
          Copy-Item "$src\bin\*.dll" "$bundle\clang\bin\"
          Copy-Item -Recurse "$src\lib\clang" "$bundle\clang\lib\clang"
          Copy-Item "target\release\epic-cc.exe" "$bundle\"
          Copy-Item "$src\LICENSE.TXT" "$bundle\LICENSE.clang.txt"

      - name: Smoke test (env path vs bundled discovery)
        shell: pwsh
        run: |
          $ver = $env:GITHUB_REF_NAME -replace '^v',''
          $bundle = "epic-cc-$ver-x86_64-windows"
          Push-Location $bundle
          $env:PIC8_CLANG_UNWRAPPED = "$PWD\clang\bin\clang.exe"
          $env:PIC8_CLANG_RESOURCE_DIR = "$PWD\clang\lib\clang\20"
          .\epic-cc.exe ..\crates\driver\tests\fixtures\add.c env.hex
          Remove-Item Env:PIC8_CLANG_UNWRAPPED
          Remove-Item Env:PIC8_CLANG_RESOURCE_DIR
          .\epic-cc.exe ..\crates\driver\tests\fixtures\add.c bundled.hex
          if ((Get-FileHash env.hex).Hash -ne (Get-FileHash bundled.hex).Hash) {
            throw "smoke mismatch: env-path and bundled-discovery HEX differ"
          }
          Pop-Location

      - name: Zip
        shell: pwsh
        run: |
          $ver = $env:GITHUB_REF_NAME -replace '^v',''
          Compress-Archive -Path "epic-cc-$ver-x86_64-windows" -DestinationPath "epic-cc-$ver-x86_64-windows.zip"

      - uses: actions/upload-artifact@v4
        with:
          name: windows-bundle
          path: epic-cc-*.zip

  release:
    needs: [linux, windows]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist

      - name: Create GitHub release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          cd dist
          sha256sum linux-bundle/*.zip windows-bundle/*.zip > SHA256SUMS
          gh release create "$GITHUB_REF_NAME" \
            linux-bundle/*.zip windows-bundle/*.zip SHA256SUMS \
            --title "epic-cc ${GITHUB_REF_NAME#v}" \
            --generate-notes
```

- [ ] **Step 2: Verify the Linux leg locally**

Run (with a test version, no tag needed):
```bash
docker buildx build --target release --build-arg EPIC_CC_VERSION=0.1.0-test \
  --output type=local,dest=/tmp/dist .
(cd /tmp/dist && zip -qr /tmp/epic-cc-0.1.0-test-x86_64-linux.zip epic-cc-0.1.0-test-x86_64-linux)
unzip -l /tmp/epic-cc-0.1.0-test-x86_64-linux.zip
```
Expected: the zip contains `epic-cc`, `clang/bin/clang`, `clang/lib/clang/20/include/...`, `LICENSE.clang.txt`; the build's in-image smoke passed (build succeeded).

- [ ] **Step 3: Verify the Windows leg**

The Windows leg cannot run on a Linux host. Verification is: (a) the PowerShell steps are reviewed for correctness against the official bundle layout (`bin/clang.exe`, `bin/*.dll`, `lib/clang/20`, `LICENSE.TXT` — confirmed present in the official asset), and (b) the workflow is exercised via `workflow_dispatch` on a real `windows-latest` runner before the first tagged release. Push the branch and trigger the workflow manually; the `windows` job must complete with the smoke test passing.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: tag-triggered release workflow with per-platform smoke"
```

---

### Task 7: Retire the flake; document the docker toolchain

**Files:**
- Delete: `flake.nix`, `flake.lock`, `.envrc`
- Modify: `.gitignore` (remove `/.direnv/`)
- Rewrite: `docs/09-build-environment.md`
- Modify: `docs/03-decisions.md` (add ADR-008, superseding ADR-007)
- Modify: `docs/06-environment.md`
- Modify: `docs/08-status-and-next-steps.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: Task 5 (CI green on docker — the gate for retiring nix).

- [ ] **Step 1: Delete the nix files**

Run:
```bash
git rm flake.nix flake.lock .envrc
```
In `.gitignore`, remove the line `/.direnv/`.

- [ ] **Step 2: Rewrite `docs/09-build-environment.md`**

Replace the whole file with:

```markdown
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
```

- [ ] **Step 3: Add ADR-008 to `docs/03-decisions.md`** — append after the ADR-007 section:

```markdown
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
```

- [ ] **Step 4: Update `docs/06-environment.md`**

Replace the "All build and test dependencies come from the Nix flake dev shell…" block (lines ~6-20) with:

```markdown
**All build and test dependencies come from the docker dev image.** See
[`09-build-environment.md`](09-build-environment.md) for the full picture; the short
version:

```bash
docker build --target dev -t epic-cc-dev .   # first build is slow (clang)
docker run --rm -it -v "$PWD:/workspace" -w /workspace epic-cc-dev bash
```

This provides pinned `clang` 20.1.8, `rustc`/`cargo` 1.97.1, `gpasm` 1.5.2, `cvise`,
`creduce`, `csmith`, and `pdftotext`. Do **not** `apt install` these on the host — a
host-installed version shadowing the pinned one is exactly the drift the image exists
to prevent.

Host-provided and used as-is: `git`, `gh`, `curl`, `docker`.
```

And replace the line "`pdftotext` and `pdfinfo` come from the Nix shell, so run these inside `nix develop`." with "`pdftotext` and `pdfinfo` come from the dev image, so run these inside the container."

- [ ] **Step 5: Update `docs/08-status-and-next-steps.md`**

Replace "The Nix dev shell **is** built and verified — see [`09-build-environment.md`](09-build-environment.md). `direnv allow`, then you have pinned clang 20.1.8, rustc 1.97.1, gpasm 1.5.2, cvise, creduce, and csmith." with the docker equivalent (`docker build --target dev`, then the same pins).

Replace the table row "| Build environment (Nix flake) | ✅ done and verified — [ADR-007](03-decisions.md), [`09`](09-build-environment.md) |" with "| Build environment (docker) | ✅ done and verified — [ADR-008](03-decisions.md), [`09`](09-build-environment.md) |".

Replace "8. **Build isolation:** Nix flake + direnv, nothing installed system-wide; clang pinned to 20.1.8 ([ADR-007](03-decisions.md))." with "8. **Build isolation:** docker multi-stage toolchain, nothing installed system-wide; clang pinned to 20.1.8 ([ADR-008](03-decisions.md))."

- [ ] **Step 6: Update `README.md`**

1. In the "What it builds on" table, replace the row:
   `| **Nix + direnv** | The whole toolchain, pinned in \`flake.lock\`. clang's version is part of our *input format*, so a silent bump could change what the parser sees. | [\`docs/09-build-environment.md\`](docs/09-build-environment.md) |`
   with:
   `| **Docker (multi-stage)** | The whole toolchain, built from a digest-pinned \`ubuntu:22.04\` base + the LLVM 20.1.8 source tarball + \`rust-toolchain.toml\`. clang's version is part of our *input format*, so a silent bump could change what the parser sees. | [\`docs/09-build-environment.md\`](docs/09-build-environment.md) |`

2. In "Getting started", replace the direnv/nix block (lines ~267-296) with:

```markdown
Everything comes from a docker multi-stage build, so **install nothing system-wide.**

```bash
docker build --target dev -t epic-cc-dev .    # first build is slow (compiles clang)
docker run --rm -it -v "$PWD:/workspace" -w /workspace epic-cc-dev bash
```

Inside the container:

```bash
cargo test --workspace            # 354 tests
bash scripts/ci-test.sh           # per-crate PASS/FAIL table (what CI runs)
```

Compile a C file to Intel HEX:

```bash
cargo run -p driver -- crates/driver/tests/fixtures/add.c out.hex
```

Run the slow fuzz corpora:

```bash
cargo test -p fuzz -- --ignored
```

Pinned by the Dockerfile: **rustc 1.97.1**, **clang 20.1.8** (source tarball),
**gpasm 1.5.2** (source), plus csmith, creduce and cvise. Gotchas and the
caching story are in [`docs/09-build-environment.md`](docs/09-build-environment.md).
```

3. In "Repository layout", replace the line `flake.nix      # the pinned toolchain` with `Dockerfile     # the pinned toolchain (multi-stage)`.

- [ ] **Step 7: Verify the docker path is the only path, and it is green**

Run:
```bash
docker buildx build --target ci --load -t epic-cc-ci:latest .
docker run --rm -v "$PWD:/workspace" -w /workspace epic-cc-ci:latest bash scripts/ci-test.sh
```
Expected: all crates PASS. Also confirm no remaining nix references:
```bash
grep -rn "nix develop\|flake" --include="*.md" --include="*.rs" --include="*.sh" --include="*.yml" . | grep -v "docs/superpowers" || echo "clean"
```
Expected: only historical references in old plan docs (under `docs/superpowers/plans/` and `docs/1[3-9]-*.md`) remain — those are historical records and stay.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "chore: retire the nix flake; document the docker toolchain"
```

---

## Self-review notes

- **Spec coverage:** D1 (Task 4/6 bundles), D2 (Tasks 2-5, 7), D3 (Task 2 tarball pin), D4 (Task 2 base digest), D5 (Task 2 gputils source), D6 (Task 1), D7 (Task 6 Windows slice), D8 (explicitly out of scope). Migration checklist: MSP430 spike (Task 2 Step 5), gputils + cross-checks (Task 2 Step 3, Task 3 Step 4), driver discovery + `[[bin]]` (Task 1), release.yml + smoke (Task 6), flake retirement (Task 7), docs/ADR (Task 7).
- **Type consistency:** `resolve_clang` signature is defined once (Task 1) and consumed only by `main.rs`; `CARGO_BIN_EXE_epic-cc` is used consistently across all 22 test files; the `EPIC_CC_VERSION` build arg flows from Task 4's Dockerfile into Task 6's workflow; the GHCR cache ref `ghcr.io/<repo>-toolchain` is identical in Tasks 5 and 6.
- **Placeholder scan:** every step carries concrete code or commands; the only conditional is the documented MSP430 fallback (Task 2 Step 5), which has an explicit decision procedure.
