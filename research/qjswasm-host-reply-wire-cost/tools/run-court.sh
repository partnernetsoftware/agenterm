#!/bin/sh
# Pricing runs: one court, three shapes, three repetitions each.
#
#   sh research/qjswasm-host-reply-wire-cost/tools/run-court.sh <journey> [N] [probe]
#
# Writes runs/court-<journey>-<shape>-<n>[-probe].json. With `probe`, the lane
# binary is run with AGENTERM_QJS_ALLOCATION_PROBE=1 so `cost.json_parse_bytes`
# is reported; a probe compiles a different guest, so those steps are reporters
# only unless they reproduce the non-probe median.
set -eu
ROOT=$(pwd)
LANE="$ROOT/target/frontier-host-reply-wire"
BIN="$LANE/debug/agenterm"
HERE="$ROOT/research/qjswasm-host-reply-wire-cost"
OUT="$HERE/runs"
mkdir -p "$OUT"

JOURNEY=$1
N=${2:-3}
PROBE=${3:-}
SUFFIX=""
if [ "$PROBE" = "probe" ]; then SUFFIX="-probe"; fi

for shape in A B C; do
  n=1
  while [ "$n" -le "$N" ]; do
    file="$OUT/court-$JOURNEY-$shape-$n$SUFFIX.json"
    if [ "$PROBE" = "probe" ]; then
      AGENTERM_QJS_ALLOCATION_PROBE=1 "$BIN" cli script run --profile tool --json \
        --project-root scripts/qjs --timeout-ms 300000 --max-operations 1000000000 \
        research/qjswasm-host-reply-wire-cost/court.qjs \
        -- "$HERE/shapes" "$JOURNEY" "$shape" > "$file" 2>&1 || true
    else
      "$BIN" cli script run --profile tool --json \
        --project-root scripts/qjs --timeout-ms 300000 --max-operations 1000000000 \
        research/qjswasm-host-reply-wire-cost/court.qjs \
        -- "$HERE/shapes" "$JOURNEY" "$shape" > "$file" 2>&1 || true
    fi
    echo "ran court $JOURNEY $shape $n$SUFFIX -> $file"
    n=$((n + 1))
  done
done
