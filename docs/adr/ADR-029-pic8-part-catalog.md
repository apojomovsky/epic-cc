# ADR-029 -- PIC8 flash-generation part catalog: one tracking table, popularity tiers, on-demand ports

**Status:** Accepted 2026-09-09<br>
**Decides:** the user-directed device-catalog request (2026-09-09); companion
data for `docs/38` D-3<br>
**Parent:** `docs/38-device-onboarding-hardening.md` (D-2, D-3), ADR-019,
ADR-020

## Decision

* `crates/device/catalog/parts.toml` carries one `[[part]]` row for every
  8-bit PIC flash-generation part ever shipped: PIC10F, PIC12F, PIC16F and
  PIC18F including the LF and HV orderable variants of the same silicon,
  1007 rows as of generation. The EPROM/OTP C parts are excluded by the same
  name rule that XC8 and gputils use. The table tracks and prioritizes; it
  never becomes compiler input. `devices/*.toml` stays the ported, verified
  set per `docs/38` D-3.
* Columns per row: canonical toolchain `name`, `display` part number, `core`
  (the closed four-core set), `flash_words`, `ram_bytes`, `eeprom_bytes`,
  the covering DFP `pack`, `gh_refs` (GitHub code-search hits for the part
  number, substring matches: a number that prefixes siblings, e.g. PIC16F87
  against PIC16F877A, also counts their mentions, so read such rows as
  family totals), `tier` (`high` >= 5000, `mid` >= 500, `low` >= 1,
  `unscored` when not yet run), `lifecycle` from Microchip's product
  database, `pdf` (direct datasheet URL) and `page` (product page).
* Sources, in precedence per field: memory facts and core from the DFP packs
  via `packs.download.microchip.com` (Apache-2.0, regenerated continuously
  by Microchip), RAM and EEPROM from the gputils `.lkr` oracle (the DFP's
  GPR sectors understate some PIC18s, the same defect `gen-device.py`'s
  header documents), lifecycle and datasheet URLs from Microchip's
  ProductInfo MCP API (`api.microchip.com/mcp/resources`, unauthenticated).
  Every cross-source disagreement is printed at build time, and the catalog
  test pins each shipped device's core and flash against its TOML.
* `scripts/catalog.py` regenerates everything: `scan` (gputils),
  `fetch`/`scan-index`/`scan-dfp` (DFP packs), `scan-mcp` (lifecycle +
  datasheet URLs), `score` (GitHub code search, paced at the API's 10
  req/min and resumable after every query), `build` (the merge). Score runs
  order the classic tutorial parts first so a partial run covers the parts
  that dominate community code.
* Support priority is a query over the table, not a new artifact: highest
  `tier` first, `lifecycle = Active` before NRND ("not recommended for new
  designs") parts, then whichever cores already have backends. Adding a
  part stays `docs/32`/`add-device.sh` regardless of tier.

## Rationale

Prioritizing ports needs a complete candidate list with a reproducible
popularity signal. GitHub code search over the exact part number is the only
community metric that can be computed for a thousand parts from CI; forum
and blog presence cannot. The DFP packs are Microchip's own
machine-readable device database (the same source ADR-020 chose for
per-device TOMLs), so the catalog inherits their coverage of both current
and discontinued parts while committing nothing Microchip-licensed.

## Alternatives rejected

* **Commit a TOML for every part** -- rejected by `docs/38` D-3: hundreds of
  never-verified memory maps are a maintenance surface, not coverage.
* **Score popularity by stock levels or forum mentions** -- stock is a
  distribution artifact that flips weekly and forums cannot be queried
  programmatically at scale. Code search under-counts hobby forums but is
  stable, dated and re-runnable.
* **Derive the catalog at build time from a downloaded DFP** -- makes the
  repo's roadmap table depend on a network fetch and pins CI to Microchip's
  CDN; the committed TOML with a regeneration script matches the ADR-020
  posture.

## Consequences

* The `gh_refs` column is a snapshot, not a live number: scores carry the
  run date in the header and `catalog.py score` resumes where it stopped.
* `crates/device/tests/catalog.rs` gates scope (F-generation only), column
  shape, tier consistency and catalog-versus-registry agreement (core and
  flash) on every device-crate test run. The catalog is not part of
  `build.rs` codegen.
* Adding a row to the catalog is not adding support; support still goes
  through `gen-device.py` + the gputils cross-check + `sanity.sh`.
* `fetch` scriptability rests on the packs being anonymously downloadable
  Apache-2.0 artifacts. `docs/38` D-1's no-automation reasoning assumed
  account-gated downloads and still governs per-device TOML sourcing, not
  this table.

## Revisit if

* Microchip's MCP API gains auth or rate limits that block a full sweep:
  the `pdf`/`lifecycle` columns become optional again, the table stands.
* A popularity signal materially better than code search appears (for
  example, Microchip publishing per-part shipment counts): re-tier, keep
  the column shape.
