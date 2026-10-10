# Docker-first dev entry point for epic-cc. Everything (build, test,
# compile, release, dev shell) runs inside the dev image; ci is an
# alias of it. Nobody needs rustup, clang, or gpasm on the host.
#
# --user + passwd/group mounts keep files you write host-owned; the
# image's own default user (UID/GID build args, set by make image)
# backs that up for any docker run bypassing --user. Cargo caches
# live under ~/.cache/, so the host's own target/ is never touched.

# Content-addressed dev tag (epic-cc#736): the Dockerfile plus uid/gid
# hash rides in the tag, so worktrees with different Dockerfiles build
# distinct images. scripts/dev-image-tag.sh is the single definition.
LOCAL_IMAGE := $(shell bash $(dir $(lastword $(MAKEFILE_LIST)))scripts/dev-image-tag.sh)
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
# worktree (WT_KEY) restores the distinction. check-warnings shares it:
# same command, flags and profile as every other build, so there is no
# separate cache to poison, and cargo replays cached warnings on a fresh
# build, so the grep still fires when the units are already compiled (#889).
WT_KEY := $(subst /,-,$(CURDIR))
TARGET_CACHE := $(CACHE_DIR)/target$(WT_KEY)
FILE        ?= crates/driver/tests/fixtures/add.c
TARGET      ?= p16f877a

# The driver stamps its commit into `epic-cc --version` (epic-hal#240, #260).
# Inside the container a worktree's `.git` is a gitfile holding an absolute
# host path outside the mount, so `git rev-parse` fails there and the stamp
# would lose its sha on exactly the build the consumer is told to run.
# Resolve it on the host, where that path is valid, and pass it in; in the
# main checkout this is belt and braces, since `.git` is a real mount.
EPIC_CC_GIT_SHA := $(shell git rev-parse --short HEAD 2>/dev/null)

# Shared container invocation, so every docker entry point on the dev image
# (exec, test, compile, shell, check-warnings) carries the same mounts and the
# EPIC_CC_GIT_SHA stamp. Each bespoke copy was a place to forget one: `shell`
# lost the sha (#543), and `check-warnings` lost it too. TARGET_CACHE_MOUNT
# stays overridable for one-off probes, but no recipe overrides it: every
# entry point shares the worktree target dir.
TARGET_CACHE_MOUNT ?= $(TARGET_CACHE)

# Recursively expanded (=), not immediate, so a TARGET_CACHE_MOUNT override
# passed on the command line is still in effect when a recipe expands
# DOCKER_RUN. With := the value is baked in at parse time and the override
# would silently build into the default target dir instead.
# One lock per target cache dir: two builds from the same CURDIR (the
# main clone under concurrent agents) share one cache dir, and cargo
# does not serialize across containers, so the second build used to
# read half-written fingerprints. The lock turns that into a queue.
WITH_TARGET_LOCK := bash $(dir $(lastword $(MAKEFILE_LIST)))scripts/with-target-lock.sh
GUARD_MAIN_CLONE := bash $(dir $(lastword $(MAKEFILE_LIST)))scripts/check-main-clone-build.sh
# Docker forwards no host env beyond EPIC_CC_GIT_SHA, so a host-prefixed
# SIZE/CYCLE_BASELINE_ONLY or UPDATE_* switch would vanish: the scope filter
# is then dropped and the update rewrites every row. `-e NAME` forwards the
# value when set, even if empty, matching native cargo.
DOCKER_ARGS = --rm \
	--user $$(id -u):$$(id -g) \
	-v /etc/passwd:/etc/passwd:ro -v /etc/group:/etc/group:ro \
	-v $(CARGO_HOME_CACHE):/opt/cargo-home -e CARGO_HOME=/opt/cargo-home \
	-v $(TARGET_CACHE_MOUNT):/tmp/cargo-target -e CARGO_TARGET_DIR=/tmp/cargo-target \
	-e "EPIC_CC_GIT_SHA=$(EPIC_CC_GIT_SHA)" \
	-e SIZE_BASELINE_ONLY -e CYCLE_BASELINE_ONLY \
	-e UPDATE_SIZE_BASELINE -e UPDATE_CYCLE_BASELINE \
	-v $(CURDIR):/workspace -w /workspace

DOCKER_RUN = mkdir -p $(CARGO_HOME_CACHE) $(TARGET_CACHE_MOUNT) && $(WITH_TARGET_LOCK) $(TARGET_CACHE_MOUNT)/.build.lock docker run $(DOCKER_ARGS) $(LOCAL_IMAGE)

.PHONY: help bootstrap doctor image shell exec test compile info release-bundle clean-containers setup-hooks fmt lint check-warnings pre-pr-check size-report guard-main-clone

