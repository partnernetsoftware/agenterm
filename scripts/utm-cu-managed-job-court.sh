#!/bin/bash
# Run the registered public managed-job journey in one Linux/Windows UTM court.

set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/utm-cu-managed-job-court.sh COURT PROFILE_DIR

COURT is one of lnx-aarch64-desktop, lnx-x86_64-desktop,
win-aarch64-desktop, or win-x86_64-desktop. PROFILE_DIR is the host directory
containing same-cell release-fast artifacts. The runner packages those exact
bytes under the task manifest's target/debug paths, executes the registered
cu-managed-job-smoke journey, retains a receipt under target/, and releases
only the VM leased by this invocation.
EOF
}

[ "$#" -eq 2 ] || { usage >&2; exit 2; }
COURT="$1"
PROFILE_DIR="$2"
case "$COURT" in
  lnx-aarch64-desktop|lnx-x86_64-desktop) GUEST_OS=linux ;;
  win-aarch64-desktop|win-x86_64-desktop) GUEST_OS=windows ;;
  *) echo "unsupported UTM court: $COURT" >&2; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
case "$PROFILE_DIR" in
  /*) ;;
  *) PROFILE_DIR="$REPO_ROOT/$PROFILE_DIR" ;;
esac
[ -d "$PROFILE_DIR" ] || { echo "artifact profile directory is missing" >&2; exit 2; }

git -C "$REPO_ROOT" diff --quiet --
git -C "$REPO_ROOT" diff --cached --quiet --
SOURCE_SHA="$(git -C "$REPO_ROOT" rev-parse HEAD)"
case "$SOURCE_SHA" in
  [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]* ) ;;
  *) echo "source identity is unavailable" >&2; exit 2 ;;
esac

resolve_court_cli() {
  for candidate in \
    "${UTM_COURT_CLI:-}" \
    "${UTM_COURT_HOME:-}/bin/utm-court" \
    "$REPO_ROOT/../utm-court/bin/utm-court" \
    "$HOME/repos/utm-court/bin/utm-court"
  do
    [ -n "$candidate" ] && [ -x "$candidate" ] && { printf '%s\n' "$candidate"; return 0; }
  done
  command -v utm-court 2>/dev/null || return 1
}
COURT_CLI="$(resolve_court_cli)" || {
  echo "utm-court CLI is unavailable; set UTM_COURT_CLI or UTM_COURT_HOME" >&2
  exit 2
}
export UTM_COURT_STATE_DIR="${UTM_COURT_STATE_DIR:-$REPO_ROOT/target/utm-court-service}"

SCRATCH="$(mktemp -d)"
LEASED=0
cleanup() {
  if [ "$LEASED" -eq 1 ]; then
    "$COURT_CLI" release "$COURT" >/dev/null 2>&1 || true
  fi
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

PAYLOAD="$SCRATCH/payload"
mkdir -p "$PAYLOAD/target/debug" "$PAYLOAD/scripts/qjs/lib"
cp "$REPO_ROOT/agenterm.tasks.json" "$PAYLOAD/"
cp "$REPO_ROOT/scripts/qjs/cu-managed-job-smoke.qjs" "$PAYLOAD/scripts/qjs/"
cp "$REPO_ROOT/scripts/qjs/lib/rh_compat.qjs" "$PAYLOAD/scripts/qjs/lib/"
cp "$REPO_ROOT/scripts/qjs/lib/test_harness.qjs" "$PAYLOAD/scripts/qjs/lib/"
printf '%s\n' "$SOURCE_SHA" >"$PAYLOAD/SOURCE_SHA"

if [ "$GUEST_OS" = linux ]; then
  for name in agenterm agenterm-cu libagenterm.so; do
    [ -f "$PROFILE_DIR/$name" ] || { echo "Linux artifact missing: $name" >&2; exit 2; }
    cp "$PROFILE_DIR/$name" "$PAYLOAD/target/debug/$name"
  done
  chmod +x "$PAYLOAD/target/debug/agenterm" "$PAYLOAD/target/debug/agenterm-cu"
else
  for name in agenterm.exe agenterm-com.exe agenterm-cu.exe agenterm.dll; do
    [ -f "$PROFILE_DIR/$name" ] || { echo "Windows artifact missing: $name" >&2; exit 2; }
  done
  cp "$PROFILE_DIR/agenterm.exe" "$PAYLOAD/target/debug/agenterm.exe"
  cp "$PROFILE_DIR/agenterm-com.exe" "$PAYLOAD/target/debug/agenterm.com"
  cp "$PROFILE_DIR/agenterm-cu.exe" "$PAYLOAD/target/debug/agenterm-cu.exe"
  cp "$PROFILE_DIR/agenterm.dll" "$PAYLOAD/target/debug/agenterm.dll"
fi

(cd "$PAYLOAD" && find . -type f ! -name MANIFEST.sha256 -print0 |
  LC_ALL=C sort -z | xargs -0 shasum -a 256) >"$PAYLOAD/MANIFEST.sha256"
BUNDLE_SHA=""
if [ "$GUEST_OS" = linux ]; then
  ARCHIVE="$SCRATCH/bundle.tar.gz"
  (cd "$PAYLOAD" && tar -czf "$ARCHIVE" .)
else
  ARCHIVE="$SCRATCH/bundle.zip"
  (cd "$PAYLOAD" && zip -q -r "$ARCHIVE" .)
fi
BUNDLE_SHA="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"

"$COURT_CLI" lease "$COURT" --disposable >/dev/null
LEASED=1
"$COURT_CLI" wait-ready "$COURT" 180 >/dev/null

RUN_ID="${SOURCE_SHA:0:12}-$COURT-$$-$RANDOM"
LOCAL_LOG="$SCRATCH/run.log"
LOCAL_EXIT="$SCRATCH/run.exit"
if [ "$GUEST_OS" = linux ]; then
  GUEST_ARCHIVE="/tmp/agenterm-$RUN_ID.tar.gz"
  GUEST_ROOT="/tmp/agenterm-$RUN_ID"
  GUEST_LOG="/tmp/agenterm-$RUN_ID.log"
  GUEST_EXIT="/tmp/agenterm-$RUN_ID.exit"
  # The x86_64 guest is fully emulated on Apple Silicon.  A multi-megabyte
  # delivery bundle can legitimately exceed utm-court's 30-second default
  # transfer deadline even though the Guest Agent is healthy (the tiny probe
  # above still completes).  Bound this exact bulk transfer separately; do not
  # weaken the service-wide command/receipt deadlines.
  UTM_COURT_TRANSFER_TIMEOUT=180 \
    "$COURT_CLI" push "$COURT" "$ARCHIVE" "$GUEST_ARCHIVE"
  "$COURT_CLI" exec "$COURT" -- /bin/bash -lc \
    "rm -rf '$GUEST_ROOT'; mkdir -p '$GUEST_ROOT'; tar -xzf '$GUEST_ARCHIVE' -C '$GUEST_ROOT'; cd '$GUEST_ROOT'; sha256sum -c MANIFEST.sha256 >'$GUEST_LOG' 2>&1; rc=\$?; if [ \$rc -eq 0 ]; then target/debug/agenterm cli script task run cu-managed-job-smoke --manifest agenterm.tasks.json >>'$GUEST_LOG' 2>&1; rc=\$?; fi; printf '%s' \"\$rc\" >'$GUEST_EXIT.tmp'; mv -f '$GUEST_EXIT.tmp' '$GUEST_EXIT'"
else
  "$COURT_CLI" interactive-ready "$COURT" 180 >/dev/null
  GUEST_BASE="C:\\minicon-six"
  GUEST_ARCHIVE="$GUEST_BASE\\agenterm-$RUN_ID.zip"
  GUEST_ROOT="$GUEST_BASE\\agenterm-$RUN_ID"
  GUEST_LOG="$GUEST_BASE\\agenterm-$RUN_ID.log"
  GUEST_EXIT="$GUEST_BASE\\agenterm-$RUN_ID.exit"
  JOB="$GUEST_BASE\\agent-v2\\job.pending.ps1"
  READY="$GUEST_BASE\\agent-v2\\job.ready"
  UTM_COURT_TRANSFER_TIMEOUT=180 \
    "$COURT_CLI" push "$COURT" "$ARCHIVE" "$GUEST_ARCHIVE"
  printf '%s\n' \
    '$ErrorActionPreference = "Stop"' \
    "\$root = '$GUEST_ROOT'" \
    "\$archive = '$GUEST_ARCHIVE'" \
    "\$log = '$GUEST_LOG'" \
    "\$result = '$GUEST_EXIT'" \
    '$resultTmp = $result + ".tmp"' \
    '$exitCode = 1' \
    'try {' \
    '  Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue' \
    '  Expand-Archive -LiteralPath $archive -DestinationPath $root -Force' \
    '  Push-Location $root' \
    '  foreach ($line in Get-Content -LiteralPath "MANIFEST.sha256") {' \
    '    $parts = $line -split "  ", 2' \
    '    if ($parts.Count -ne 2) { throw "invalid manifest row" }' \
    '    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $parts[1]).Hash.ToLowerInvariant()' \
    '    if ($actual -ne $parts[0]) { throw "artifact digest mismatch" }' \
    '  }' \
    '  & ".\target\debug\agenterm.com" cli script task run cu-managed-job-smoke --manifest agenterm.tasks.json *> $log' \
    '  $exitCode = $LASTEXITCODE' \
    '} catch {' \
    '  $_ | Out-String | Add-Content -LiteralPath $log' \
    '  $exitCode = 1' \
    '} finally {' \
    '  Pop-Location -ErrorAction SilentlyContinue' \
    '  [IO.File]::WriteAllText($resultTmp, [string]$exitCode)' \
    '  Move-Item -LiteralPath $resultTmp -Destination $result -Force' \
    '}' \
    'exit $exitCode' | "$COURT_CLI" push "$COURT" - "$JOB"
  printf ready | "$COURT_CLI" push "$COURT" - "$READY"
fi

deadline=$((SECONDS + 360))
while :; do
  : >"$LOCAL_EXIT"
  "$COURT_CLI" pull "$COURT" "$GUEST_EXIT" "$LOCAL_EXIT" >/dev/null 2>&1 || true
  [ -s "$LOCAL_EXIT" ] && break
  [ "$SECONDS" -lt "$deadline" ] || { echo "UTM managed-job journey timed out" >&2; exit 1; }
  sleep 1
done
"$COURT_CLI" pull "$COURT" "$GUEST_LOG" "$LOCAL_LOG"
cat "$LOCAL_LOG"
RUN_RC="$(tr -d '\r\n ' <"$LOCAL_EXIT")"
case "$RUN_RC" in ''|*[!0-9]*) echo "invalid guest exit receipt" >&2; exit 1 ;; esac

EVIDENCE_DIR="$REPO_ROOT/target/utm-court-evidence/$SOURCE_SHA/$COURT"
mkdir -p "$EVIDENCE_DIR"
cp "$LOCAL_LOG" "$EVIDENCE_DIR/run.log"
cp "$LOCAL_EXIT" "$EVIDENCE_DIR/run.exit"
cp "$PAYLOAD/MANIFEST.sha256" "$EVIDENCE_DIR/MANIFEST.sha256"
python3 - "$EVIDENCE_DIR/receipt.json" "$SOURCE_SHA" "$COURT" "$BUNDLE_SHA" "$RUN_RC" <<'PY'
import datetime, json, sys
receipt = {
    "schema": 1,
    "source_sha": sys.argv[2],
    "court": sys.argv[3],
    "bundle_sha256": sys.argv[4],
    "exit_code": int(sys.argv[5]),
    "evidence": "cu.managed-job-lifecycle",
    "completed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
with open(sys.argv[1] + ".tmp", "w", encoding="utf-8") as stream:
    json.dump(receipt, stream, indent=2, sort_keys=True)
    stream.write("\n")
import os
os.replace(sys.argv[1] + ".tmp", sys.argv[1])
PY

[ "$RUN_RC" -eq 0 ] || exit "$RUN_RC"
grep -Fqx 'EVIDENCE cu.managed-job-lifecycle' "$LOCAL_LOG"
grep -Fqx 'PASS: agenterm-cu platform-neutral managed-job lifecycle (replay, dual cursors, renewal, stop and owned cleanup)' "$LOCAL_LOG"
