# Contributing to epic-cc

## Before you push

Run the **exact** CI locally via Docker — it *is* CI:

```bash
make ci-local
```

This runs `docker epic-cc-ci:latest bash scripts/ci-test.sh` (see `scripts/ci-test.sh` and `.github/workflows/ci.yml`). `cargo test -- --skip …` or `make test  # docker` is **not** a substitute — it skips `pic14e_firewall`, `gpasm`, and other Docker-only checks that broke PR 88.

`make ci-local` is installed as a `pre-push` hook via `make setup-hooks` (`.githooks/pre-push`). If you bypass hooks (`git push --no-verify`), you must still run `make ci-local` manually.

## Why

PR 88 (`feat/58-epic-cc-backend` for HAL-2) failed repeatedly on CI while appearing green locally via `make test  # docker -- --skip gpasm` — the local reproduction was not 1-to-1 with remote. See #99.

## Pic14e firewall

`crates/asm/tests/pic14e_firewall.rs` now asserts that `Pic14e` (`p16f193x` etc.) **assembles as Pic14** for the HAL-2 demo (`Pic14e => assemble(src)` in `crates/asm/src/lib.rs:482`), not that it panics. The old firewall (`should_panic(expected = "pic14e")`) was swapped for a silent fallthrough to make the `p16f877a` HAL build succeed — the full Pic14e backend is still TODO (tracked separately).
