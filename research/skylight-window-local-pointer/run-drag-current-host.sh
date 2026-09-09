#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$ROOT/../.." && pwd)
BUILD="$ROOT/.build/drag"
AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}
AGENTERM_CU_EXE=${AGENTERM_CU_EXE:-"$REPO/target/debug/agenterm-cu"}
CHROMIUM_EXE=${AGENTERM_CU_BROWSER_EXE:-}
COURT="$ROOT/drag/court-chromium.qjs"
PAGE="$ROOT/drag/page.html"
INJECT_SOURCE="$ROOT/drag/InjectDrag.swift"
GUARD_SOURCE="$ROOT/fixture/Fixture.swift"
INJECTOR="$BUILD/InjectDrag"
GUARD="$BUILD/GuardFixture"
DRY_ATTEMPT=${ACU004_DRY_ATTEMPT:-1}
REPAIR_COUNT=${ACU004_REPAIR_COUNT:-0}
RUN_NUMBER=${ACU004_RUN_NUMBER:-1}

require_file() {
  [ -f "$1" ] || {
    printf '%s\n' "missing required file: $2" >&2
    exit 2
  }
}

require_file "$AGENTERM_EXE" AGENTERM_EXE
require_file "$AGENTERM_CU_EXE" AGENTERM_CU_EXE
require_file "$CHROMIUM_EXE" AGENTERM_CU_BROWSER_EXE
require_file "$COURT" drag/court-chromium.qjs
require_file "$PAGE" drag/page.html
require_file "$INJECT_SOURCE" drag/InjectDrag.swift
require_file "$GUARD_SOURCE" fixture/Fixture.swift

case "$DRY_ATTEMPT:$REPAIR_COUNT:$RUN_NUMBER" in
  [1-6]:[0-2]:[1-2]) ;;
  *)
    printf '%s\n' "ACU004_DRY_ATTEMPT must be 1..6, ACU004_REPAIR_COUNT 0..2, and ACU004_RUN_NUMBER 1..2" >&2
    exit 2
    ;;
esac

DIRTY=$(git -C "$REPO" status --porcelain --untracked-files=all -- \
  research/skylight-window-local-pointer/drag \
  research/skylight-window-local-pointer/fixture/Fixture.swift \
  research/skylight-window-local-pointer/run-drag-current-host.sh)
if [ -n "$DIRTY" ]; then
  printf '%s\n' "the section 10.6 research sources must be frozen in the current commit" >&2
  exit 2
fi

SOURCE_SHA=$(git -C "$REPO" rev-parse HEAD)
git -C "$REPO" merge-base --is-ancestor "$SOURCE_SHA" origin/main || {
  printf '%s\n' "the frozen section 10.6 commit must be reachable from origin/main" >&2
  exit 2
}

mkdir -p "$BUILD"
xcrun swiftc -O -framework AppKit -framework ApplicationServices \
  -framework CoreGraphics -framework Foundation "$INJECT_SOURCE" -o "$INJECTOR"
xcrun swiftc -O -framework AppKit -framework Foundation \
  "$GUARD_SOURCE" -o "$GUARD"

PROBE_DIGEST=$(
  {
    for source in \
      drag/page.html \
      drag/court-chromium.qjs \
      drag/InjectDrag.swift \
      fixture/Fixture.swift \
      run-drag-current-host.sh
    do
      printf '%s\n' "$source"
      shasum -a 256 "$ROOT/$source" | awk '{print $1}'
    done
  } | shasum -a 256 | awk '{print $1}'
)

exec "$AGENTERM_EXE" cli script run \
  --profile tool \
  --timeout-ms 900000 \
  --max-operations 1000000000 \
  --max-host-operations 1000000 \
  --max-output-bytes 1048576 \
  --max-string-bytes 1048576 \
  --project-root "$REPO/scripts/qjs" \
  "$COURT" \
  -- \
  "$REPO" \
  "$AGENTERM_CU_EXE" \
  "$CHROMIUM_EXE" \
  "$INJECTOR" \
  "$GUARD" \
  "$SOURCE_SHA" \
  "$PROBE_DIGEST" \
  "$DRY_ATTEMPT" \
  "$REPAIR_COUNT" \
  "$RUN_NUMBER"
