# Slice 2 -- CI stratification: canonical per core + changed-device lightweight

**Parent spec:** §8<br>
**Ticket:** `epic-cc#84`<br>
**Depends on:** slice 1

## Goal

Stop CI from scaling as `devices × fixtures`. PRs run canonical full (2 jobs); only touched TOMLs get a lightweight drill.

## Steps

1. `ci.yml` split `canonical` (matrix `p16f877a`/`p18f4550`, full fixtures+sim+fuzz) vs `devices-changed` (bash: `git diff --name-only origin/master -- crates/device/devices/*.toml`, loop).
2. `make sanity DEVICE=<stem>` helper: alloc check + one `add.c → hex → gpasm -p <stem>` round-trip.
3. Nightly `schedule` + `workflow_dispatch` lightweight for all devices.
4. Update `docs/05-verification.md` with changed-device drill.

## Acceptance (from #84)

- PR touching only `p16f887.toml` does not re-run 15 fixtures for `p16f877a`.
- PR with no device diff runs only 2 canonical jobs.
- Nightly runs lightweight for every `devices/*.toml`.
