# Docker-first dev entry point for epic-cc. Everything (build, test,
# compile, release, dev shell) runs inside the dev image; ci is an
# alias of it. Nobody needs rustup, clang, or gpasm on the host.
#
# --user + passwd/group mounts keep files you write host-owned; the
# image's own default user (UID/GID build args, set by make image)
# backs that up for any docker run bypassing --user. Cargo caches
# live under ~/.cache/, so the host's own target/ is never touched.

LOCAL_IMAGE := epic-cc-dev:local
CACHE_DIR   := $(HOME)/.cache/epic-cc
CARGO_HOME_CACHE := $(CACHE_DIR)/cargo-home

# Dedicated buildx builder (docker-container driver) for every local image
# build. Its cache, including the clang-builder layer and the LLVM ccache
# mount, lives in this builder's own container/volume, not the default
# docker driver's storage. `docker system prune` / `docker builder prune`
# (no --builder flag) only ever touch the default builder, so a generic
# cache cleanup can no longer reach the clang cache at all. That is the
# fix, not the registry cache below, which is just the fallback for a
# fresh machine or someone deleting this builder by name.
BUILDER := epic-cc-builder
ENSURE_BUILDER := docker buildx inspect $(BUILDER) >/dev/null 2>&1 || \
	docker buildx create --name $(BUILDER) --driver docker-container --bootstrap >/dev/null

# Same registry cache CI warms (.github/workflows/toolchain-cache.yml).
# Fallback path only: a fresh machine (no local builder cache yet) pulls
# the already-built clang layer from GHCR instead of compiling it. Public
# image, no login needed; ignore-error=true so a GHCR outage just falls
# back to a normal (uncached) build instead of failing it.
TOOLCHAIN_CACHE := --cache-from type=registry,ref=ghcr.io/apojomovsky/epic-cc-toolchain,ignore-error=true

# Every docker invocation mounts its worktree at the identical in-container
# path (/workspace), so a shared target dir lets cargo silently replay a
# DIFFERENT worktree's cached artifacts here: fingerprints key on the
# absolute path, which is constant across worktrees. One target dir per
# worktree (WT_KEY) restores the distinction. check-warnings used to be the
# only target isolated this way; now every target is.
WT_KEY := $(subst /,-,$(CURDIR))
TARGET_CACHE := $(CACHE_DIR)/target$(WT_KEY)
WARNCHECK_TARGET_CACHE := $(CACHE_DIR)/target-warncheck$(WT_KEY)
FILE        ?= crates/driver/tests/fixtures/add.c
TARGET      ?= p16f877a

DOCKER_RUN := mkdir -p $(CARGO_HOME_CACHE) $(TARGET_CACHE) && docker run --rm \
	--user $$(id -u):$$(id -g) \
	-v /etc/passwd:/etc/passwd:ro -v /etc/group:/etc/group:ro \
	-v $(CARGO_HOME_CACHE):/opt/cargo-home -e CARGO_HOME=/opt/cargo-home \
	-v $(TARGET_CACHE):/tmp/cargo-target -e CARGO_TARGET_DIR=/tmp/cargo-target \
	-v $(CURDIR):/workspace -w /workspace $(LOCAL_IMAGE)

.PHONY: help bootstrap doctor image shell exec test compile info release-bundle clean-containers setup-hooks fmt lint check-warnings pre-pr-check

bootstrap: ## First-time setup: host deps, git hooks, dev image
	@bash scripts/bootstrap.sh

doctor: ## Report what first-time setup is missing, change nothing
	@bash scripts/bootstrap.sh --check-only

help: ## List targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

image: ## Build the dev image (only image you need locally)
	@$(ENSURE_BUILDER)
	docker buildx build --builder $(BUILDER) --load --target dev $(TOOLCHAIN_CACHE) --build-arg UID=$$(id -u) --build-arg GID=$$(id -g) -t $(LOCAL_IMAGE) .

shell: image ## Interactive dev shell inside the container
	@mkdir -p $(CARGO_HOME_CACHE) $(TARGET_CACHE)
	docker run --rm -it --user $$(id -u):$$(id -g) \
		-v /etc/passwd:/etc/passwd:ro -v /etc/group:/etc/group:ro \
		-v $(CARGO_HOME_CACHE):/opt/cargo-home -e CARGO_HOME=/opt/cargo-home \
		-v $(TARGET_CACHE):/tmp/cargo-target -e CARGO_TARGET_DIR=/tmp/cargo-target \
		-v $(CURDIR):/workspace -w /workspace $(LOCAL_IMAGE) bash

exec: image ## One-off command: make exec CMD='cargo test -p asm'
	@$(DOCKER_RUN) bash -c '$(CMD)'

test: image ## Full suite (ci-test.sh, what CI runs); CRATE=asm scopes to one
	@$(DOCKER_RUN) bash -c '$(if $(CRATE),cargo test -p $(CRATE) --no-fail-fast,bash scripts/ci-test.sh)'

ci-local: image ## EXACT CI, locally: docker epic-cc-ci bash scripts/ci-test.sh, run before git push (see #99)
	@$(DOCKER_RUN) bash scripts/ci-test.sh

