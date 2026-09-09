#!/bin/sh
# Pre-committed 1000-action Chromium PRIVATE repeat runner.
#
# Rebuilds the frozen discriminator sources (chromium/Inject.swift,
# fixture/Fixture.swift) exactly like run-chromium-current-host.sh and reuses
# fixture-chromium/page.html unchanged. One parent qjs invocation owns the
# fixture while fresh 50-action qjs workers reset the interpreter step budget.
# The verdict JSON stays on stdout for the caller. Optional ACTIONS (1..1000)
# is the detector-test knob and is passed through as the ninth argument.
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO=$(CDPATH='' cd -- "$ROOT/../.." && pwd)
BUILD="$ROOT/.build/chromium"
AGENTERM_EXE=${AGENTERM_EXE:-"$REPO/target/debug/agenterm"}
AGENTERM_CU_EXE=${AGENTERM_CU_EXE:-"$REPO/target/debug/agenterm-cu"}
CHROMIUM_EXE=${AGENTERM_CU_BROWSER_EXE:-}
ACTIONS=${ACU074_REPEAT_ACTIONS:-}
REPEAT="$ROOT/fixture-chromium/repeat-chromium.qjs"
BLOCK="$ROOT/fixture-chromium/repeat-chromium-block.qjs"
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
require_file "$REPEAT" fixture-chromium/repeat-chromium.qjs
require_file "$BLOCK" fixture-chromium/repeat-chromium-block.qjs
require_file "$PAGE" fixture-chromium/page.html
require_file "$INJECT_SOURCE" chromium/Inject.swift
require_file "$GUARD_SOURCE" fixture/Fixture.swift

if [ -n "$ACTIONS" ]; then
  case "$ACTIONS" in
    ''|*[!0-9]*)
      printf '%s\n' "ACU074_REPEAT_ACTIONS must be an integer from 1 to 1000" >&2
      exit 2
      ;;
  esac
  if [ "$ACTIONS" -lt 1 ] || [ "$ACTIONS" -gt 1000 ]; then
    printf '%s\n' "ACU074_REPEAT_ACTIONS must be an integer from 1 to 1000" >&2
    exit 2
  fi
fi

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
      fixture-chromium/repeat-chromium.qjs \
      fixture-chromium/repeat-chromium-block.qjs \
      chromium/Inject.swift \
      fixture/Fixture.swift \
      run-chromium-repeat-current-host.sh
    do
      printf '%s\n' "$source"
      shasum -a 256 "$ROOT/$source" | awk '{print $1}'
    done
  } | shasum -a 256 | awk '{print $1}'
)

# Keep the display and system awake for the ~20-minute repeat when the tool
# exists; the court does not depend on it and records its own timebox.
WRAP=""
if command -v caffeinate >/dev/null 2>&1; then
  WRAP="caffeinate -dis"
fi

set -- \
  "$REPO" \
  "$AGENTERM_EXE" \
  "$AGENTERM_CU_EXE" \
  "$CHROMIUM_EXE" \
  "$INJECTOR" \
  "$GUARD" \
  "$SOURCE_SHA" \
  "$PROBE_DIGEST"
if [ -n "$ACTIONS" ]; then
  set -- "$@" "$ACTIONS"
fi

exec $WRAP "$AGENTERM_EXE" cli script run \
  --profile tool \
  --timeout-ms 1800000 \
  --max-operations 1000000000 \
  --max-host-operations 1000000 \
  --max-output-bytes 1048576 \
  --max-string-bytes 1048576 \
  --project-root "$REPO/scripts/qjs" \
  "$REPEAT" \
  -- \
  "$@"
