#!/bin/bash
# Run one registered public ACU journey in a Linux/Windows UTM court.
# Defaults preserve the historical managed-job entry; thin owning wrappers may
# select another checked-in task and its exact evidence/PASS lines.

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
TASK="${AGENTERM_UTM_TASK:-cu-managed-job-smoke}"
EVIDENCE="${AGENTERM_UTM_EVIDENCE:-cu.managed-job-lifecycle}"
PASS_LINE="${AGENTERM_UTM_PASS_LINE:-PASS: agenterm-cu platform-neutral managed-job lifecycle (replay, dual cursors, renewal, stop and owned cleanup)}"
case "$TASK" in ''|*[!a-z0-9-]*) echo "invalid UTM task id" >&2; exit 2 ;; esac
case "$EVIDENCE" in ''|*[!a-z0-9.-]*) echo "invalid UTM evidence id" >&2; exit 2 ;; esac
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

is_real_court_cli() {
  [ -n "$1" ] && [ -x "$1" ] && [ -f "$1" ] &&
    grep -q 'Uniform, product-neutral lifecycle' "$1" 2>/dev/null
}

resolve_court_cli() {
  for candidate in \
    "${UTM_COURT_CLI:-}" \
    "${UTM_COURT_HOME:-}/bin/utm-court" \
    "$REPO_ROOT/../utm-court/bin/utm-court" \
    "$HOME/repos/utm-court/bin/utm-court"
  do
    is_real_court_cli "$candidate" && { printf '%s\n' "$candidate"; return 0; }
  done
  found="$(command -v utm-court 2>/dev/null || true)"
  is_real_court_cli "$found" && { printf '%s\n' "$found"; return 0; }
  return 1
}
COURT_CLI="$(resolve_court_cli)" || {
  echo "utm-court CLI is unavailable; set UTM_COURT_CLI or UTM_COURT_HOME" >&2
  exit 2
}
export UTM_COURT_STATE_DIR="${UTM_COURT_STATE_DIR:-$REPO_ROOT/target/utm-court-service}"
WINDOWS_ROOT="${UTM_COURT_WINDOWS_ROOT:-$("$COURT_CLI" windows-root)}"

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
[ -f "$REPO_ROOT/scripts/qjs/$TASK.qjs" ] || { echo "task script is missing: $TASK" >&2; exit 2; }
cp "$REPO_ROOT/scripts/qjs/$TASK.qjs" "$PAYLOAD/scripts/qjs/"
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
EVIDENCE_DIR="$REPO_ROOT/target/utm-court-evidence/$SOURCE_SHA/$COURT/$TASK"

# Windows PowerShell 5 native redirection writes UTF-16LE, while Linux guests
# and newer shells write UTF-8. Evidence is a text protocol, so normalize only
# the pulled copy before exact EVIDENCE/PASS matching; never reinterpret the
# executable payload or its digest manifest.
normalize_pulled_log() {
  log_path="$1"
  python3 - "$log_path" <<'PY'
import os, pathlib, sys
path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
if data.startswith((b"\xff\xfe", b"\xfe\xff")):
    text = data.decode("utf-16")
elif data.startswith(b"\xef\xbb\xbf"):
    text = data.decode("utf-8-sig")
else:
    text = data.decode("utf-8")
temporary = path.with_name(path.name + ".utf8.tmp")
temporary.write_text(text, encoding="utf-8", newline="")
os.replace(temporary, path)
PY
}

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
    "rm -rf '$GUEST_ROOT'; mkdir -p '$GUEST_ROOT'; tar -xzf '$GUEST_ARCHIVE' -C '$GUEST_ROOT'; cd '$GUEST_ROOT'; sha256sum -c MANIFEST.sha256 >'$GUEST_LOG' 2>&1; rc=\$?; if [ \$rc -eq 0 ]; then target/debug/agenterm cli script task run '$TASK' --manifest agenterm.tasks.json >>'$GUEST_LOG' 2>&1; rc=\$?; fi; printf '%s' \"\$rc\" >'$GUEST_EXIT.tmp'; mv -f '$GUEST_EXIT.tmp' '$GUEST_EXIT'"
else
  "$COURT_CLI" interactive-ready "$COURT" 180 >/dev/null
  GUEST_BASE="$WINDOWS_ROOT"
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
    "  & \".\\target\\debug\\agenterm.com\" cli script task run $TASK --manifest agenterm.tasks.json *> \$log" \
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

