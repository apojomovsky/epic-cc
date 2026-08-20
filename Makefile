# Docker-first dev entry point for epic-cc. The dev image (ci is an empty
# alias of it, so running in dev IS running in ci) hosts every target:
# build, test, compile, release bundles, and a dev shell. Nobody needs
# rustup, clang, or gpasm installed on the host.
#
# Container plumbing (why it is the way it is):
#   --user + passwd/group mounts   files written into the workspace stay
#                                  host-owned (root-owned files break host
#                                  `rm -rf`)
#   cargo caches under ~/.cache/   registry + target are persisted across
#                                  container runs (ephemeral containers
#                                  would rebuild the whole workspace each
#                                  time); CARGO_TARGET_DIR points there so
#                                  the host's own target/ is never touched

LOCAL_IMAGE := epic-cc-dev:local
CACHE_DIR   := $(HOME)/.cache/epic-cc
CARGO_HOME_CACHE := $(CACHE_DIR)/cargo-home
TARGET_CACHE := $(CACHE_DIR)/target
FILE        ?= crates/driver/tests/fixtures/add.c

DOCKER_RUN := mkdir -p $(CARGO_HOME_CACHE) $(TARGET_CACHE) && docker run --rm \
	--user $$(id -u):$$(id -g) \
	-v /etc/passwd:/etc/passwd:ro -v /etc/group:/etc/group:ro \
	-v $(CARGO_HOME_CACHE):/opt/cargo-home -e CARGO_HOME=/opt/cargo-home \
	-v $(TARGET_CACHE):/tmp/cargo-target -e CARGO_TARGET_DIR=/tmp/cargo-target \
	-v $(CURDIR):/workspace -w /workspace $(LOCAL_IMAGE)

.PHONY: help image shell exec test compile info release-bundle clean-containers setup-hooks fmt lint pre-pr-check

help: ## List targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-16s %s\n", $$1, $$2}'

image: ## Build the dev image (only image you need locally)
	docker build --target dev -t $(LOCAL_IMAGE) .

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

compile: image ## Compile C to HEX and print it: FILE=crates/driver/tests/fixtures/add.c
	@$(DOCKER_RUN) bash -c 'cargo run -q -p driver -- $(FILE) /tmp/out.hex && cat /tmp/out.hex'
info: image ## Toolchain versions + env vars from the image
	@$(DOCKER_RUN) bash -c 'rustc --version && $$PIC8_CLANG_UNWRAPPED --version | head -1 && gpasm --version | head -1 && echo && env | grep ^PIC8_ | sort'

release-bundle: ## Build the Linux release zip: VERSION=0.1.0
	@test -n "$(VERSION)" || (echo "make release-bundle VERSION=x.y.z"; exit 1)
	docker build --target release --build-arg EPIC_CC_VERSION=$(VERSION) \
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

setup-hooks: ## Install git hooks (.githooks/ -> the repo's hooks dir)
	@mkdir -p $$(git rev-parse --git-path hooks) \
		&& cp .githooks/pre-commit .githooks/commit-msg $$(git rev-parse --git-path hooks)/ \
		&& chmod +x $$(git rev-parse --git-path hooks)/pre-commit $$(git rev-parse --git-path hooks)/commit-msg
	@echo "git hooks installed (pre-commit, commit-msg)"

pre-pr-check: ## Takeoff ritual before opening a PR; TEST=1 also runs the suite
	@bash scripts/pre-pr-check.sh $(if $(TEST),--test,)
