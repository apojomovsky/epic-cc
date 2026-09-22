#!/bin/sh
# xc8-size-compare.sh: per-bench epic-cc size plus the exact XC8 command.
# XC8 needs the licence-gated oracle image, so the XC8 column is filled
# by running the printed command, never by this script. Usage:
#   sh scripts/xc8-size-compare.sh
set -eu
dir="crates/driver/tests/fixtures/size-bench"
cargo build -q -p driver --bin epic-cc
target_dir="${CARGO_TARGET_DIR:-target}"
bin=$(find "$target_dir" -maxdepth 3 -path "*/debug/epic-cc" -type f | head -n 1)
test -n "$bin" || { echo "xc8-size-compare: built epic-cc not found under $target_dir" >&2; exit 1; }
echo "| bench | epic-cc flash | XC8 flash |"
echo "|---|---|---|"
for f in "$dir"/bench-*.c; do
    stem=$(basename "$f" .c)
    "$bin" --target 18F4550 -o /tmp/xc8cmp.hex "$f" 2>/tmp/xc8cmp.rep || {
        echo "| $stem | BUILD-FAIL | |"
        continue
    }
    words=$(awk '/flash:/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9]+\/[0-9]+$/) {split($i,a,"/"); print a[1]; exit}}' /tmp/xc8cmp.rep)
    echo "| $stem | $words | |"
    echo "  XC8: make oracle-exec CMD='xc8-cc -mcpu=18f4550 -O2 $f -o /tmp/$stem.hex'"
done
