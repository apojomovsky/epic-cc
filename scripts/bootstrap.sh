#!/usr/bin/env bash
# One-time (idempotent) dev-environment setup for epic-cc. Everything
# runs inside the docker dev image, so the host needs only docker, make
# and git: no rustup, clang, or gpasm are ever installed on the host
# (AGENTS.md). This checks those host deps, installs the git hooks, and
# builds the dev image.
#
#   ./scripts/bootstrap.sh [--check-only]   --check-only: report only, exit
#                                            nonzero if anything is missing

set -euo pipefail

check_only=0
[ "${1:-}" = "--check-only" ] && check_only=1

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
problems=0

# ---- host dependencies ----
# docker is the only hard requirement: every build, test and shell runs
# in the dev image. make and git are needed to drive the Makefile and
# the hooks.
if ! command -v docker >/dev/null 2>&1; then
    echo "bootstrap: docker not found. Install Docker first:" >&2
    echo "  https://docs.docker.com/engine/install/" >&2
    echo "  (Debian/Ubuntu: sudo apt-get install docker.io, then add your" >&2
    echo "   user to the docker group and re-login)" >&2
    problems=1
elif ! docker info >/dev/null 2>&1; then
    echo "bootstrap: docker found but the daemon is not reachable (is it" >&2
    echo "  running? are you in the docker group?)." >&2
    problems=1
fi

if ! command -v make >/dev/null 2>&1; then
    echo "bootstrap: make not found. Install it:" >&2
    echo "  Debian/Ubuntu: sudo apt-get install make" >&2
    echo "  Fedora/RHEL:   sudo dnf install make" >&2
    echo "  Arch:          sudo pacman -S make" >&2
    problems=1
fi

if ! command -v git >/dev/null 2>&1; then
    echo "bootstrap: git not found. Install it:" >&2
    echo "  Debian/Ubuntu: sudo apt-get install git" >&2
    echo "  Fedora/RHEL:   sudo dnf install git" >&2
    echo "  Arch:          sudo pacman -S git" >&2
    problems=1
fi

if [ "$problems" -eq 1 ]; then
    exit 1
fi
echo "bootstrap: host deps present (docker, make, git)."

# ---- git hooks ----
# The hooks dir lives in the common dir, shared by every worktree, so
# this reports the same state from a .worktrees/ checkout as from master.
if [ "$check_only" = 1 ]; then
    hooks_dir="$(cd "$(git -C "$repo_root" rev-parse --git-common-dir)" && pwd)/hooks"
    for hook in pre-commit commit-msg pre-push; do
        if [ -e "$hooks_dir/$hook" ]; then
            echo "bootstrap: $hook hook already installed."
        else
            echo "bootstrap: $hook hook not installed (run 'make setup-hooks')."
            problems=1
        fi
    done
else
    make -C "$repo_root" setup-hooks
fi

# ---- docker dev image ----
# Same tag as the Makefile's LOCAL_IMAGE. Building it compiles clang
# from a digest-pinned tarball, so the first build is slow.
if [ "$check_only" = 1 ]; then
    if docker image inspect epic-cc-dev:local >/dev/null 2>&1; then
        echo "bootstrap: docker dev image present (epic-cc-dev:local)."
    else
        echo "bootstrap: docker dev image not built yet (run ./scripts/bootstrap.sh"
        echo "  or 'make image')."
        problems=1
    fi
else
    make -C "$repo_root" image
fi

if [ "$problems" -eq 1 ]; then
    exit 1
fi
echo "bootstrap: ready. Run 'make test' to verify, or 'make shell' for a dev shell."
exit 0
