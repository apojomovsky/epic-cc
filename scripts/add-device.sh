#!/usr/bin/env bash
# One-command device onboarding (docs/internals/design/38 D-1): collapse docs/internals/design/32 §2-§6 into
# a single wrapper. Generates the TOML, cross-checks it against gputils,
# adds the pack SFR/config aliases, runs the per-device sanity, compiles a synthesized EPIC_CONFIG fixture,
# and reports "ready to commit" with a field-diff vs the closest sibling,
# or a failure list naming exactly which step failed. Run inside the dev
# container (make exec / make shell); DFP sourcing stays manual (docs/internals/design/38
# D-1). Usage: bash scripts/add-device.sh <part> --atdf <path> [--pack <name>]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PART=""
ATDF=""
PACK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --atdf) ATDF="$2"; shift 2 ;;
    --pack) PACK="$2"; shift 2 ;;
    -h|--help)
      cat <<HELP
usage: $0 <part> --atdf <path> [--pack <name>]

steps: 1 gen-device.py, 2 gputils RAM correction,
  3 device_names.py (pack SFR and config aliases; skipped without an EDC .PIC),
  4 gputils crosscheck (PIC8_DEVICE_SCOPE=<stem>, incl. SFR headers),
  5 sanity.sh, 6 EPIC_CONFIG fixture, 7 field-diff vs sibling
HELP
      exit 0 ;;
    *) PART="$1"; shift ;;
  esac
done
if [ -z "$PART" ] || [ -z "$ATDF" ]; then
  echo "usage: $0 <part> --atdf <path> [--pack <name>]" >&2
  exit 2
fi

STEM="$(python3 scripts/add_device.py stem "$PART")"
DEVICES="crates/device/devices"
TOML="$DEVICES/$STEM.toml"

TMP="$(mktemp -d .add-device.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

# Back up an existing TOML so a failed run restores the registry.
BACKUP=""
if [ -f "$TOML" ]; then
  BACKUP="$TMP/backup.toml"
  cp "$TOML" "$BACKUP"
fi
restore() {
  if [ -n "$BACKUP" ]; then
    cp "$BACKUP" "$TOML"
  else
    rm -f "$TOML"
  fi
}

step_fail() {
  local step="$1"; shift
  restore
  echo "add-device $STEM: FAILED at step $step" >&2
  echo "  $*" >&2
  exit 1
}

# --- step 1: generate the TOML ---
echo "--- add-device $STEM: gen-device.py ---"
GEN_ARGS=(--atdf "$ATDF")
if [ -n "$PACK" ]; then
  GEN_ARGS+=(--pack "$PACK")
fi
if ! python3 scripts/gen-device.py "$PART" "${GEN_ARGS[@]}" --out "$TOML" 2>"$TMP/gen.err"; then
  step_fail "1 (gen-device.py)" "$(cat "$TMP/gen.err")"
fi

# --- step 2: gputils RAM correction (gputils wins, docs/internals/design/32 §3) ---
LKR="/usr/local/share/gputils/lkr/${STEM#p}_g.lkr"
if [ -f "$LKR" ]; then
  if python3 scripts/add_device.py correct-ram "$TOML" "$LKR"; then
    echo "add-device $STEM: RAM widened to match gputils (DFP understates it); add a citing comment"
  fi
fi

# --- step 3: pack SFR and config aliases (docs/internals/design/32 §5a) ---
# Runs before the crosscheck so sfr_tables_match_gputils_headers sees the
# [[sfrs]] table. Exit 2 is the no-DFP signal: skip, never fail.
echo "--- add-device $STEM: device_names.py ---"
NAMES_ARGS=()
NAMES_SRC="$(python3 scripts/add_device.py names-source "$ATDF")"
if [ -n "$NAMES_SRC" ]; then
  NAMES_ARGS+=(--atdf "$NAMES_SRC")
fi
set +e
python3 scripts/device_names.py "$STEM" ${NAMES_ARGS[@]+"${NAMES_ARGS[@]}"} 2>"$TMP/names.err"
NAMES_RC=$?
set -e
cat "$TMP/names.err" >&2
if [ "$NAMES_RC" -eq 2 ]; then
  echo "add-device $STEM: no EDC .PIC found, SFR aliases skipped; rerun device_names.py with --atdf <pack>/edc/<PART>.PIC"
elif [ "$NAMES_RC" -ne 0 ]; then
  step_fail "3 (device_names.py)" "$(cat "$TMP/names.err")"
fi

# --- step 4: gputils crosscheck (includes the SFR header cross-check) ---
echo "--- add-device $STEM: gputils crosscheck ---"
if ! PIC8_DEVICE_SCOPE="$STEM" cargo test -p device --test gputils_crosscheck 2>&1 | tee "$TMP/crosscheck.out"; then
  step_fail "4 (gputils crosscheck)" "see output above"
fi

# --- step 5: per-device sanity ---
echo "--- add-device $STEM: sanity.sh ---"
if ! bash scripts/sanity.sh "$STEM" 2>&1 | tee "$TMP/sanity.out"; then
  step_fail "5 (sanity.sh)" "see output above"
fi

# --- step 6: synthesize + compile EPIC_CONFIG fixture ---
echo "--- add-device $STEM: EPIC_CONFIG fixture ---"
SPEC="$(python3 scripts/add_device.py synthesize "$TOML")"
XTAL="$(python3 scripts/add_device.py synthesize-xtal "$TOML")"
cat > "$TMP/fixture.c" <<EOF
#include <epic-cc.h>
EPIC_CONFIG("$SPEC, xtal_hz=$XTAL");
int main(void) { return 0; }
EOF
if ! cargo run -q -p driver -- --target "$STEM" "$TMP/fixture.c" -o "$TMP/out.hex" 2>&1 | tee "$TMP/fixture.out"; then
  step_fail "6 (EPIC_CONFIG fixture)" "see output above"
fi

# --- step 7: field-diff vs closest sibling ---
SIB="$(python3 scripts/add_device.py sibling "$TOML" "$DEVICES")"
if [ -n "$SIB" ]; then
  echo "--- add-device $STEM: field-diff vs $SIB ---"
  python3 scripts/add_device.py field-diff "$TOML" "$SIB"
fi

echo "add-device $STEM: ready to commit"
