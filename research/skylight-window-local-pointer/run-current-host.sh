#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
BUILD="$ROOT/.build"
REPO=$(CDPATH='' cd -- "$ROOT/../.." && pwd)
SOURCE_SHA=$(git -C "$REPO" rev-parse HEAD)
PROBE_DIGEST=$(
  {
    printf '%s\n' 'fixture/Fixture.swift'
    shasum -a 256 "$ROOT/fixture/Fixture.swift" | awk '{print $1}'
    printf '%s\n' 'probe/Probe.swift'
    shasum -a 256 "$ROOT/probe/Probe.swift" | awk '{print $1}'
    printf '%s\n' 'run-current-host.sh'
    shasum -a 256 "$ROOT/run-current-host.sh" | awk '{print $1}'
  } | shasum -a 256 | awk '{print $1}'
)
RUN=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-skylight-probe.XXXXXX")
TARGET_STATE="$RUN/target.json"
TARGET_COMMAND="$RUN/target.command"
GUARD_STATE="$RUN/guard.json"
TARGET_PID=""
GUARD_PID=""

cleanup() {
  [ -z "$TARGET_PID" ] || kill "$TARGET_PID" 2>/dev/null || true
  [ -z "$GUARD_PID" ] || kill "$GUARD_PID" 2>/dev/null || true
  rm -rf "$RUN"
}
trap cleanup EXIT INT TERM HUP

stop_fixtures() {
  [ -z "$TARGET_PID" ] || kill "$TARGET_PID" 2>/dev/null || true
  [ -z "$GUARD_PID" ] || kill "$GUARD_PID" 2>/dev/null || true
  [ -z "$TARGET_PID" ] || wait "$TARGET_PID" 2>/dev/null || true
  [ -z "$GUARD_PID" ] || wait "$GUARD_PID" 2>/dev/null || true
  TARGET_PID=""
  GUARD_PID=""
  rm -f "$TARGET_STATE" "$TARGET_COMMAND" "$GUARD_STATE"
}

start_fixtures() {
  "$BUILD/Fixture" --role target --state "$TARGET_STATE" --command "$TARGET_COMMAND" &
  TARGET_PID=$!
  i=0
  while [ ! -s "$TARGET_STATE" ]; do
    i=$((i + 1)); [ "$i" -lt 200 ] || { echo "target fixture readiness timeout" >&2; exit 1; }
    sleep 0.02
  done

  "$BUILD/Fixture" --role guard --state "$GUARD_STATE" &
  GUARD_PID=$!
  i=0
  while [ ! -s "$GUARD_STATE" ]; do
    i=$((i + 1)); [ "$i" -lt 200 ] || { echo "guard fixture readiness timeout" >&2; exit 1; }
    sleep 0.02
  done
  sleep 0.15
}

mkdir -p "$BUILD"
xcrun swiftc -O -framework AppKit -framework Foundation \
  "$ROOT/fixture/Fixture.swift" -o "$BUILD/Fixture"
xcrun swiftc -O -framework AppKit -framework CoreGraphics -framework Foundation \
  "$ROOT/probe/Probe.swift" -o "$BUILD/Probe"

start_fixtures
"$BUILD/Probe" --target-state "$TARGET_STATE" --guard-state "$GUARD_STATE" \
  --target-command "$TARGET_COMMAND" --mode court \
  --source-sha "$SOURCE_SHA" --probe-digest "$PROBE_DIGEST"

# C8 is gated by the successful C1-C7 process above and owns a fresh fixture.
stop_fixtures
start_fixtures
"$BUILD/Probe" --target-state "$TARGET_STATE" --guard-state "$GUARD_STATE" \
  --target-command "$TARGET_COMMAND" --mode repeat \
  --source-sha "$SOURCE_SHA" --probe-digest "$PROBE_DIGEST"