result_timeout=360
[ "$COURT" = win-x86_64-desktop ] && result_timeout=600
deadline=$((SECONDS + result_timeout))
while :; do
  : >"$LOCAL_EXIT"
  "$COURT_CLI" pull "$COURT" "$GUEST_EXIT" "$LOCAL_EXIT" >/dev/null 2>&1 || true
  [ -s "$LOCAL_EXIT" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    mkdir -p "$EVIDENCE_DIR"
    if "$COURT_CLI" pull "$COURT" "$GUEST_LOG" "$LOCAL_LOG" >/dev/null 2>&1; then
      normalize_pulled_log "$LOCAL_LOG"
      cp "$LOCAL_LOG" "$EVIDENCE_DIR/timeout.log"
      cat "$LOCAL_LOG" >&2
    fi
    if [ "$GUEST_OS" = windows ]; then
      "$COURT_CLI" pull "$COURT" \
        "$WINDOWS_ROOT\\agent-v2\\job.log" "$EVIDENCE_DIR/agent-job.log" \
        >/dev/null 2>&1 || true
    fi
    printf '124\n' >"$EVIDENCE_DIR/run.exit"
    python3 - "$EVIDENCE_DIR/receipt.json" "$SOURCE_SHA" "$COURT" "$BUNDLE_SHA" "$EVIDENCE" <<'PY'
import datetime, json, os, sys
receipt = {
    "schema": 1,
    "source_sha": sys.argv[2],
    "court": sys.argv[3],
    "bundle_sha256": sys.argv[4],
    "exit_code": 124,
    "outcome": "timeout",
    "evidence": sys.argv[5],
    "completed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
with open(sys.argv[1] + ".tmp", "w", encoding="utf-8") as stream:
    json.dump(receipt, stream, indent=2, sort_keys=True)
    stream.write("\n")
os.replace(sys.argv[1] + ".tmp", sys.argv[1])
PY
    echo "UTM $TASK journey timed out after ${result_timeout}s" >&2
    exit 1
  fi
  sleep 1
done
"$COURT_CLI" pull "$COURT" "$GUEST_LOG" "$LOCAL_LOG"
normalize_pulled_log "$LOCAL_LOG"
cat "$LOCAL_LOG"
RUN_RC="$(tr -d '\r\n ' <"$LOCAL_EXIT")"
case "$RUN_RC" in ''|*[!0-9]*) echo "invalid guest exit receipt" >&2; exit 1 ;; esac

FINAL_RC="$RUN_RC"
OUTCOME=failed
if [ "$RUN_RC" -eq 0 ] &&
   grep -Fqx "EVIDENCE $EVIDENCE" "$LOCAL_LOG" &&
   grep -Fqx "$PASS_LINE" "$LOCAL_LOG"; then
  FINAL_RC=0
  OUTCOME=passed
elif [ "$RUN_RC" -eq 0 ]; then
  # A zero guest exit without both exact protocol lines is a court failure,
  # not a successful receipt followed by an incidental grep error.
  FINAL_RC=1
fi

mkdir -p "$EVIDENCE_DIR"
cp "$LOCAL_LOG" "$EVIDENCE_DIR/run.log"
cp "$LOCAL_EXIT" "$EVIDENCE_DIR/run.exit"
cp "$PAYLOAD/MANIFEST.sha256" "$EVIDENCE_DIR/MANIFEST.sha256"
python3 - "$EVIDENCE_DIR/receipt.json" "$SOURCE_SHA" "$COURT" "$BUNDLE_SHA" "$FINAL_RC" "$RUN_RC" "$OUTCOME" "$EVIDENCE" <<'PY'
import datetime, json, sys
receipt = {
    "schema": 1,
    "source_sha": sys.argv[2],
    "court": sys.argv[3],
    "bundle_sha256": sys.argv[4],
    "exit_code": int(sys.argv[5]),
    "guest_exit_code": int(sys.argv[6]),
    "outcome": sys.argv[7],
    "evidence": sys.argv[8],
    "completed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
with open(sys.argv[1] + ".tmp", "w", encoding="utf-8") as stream:
    json.dump(receipt, stream, indent=2, sort_keys=True)
    stream.write("\n")
import os
os.replace(sys.argv[1] + ".tmp", sys.argv[1])
PY

[ "$FINAL_RC" -eq 0 ] || exit "$FINAL_RC"
