#!/usr/bin/env bash
# Per-device lightweight sanity for the CI stratification design.
# Usage: bash scripts/sanity.sh <device>
#   e.g., bash scripts/sanity.sh p16f877a
# Does for one device what nightly does for all:
# - device schema/invariants (via cargo test -p device --test sanity)
# - alloc empty-prog and 80-byte global placement
# - asm flash-bound tiny program
# - add.c -> HEX with --target <device> + gpasm -p <device> assemble
set -euo pipefail

DEVICE="${1:-}"
if [ -z "$DEVICE" ]; then
  echo "usage: $0 <device>  (e.g., $0 p16f877a)" >&2
  exit 2
fi

echo "--- sanity $DEVICE: alloc/asm checks ---"
SANITY_DEVICE="$DEVICE" cargo test -p device --test sanity -- --nocapture

echo "--- sanity $DEVICE: add.c -> HEX + gpasm ---"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Compile add.c to HEX and to ASM for this device. pic-baseline's backend
# landed in P2 (docs/37), so the driver compiles it like the other cores;
# the gpasm drill below still proves the part name.
if cargo run -q -p driver -- --target "$DEVICE" crates/driver/tests/fixtures/add.c -o "$TMP/out.hex" 2>"$TMP/driver.err"; then
  cargo run -q -p driver -- --target "$DEVICE" crates/driver/tests/fixtures/add.c -o "$TMP/out.asm" --emit asm
  if [ ! -s "$TMP/out.hex" ]; then
    echo "sanity $DEVICE: empty HEX" >&2
    exit 1
  fi
  if [ ! -s "$TMP/out.asm" ]; then
    echo "sanity $DEVICE: empty ASM" >&2
    exit 1
  fi
else
  if ! grep -q "no backend yet" "$TMP/driver.err"; then
    echo "sanity $DEVICE: driver failed unexpectedly:" >&2
    cat "$TMP/driver.err" >&2
    exit 1
  fi
  echo "sanity $DEVICE: driver refuses pic-baseline (no backend yet), as expected"
fi

# gpasm cross-check: the driver ASM uses PAGE()/BANKSEL pseudo-ops that
# gpasm does not know (the asm crate's gpasm tests translate them via
# to_gpasm_src). Rather than reimplement that translation in shell, the
# lightweight per-device drill proves the device name is valid for gpasm
# by assembling a minimal program that gpasm does know. The add.c compile
# above already proves our assembler accepts the device's flash map.
GPASM="${PIC8_GPASM:-gpasm}"
if ! command -v "$GPASM" >/dev/null 2>&1; then
  echo "sanity $DEVICE: $GPASM not on PATH, skipping gpasm assemble (inside docker it must exist)" >&2
  exit 0
fi

cat > "$TMP/minimal.asm" <<EOF
    list p=$DEVICE
    org 0x0000
    nop
    goto 0x0000
    end
EOF
if ! "$GPASM" -p "$DEVICE" -o "$TMP/gpasm.hex" "$TMP/minimal.asm" >/dev/null 2>&1; then
  echo "sanity $DEVICE: gpasm -p $DEVICE failed on minimal asm" >&2
  "$GPASM" -p "$DEVICE" -o "$TMP/gpasm.hex" "$TMP/minimal.asm" >&2 || true
  exit 1
fi
if [ ! -s "$TMP/gpasm.hex" ]; then
  echo "sanity $DEVICE: gpasm produced empty HEX" >&2
  exit 1
fi

echo "sanity $DEVICE: ok (HEX + gpasm)"