bootstrap: ## First-time setup: host deps, git hooks, dev image
	@bash scripts/bootstrap.sh

guard-main-clone: ## Refuse main-clone builds while agents hold worktrees
	@$(GUARD_MAIN_CLONE)

doctor: ## Report what first-time setup is missing, change nothing
	@bash scripts/bootstrap.sh --check-only

help: ## List targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

image: ## Build the dev image (only image you need locally)
	@$(ENSURE_BUILDER)
	@img="$(LOCAL_IMAGE)"; \
	test -n "$$img" || { echo "image: empty dev tag" >&2; exit 1; }; \
	if docker image inspect "$$img" >/dev/null 2>&1; then \
		echo "dev image up to date ($$img), skipping rebuild"; \
	else \
		src_hash=$${img##*-}; \
		docker buildx build --builder $(BUILDER) --load --target dev $(TOOLCHAIN_CACHE) \
			--build-arg UID=$$(id -u) --build-arg GID=$$(id -g) \
			--label org.epic-cc.source-hash=$$src_hash -t "$$img" .; \
	fi

shell: guard-main-clone image ## Interactive dev shell inside the container
	@mkdir -p $(CARGO_HOME_CACHE) $(TARGET_CACHE_MOUNT)
	@$(WITH_TARGET_LOCK) $(TARGET_CACHE_MOUNT)/.build.lock docker run -it $(DOCKER_ARGS) $(LOCAL_IMAGE) bash

exec: guard-main-clone image ## One-off command: make exec CMD='cargo test -p asm'
	@$(DOCKER_RUN) bash -c '$(CMD)'

mdb-oracle: ## MDB execution oracle: make mdb-oracle SPEC=<name> [TIER=pr|nightly]
	@bash scripts/mdb-oracle.sh --spec $(SPEC) --tier $(or $(TIER),pr)

test: guard-main-clone image ## Full suite (ci-test.sh, what CI runs); CRATE=asm scopes to one
	@$(DOCKER_RUN) bash -c '$(if $(CRATE),cargo test -p $(CRATE) --no-fail-fast,bash scripts/ci-test.sh)'
 # Menu-demo ladder inputs. Must match the hal-pic18-menu-demo-18f4550 case
 # in crates/driver/tests/size_regression_e2e.rs; size-report.py fails the
 # render if this list drifts from the sizes.json inputs it records.
 # Shared files live once in hal-pic18-base; the demo dir holds only its own
 # sources plus its config.
 MENU_DEMO_BASE := crates/driver/tests/fixtures/vendor/hal-pic18-base
 MENU_DEMO_FIX := crates/driver/tests/fixtures/vendor/hal-pic18-menu-demo
 MENU_DEMO_INCLUDES := \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/include/epiccc \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/include \
	$(MENU_DEMO_BASE)/epic-common/include \
	$(MENU_DEMO_BASE)/epic-taskmgr/include \
	$(MENU_DEMO_BASE)/epic-tick/include \
	$(MENU_DEMO_FIX)/epic-lcd/include \
	$(MENU_DEMO_BASE)/epic-serial/include \
	$(MENU_DEMO_FIX)/epic-menu-demo/include
 MENU_DEMO_DEFINES := PIC18F4550 FOSC_HZ=48000000 __EPIC_CC__
 MENU_DEMO_SRCS := \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_gpio.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer0.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_timer2.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_usart.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_adc.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_ccp.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/peripherals/pic18fxx5x_eeprom.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/core/pic18_irq.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/core/pic18fxx5x_wdt_sleep.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/epiccc/pic18fxx5x_wdt_sleep_epiccc.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/epiccc/pic18_isr_vector.c \
	$(MENU_DEMO_BASE)/pic18fxx5x-hal/src/epiccc/pic18_irq_dispatch_epiccc_tick.c \
	$(MENU_DEMO_FIX)/pic18fxx5x-hal/src/mdb/pic18_harness_mdb.c \
	$(MENU_DEMO_BASE)/epic-taskmgr/src/epic_taskmgr.c \
	$(MENU_DEMO_BASE)/epic-tick/src/epic_tick.c \
	$(MENU_DEMO_FIX)/epic-lcd/src/epic_lcd.c \
	$(MENU_DEMO_BASE)/epic-serial/src/epic_serial.c \
	$(MENU_DEMO_FIX)/epic-menu-demo/src/menu_demo_core.c \
	$(MENU_DEMO_FIX)/epic-lcd/src/epic_lcd_gpio4.c \
	$(MENU_DEMO_FIX)/epic-menu-demo/tests/sim_menu_demo.c \
	$(MENU_DEMO_FIX)/config_18F4550.c
REPORT_DATE := $(shell date -u +%F)

size-report: guard-main-clone image ## Regenerate crates/driver/tests/fixtures/SIZE_REPORT.md from the tree
	@mkdir -p scratch/size-report
	@$(DOCKER_RUN) bash -c 'set -o pipefail; SIZE_REPORT_JSON=1 cargo test -q -p driver --test size_regression_e2e -- --nocapture 2>/dev/null | sed -n /SIZE_REPORT_JSON_BEGIN/,/SIZE_REPORT_JSON_END/p | grep -v SIZE_REPORT_JSON > scratch/size-report/sizes.json'
	@$(DOCKER_RUN) bash -c 'cargo run -q -p driver -- --target 18F4550 $(addprefix -I ,$(MENU_DEMO_INCLUDES)) $(addprefix -D ,$(MENU_DEMO_DEFINES)) --emit asm --save-temps scratch/size-report/temps -o scratch/size-report/menu-demo.asm $(MENU_DEMO_SRCS) && printf "%s\n" $(MENU_DEMO_SRCS) | sort > scratch/size-report/asm-inputs.txt && python3 scripts/inline-map.py scratch/size-report/temps/merged.ll scratch/size-report/temps/merged_opt.ll > scratch/size-report/inline.json 2>scratch/size-report/inline.err && python3 scripts/density-profile.py scratch/size-report/menu-demo.asm --inline-map scratch/size-report/inline.json --json > scratch/size-report/density.json'
	@python3 scripts/size-report.py --sizes scratch/size-report/sizes.json --density scratch/size-report/density.json --baseline crates/driver/tests/fixtures/size_baseline.toml --sha $(EPIC_CC_GIT_SHA) --date $(REPORT_DATE) --asm-inputs scratch/size-report/asm-inputs.txt --out crates/driver/tests/fixtures/SIZE_REPORT.md && echo "wrote crates/driver/tests/fixtures/SIZE_REPORT.md"

ci-local: guard-main-clone image ## EXACT CI, locally: docker epic-cc-ci bash scripts/ci-test.sh, run before git push (see #99)
	@$(DOCKER_RUN) bash scripts/ci-test.sh

compile: guard-main-clone image ## Compile C to HEX and print it: FILE=x.c TARGET=p16f887
	@$(DOCKER_RUN) bash -c 'cargo run -q -p driver -- $(FILE) -o /tmp/out.hex --target $(TARGET) && cat /tmp/out.hex'

info: guard-main-clone image ## Toolchain versions + env vars from the image
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

fmt: guard-main-clone image ## Format the workspace (cargo fmt)
	@$(DOCKER_RUN) bash -c 'cargo fmt'

lint: guard-main-clone image ## Clippy, advisory (never fails the build)
	@$(DOCKER_RUN) bash -c 'cargo clippy --workspace 2>&1 | tail -20'

fuzz: guard-main-clone ## Lane B (#286): coverage-guided fuzz of irparse's parser (needs nightly + cargo-fuzz)
	@$(DOCKER_RUN) bash -c 'cargo install cargo-fuzz --version 0.12.0 2>&1 | tail -1 && cargo fuzz run irparse_parse_ll -- -max_total_time=60'

check-warnings: guard-main-clone image ## Fail if cargo build --workspace --all-targets emits any warnings
	@$(DOCKER_RUN) bash -c '\
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

sanity: guard-main-clone image ## Per-device lightweight: DEVICE=p16f877a (spec 2026-08-22 section 8)
	@test -n "$(DEVICE)" || (echo "usage: make sanity DEVICE=<stem>  e.g. make sanity DEVICE=p16f887" >&2; exit 2)
	@$(DOCKER_RUN) bash scripts/sanity.sh $(DEVICE)

sanity-all: guard-main-clone image ## Nightly lightweight for every device in crates/device/devices/*.toml
	@$(DOCKER_RUN) bash -c 'for f in crates/device/devices/*.toml; do s=$$(basename $$f .toml); echo "=== sanity $$s ==="; bash scripts/sanity.sh $$s || exit 1; done'

sanity-changed: guard-main-clone image ## PR lightweight for TOMLs touched vs origin/master
	@$(DOCKER_RUN) bash -c '\
		git fetch origin master --quiet || true; \
		changed=$$(git diff --name-only origin/master...HEAD -- crates/device/devices/*.toml 2>/dev/null || true); \
		if [ -z "$$changed" ]; then echo "sanity-changed: no device TOML touched vs origin/master"; exit 0; fi; \
		for f in $$changed; do s=$$(basename $$f .toml); echo "=== sanity-changed $$s ==="; bash scripts/sanity.sh $$s || exit 1; done'

pre-pr-check: ## Takeoff ritual before opening a PR; TEST=1 runs the suite
	@bash scripts/pre-pr-check.sh $(if $(TEST),--test,)
