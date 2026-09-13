#!/bin/sh
# V1 reply capture: run each journey once through its instrumented reporter copy
# and freeze the replies it received.
#
#   sh research/qjswasm-host-reply-wire-cost/tools/run-capture.sh [OUTDIR]
#
# OUTDIR defaults to research/qjswasm-host-reply-wire-cost/replies. It fails
# closed if a journey's directory already exists: the frozen set is captured
# once and must not be silently overwritten (V1).
set -eu
ROOT=$(pwd)
LANE="$ROOT/target/frontier-host-reply-wire"
BIN="$LANE/debug/agenterm"
HERE="$ROOT/research/qjswasm-host-reply-wire-cost"
OUT=${1:-"$HERE/replies"}

capture() {
  journey=$1
  shift
  dir="$OUT/$journey"
  if [ -e "$dir" ]; then
    echo "refusing to overwrite frozen reply set: $dir" >&2
    exit 1
  fi
  mkdir -p "$dir"
  AGENTERM_REPLY_CAPTURE_DIR="$dir" "$@" > "$dir/capture-run.json" 2>&1 || true
  echo "captured $journey -> $dir"
}

capture server-smoke \
  "$BIN" cli script run --profile tool --json \
  --project-root scripts/qjs --timeout-ms 300000 --max-operations 1000000000 \
  research/qjswasm-host-reply-wire-cost/capture/server-smoke.capture.qjs \
  -- . "$BIN" "$BIN"

capture native-ipc-smoke \
  "$BIN" cli script run --profile tool --json \
  --project-root scripts/qjs --timeout-ms 120000 --max-operations 100000000 \
  research/qjswasm-host-reply-wire-cost/capture/native-ipc-smoke.capture.qjs \
  -- . "$BIN" "$BIN"

AGENTERM_NO_ACTIVATE=1
AGENTERM_QJS_MAX_MEMORY_PAGES=4096
export AGENTERM_NO_ACTIVATE AGENTERM_QJS_MAX_MEMORY_PAGES
capture workbench-smoke \
  "$BIN" cli script run --profile tool --json \
  --project-root scripts/qjs --timeout-ms 120000 --max-operations 1000000000 \
  research/qjswasm-host-reply-wire-cost/capture/workbench-smoke.capture.qjs \
  -- . "$BIN" "$BIN" --phase editing
