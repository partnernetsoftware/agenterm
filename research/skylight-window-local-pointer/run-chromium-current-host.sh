#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$ROOT/../.." && pwd)
BUILD="$ROOT/.build/chromium"
AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}
AGENTERM_CU_EXE=${AGENTERM_CU_EXE:-"$REPO/target/debug/agenterm-cu"}
CHROMIUM_EXE=${AGENTERM_CU_BROWSER_EXE:-}
COURT="$ROOT/fixture-chromium/court-chromium.qjs"
PAGE="$ROOT/fixture-chromium/page.html"
INJECT_SOURCE="$ROOT/chromium/Inject.swift"
GUARD_SOURCE="$ROOT/fixture/Fixture.swift"
INJECTOR="$BUILD/Inject"
GUARD="$BUILD/GuardFixture"

require_file() {
  [ -f "$1" ] || {
    printf '%s\n' "missing required file: $2" >&2
    exit 2
  }
}

require_file "$AGENTERM_EXE" AGENTERM_EXE
require_file "$AGENTERM_CU_EXE" AGENTERM_CU_EXE
require_file "$CHROMIUM_EXE" AGENTERM_CU_BROWSER_EXE
require_file "$COURT" fixture-chromium/court-chromium.qjs
require_file "$PAGE" fixture-chromium/page.html
require_file "$INJECT_SOURCE" chromium/Inject.swift
require_file "$GUARD_SOURCE" fixture/Fixture.swift

mkdir -p "$BUILD"
xcrun swiftc -O -framework AppKit -framework CoreGraphics -framework Foundation \
  "$INJECT_SOURCE" -o "$INJECTOR"
xcrun swiftc -O -framework AppKit -framework Foundation \
  "$GUARD_SOURCE" -o "$GUARD"

SOURCE_SHA=$(git -C "$REPO" rev-parse HEAD)
PROBE_DIGEST=$(
  {
    for source in \
      fixture-chromium/page.html \
      fixture-chromium/court-chromium.qjs \
      chromium/Inject.swift \
      fixture/Fixture.swift \
      run-chromium-current-host.sh
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
  "$PROBE_DIGEST"