compile: image ## Compile C to HEX and print it: FILE=x.c TARGET=p16f887
	@$(DOCKER_RUN) bash -c 'cargo run -q -p driver -- $(FILE) -o /tmp/out.hex --target $(TARGET) && cat /tmp/out.hex'

info: image ## Toolchain versions + env vars from the image
	@$(DOCKER_RUN) bash -c 'rustc --version && $$PIC8_CLANG_UNWRAPPED --version | head -1 && gpasm --version | head -1 && echo && env | grep ^PIC8_ | sort'

release-bundle: ## Build the Linux release zip: VERSION=0.1.0
	@test -n "$(VERSION)" || (echo "make release-bundle VERSION=x.y.z"; exit 1)
	@$(ENSURE_BUILDER)
	docker buildx build --builder $(BUILDER) --load --target release $(TOOLCHAIN_CACHE) --build-arg EPIC_CC_VERSION=$(VERSION) \
		-t epic-cc-release:$(VERSION) .
	@mkdir -p dist
	docker run --rm --user $$(id -u):$$(id -g) \
		-v $(CURDIR)/dist:/out-dist epic-cc-release:$(VERSION) \
		bash -c 'cp -r /out/epic-cc-$(VERSION)-x86_64-linux /out-dist/'
	(cd dist && zip -qr ../epic-cc-$(VERSION)-x86_64-linux.zip epic-cc-$(VERSION)-x86_64-linux)
	@echo "built dist/epic-cc-$(VERSION)-x86_64-linux.zip"

clean-containers: ## Remove leftover containers from interrupted runs
	@docker ps -aq --filter name=epic-cc-bundle | xargs -r docker rm -f
	@echo "cleaned"

fmt: image ## Format the workspace (cargo fmt)
	@$(DOCKER_RUN) bash -c 'cargo fmt'

lint: image ## Clippy, advisory (never fails the build)
	@$(DOCKER_RUN) bash -c 'cargo clippy --workspace 2>&1 | tail -20'

fuzz: ## Lane B (#286): coverage-guided fuzz of irparse's parser (needs nightly + cargo-fuzz)
	@$(DOCKER_RUN) bash -c 'cargo install cargo-fuzz --version 0.12.0 2>&1 | tail -1 && cargo fuzz run irparse_parse_ll -- -max_total_time=60'

check-warnings: image ## Fail if cargo build --workspace --all-targets emits any warnings
	@mkdir -p $(CARGO_HOME_CACHE) $(WARNCHECK_TARGET_CACHE)
	@docker run --rm --user $$(id -u):$$(id -g) \
		-v /etc/passwd:/etc/passwd:ro -v /etc/group:/etc/group:ro \
		-v $(CARGO_HOME_CACHE):/opt/cargo-home -e CARGO_HOME=/opt/cargo-home \
		-v $(WARNCHECK_TARGET_CACHE):/tmp/cargo-target -e CARGO_TARGET_DIR=/tmp/cargo-target \
		-v $(CURDIR):/workspace -w /workspace $(LOCAL_IMAGE) bash -c '\
		out=$$(cargo build --workspace --all-targets 2>&1); \
		warnings=$$(printf "%s\n" "$$out" | grep "^warning:" || true); \
		if [ -n "$$warnings" ]; then \
			printf "%s\n" "$$out"; \
			echo; \
			echo "check-warnings: compiler warnings present (above); fix before merging"; \
			exit 1; \
		fi'

setup-hooks: ## Install git hooks (.githooks/ -> the repo's hooks dir)
	@mkdir -p $$(git rev-parse --git-path hooks) \
		&& cp .githooks/pre-commit .githooks/commit-msg .githooks/pre-push $$(git rev-parse --git-path hooks)/ \
		&& chmod +x $$(git rev-parse --git-path hooks)/pre-commit $$(git rev-parse --git-path hooks)/commit-msg $$(git rev-parse --git-path hooks)/pre-push
	@echo "git hooks installed (pre-commit, commit-msg, pre-push)"

sanity: image ## Per-device lightweight: DEVICE=p16f877a (spec 2026-08-22 section 8)
	@test -n "$(DEVICE)" || (echo "usage: make sanity DEVICE=<stem>  e.g. make sanity DEVICE=p16f887" >&2; exit 2)
	@$(DOCKER_RUN) bash scripts/sanity.sh $(DEVICE)

sanity-all: image ## Nightly lightweight for every device in crates/device/devices/*.toml
	@$(DOCKER_RUN) bash -c 'for f in crates/device/devices/*.toml; do s=$$(basename $$f .toml); echo "=== sanity $$s ==="; bash scripts/sanity.sh $$s || exit 1; done'

sanity-changed: image ## PR lightweight for TOMLs touched vs origin/master
	@$(DOCKER_RUN) bash -c '\
		git fetch origin master --quiet || true; \
		changed=$$(git diff --name-only origin/master...HEAD -- crates/device/devices/*.toml 2>/dev/null || true); \
		if [ -z "$$changed" ]; then echo "sanity-changed: no device TOML touched vs origin/master"; exit 0; fi; \
		for f in $$changed; do s=$$(basename $$f .toml); echo "=== sanity-changed $$s ==="; bash scripts/sanity.sh $$s || exit 1; done'

pre-pr-check: ## Takeoff ritual before opening a PR; TEST=1 runs the suite
	@bash scripts/pre-pr-check.sh $(if $(TEST),--test,)
