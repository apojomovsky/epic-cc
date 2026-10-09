#!/usr/bin/env bash
# One-time (idempotent) dev-environment setup for epic-cc. Everything
# runs inside the docker dev image, so the host needs only docker, make
# and git: no rustup, clang, or gpasm are ever installed on the host
# (AGENTS.md). This checks those host deps, installs the git hooks,
# ensures the ssh push keepalive (#994), and builds the dev image.
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

# ---- ssh keepalive for long pre-push hooks ----
# git connects to learn remote refs before running the pre-push hook, so
# the minutes-long make ci-local run idles the ssh connection until the
# server kills it and the push dies after a green hook (#994). A
# Host github.com keepalive fixes it. The marker block below is appended
# once and existing entries are never edited. A config with an active
# ServerAliveInterval covering github.com (global or matching Host
# stanza, not a comment or another host) is left alone.
keepalive_begin="# BEGIN epic-cc push keepalive (#994)"
keepalive_end="# END epic-cc push keepalive"
ssh_config="${HOME}/.ssh/config"
keepalive_wanted=0
if grep -qF "$keepalive_begin" "$ssh_config" 2>/dev/null; then
    echo "bootstrap: ssh push keepalive already installed."
# Active ServerAliveInterval lines only: global ones, or a Host stanza
# matching github.com. Comments and other hosts do not count.
elif [ -f "$ssh_config" ] && awk '
    /^[ \t]*#/ || /^[ \t]*$/ { next }
    { line = $0; sub(/[ \t]+#.*$/, "", line); n = split(line, w) }
    tolower(w[1]) == "host" {
        scope = 0
        for (i = 2; i <= n; i++) { if (tolower(w[i]) == "*" || index(tolower(w[i]), "github") > 0) scope = 1 }
        seen_host = 1; next
    }
    tolower(w[1]) == "serveraliveinterval" && (!seen_host || scope) { found = 1; exit 0 }
    END { exit !found }
' "$ssh_config"; then
    echo "bootstrap: ssh config already keeps github.com alive, leaving it alone."
else
    keepalive_wanted=1
    if [ "$check_only" = 1 ]; then
        echo "bootstrap: ssh push keepalive not installed (run ./scripts/bootstrap.sh)."
        problems=1
    fi
fi
if [ "$keepalive_wanted" = 1 ] && [ "$check_only" = 0 ]; then
    mkdir -p "${HOME}/.ssh"
    chmod 700 "${HOME}/.ssh"
    touch "$ssh_config"
    chmod 600 "$ssh_config"
    if [ -s "$ssh_config" ] && [ -n "$(tail -c 1 "$ssh_config")" ]; then
        printf '\n' >> "$ssh_config"
    fi
    {
        echo "$keepalive_begin"
        echo "Host github.com"
        echo "    ServerAliveInterval 15"
        echo "    ServerAliveCountMax 40"
        echo "$keepalive_end"
    } >> "$ssh_config"
    echo "bootstrap: ssh push keepalive installed ($ssh_config)."
fi

# ---- docker dev image ----
# Content-addressed tag (epic-cc#736) via the single script, same tag
# as the Makefile's LOCAL_IMAGE. First build compiles clang, so slow.
dev_image="$(bash "$repo_root/scripts/dev-image-tag.sh" 2>/dev/null || true)"
if [ "$check_only" = 1 ]; then
    if [ -n "$dev_image" ] && docker image inspect "$dev_image" >/dev/null 2>&1; then
        echo "bootstrap: docker dev image present ($dev_image)."
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
