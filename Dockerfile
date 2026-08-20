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
        cvise \
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
# All targets (no -DLLVM_TARGETS_TO_BUILD): this clang is also the fuzz
# harness's HOST reference — it compiles the generated program for x86-64
# and runs it (docs/27-phase6-random-testing-plan.md). MSP430-only would
# break the differential tests' host side.
RUN --mount=type=cache,target=/ccache \
    cmake -S /src/llvm/llvm -B /build/llvm -G Ninja \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/opt/clang \
        -DLLVM_ENABLE_PROJECTS=clang \
        -DLLVM_INCLUDE_TESTS=OFF \
        -DLLVM_INCLUDE_BENCHMARKS=OFF \
        -DLLVM_INCLUDE_EXAMPLES=OFF \
        -DLLVM_CCACHE_BUILD=ON \
        -DLLVM_CCACHE_DIR=/ccache \
    && cmake --build /build/llvm --target install -j"$(nproc)" \
    && rm -rf /build/llvm /src/llvm

FROM clang-builder AS dev

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# rustc 1.97.1, pinned in rust-toolchain.toml (kept in sync deliberately).
RUN curl -fsSL https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.97.1 \
    && rustup component add rustfmt clippy rust-src \
    && rm /tmp/rustup.sh

# PIC8_HOST_CLANG: the fuzz differential harness's host-reference compiler
# (crates/fuzz/src/lib.rs host_clang()) — this same clang, no -target,
# compiling and running native x86-64 programs.
ENV PIC8_CLANG_UNWRAPPED=/opt/clang/bin/clang \
    PIC8_CLANG_RESOURCE_DIR=/opt/clang/lib/clang/20 \
    PIC8_HOST_CLANG=/opt/clang/bin/clang \
    PIC8_GPASM=/usr/local/bin/gpasm \
    PIC8_VENDOR_DIR=/workspace/vendor \
    PIC8_XC8_ROOT=/opt/microchip/xc8/v4.00

WORKDIR /workspace

FROM dev AS ci
# ci-test.sh runs from the mounted workspace; nothing extra to install.

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
    && curl -fsSL -o "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/LICENSE.clang.txt" \
        https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-20.1.8/llvm/LICENSE.TXT \
    && echo "8d85c1057d742e597985c7d4e6320b015a9139385cff4cbae06ffc0ebe89afee  /out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux/LICENSE.clang.txt" | sha256sum -c - \
    && cd "/out/epic-cc-${EPIC_CC_VERSION}-x86_64-linux" \
    && ./epic-cc /workspace/crates/driver/tests/fixtures/add.c /tmp/env.hex \
    && env -u PIC8_CLANG_UNWRAPPED -u PIC8_CLANG_RESOURCE_DIR \
       ./epic-cc /workspace/crates/driver/tests/fixtures/add.c /tmp/bundled.hex \
    && cmp /tmp/env.hex /tmp/bundled.hex
