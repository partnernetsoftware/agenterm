#!/bin/sh
# Frozen control runs (V2): each journey's own unmodified command and budget,
# three times, in the experiment lane. Writes one JSON envelope per run to
# ../runs/<journey>-<n>.json and echoes the judged fields.
#
# Usage (from repository root):
#   sh research/qjswasm-host-reply-wire-cost/tools/run-control.sh [n]
set -eu
ROOT=$(pwd)
LANE="$ROOT/target/frontier-host-reply-wire"
BIN="$LANE/debug/agenterm"
HERE="$ROOT/research/qjswasm-host-reply-wire-cost"
OUT="$HERE/runs"
mkdir -p "$OUT"

cmd_server_smoke() {
  "$BIN" cli script run --profile tool --json \
    --timeout-ms 300000 --max-operations 1000000000 \
    scripts/qjs/server-smoke.qjs -- . "$BIN" "$BIN"
}
cmd_native_ipc() {
  "$BIN" cli script run --profile tool --json \
    --timeout-ms 120000 --max-operations 100000000 \
    scripts/qjs/native-ipc-smoke.qjs -- . "$BIN" "$BIN"
}
cmd_workbench_editing() {
  AGENTERM_NO_ACTIVATE=1 AGENTERM_QJS_MAX_MEMORY_PAGES=4096 \
  "$BIN" cli script run --profile tool --json \
    --timeout-ms 120000 --max-operations 1000000000 \
    scripts/qjs/workbench-smoke.qjs -- . "$BIN" "$BIN" --phase editing
}

N=${1:-3}
for journey in server-smoke native-ipc-smoke workbench-smoke; do
  n=1
  while [ "$n" -le "$N" ]; do
    file="$OUT/$journey-$n.json"
    case "$journey" in
      server-smoke) cmd_server_smoke > "$file" 2>&1 || true ;;
      native-ipc-smoke) cmd_native_ipc > "$file" 2>&1 || true ;;
      workbench-smoke) cmd_workbench_editing > "$file" 2>&1 || true ;;
    esac
    echo "ran $journey run $n -> $file"
    n=$((n + 1))
  done
done
