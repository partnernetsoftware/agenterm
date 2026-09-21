#!/bin/bash
# Run the Candidate's Windows runtime cell in a UTM court instead of CI.
#
# The cell is `cu-retirement-cell-smoke` plus the two setup courts that feed it,
# driven exactly as `.github/workflows/candidate.yml` drives them -- same five
# artifacts, same launcher, same argument order. It exists because the release
# pipeline takes 45 minutes to report one exception string, and this reports the
# whole log in about three.
#
# Usage: scripts/windows-court-runtime-cell.sh [court] [target]
set -uo pipefail
R="$(cd "$(dirname "$0")/.." && pwd)"
COURT="${1:-win-aarch64-desktop}"
TARGET="${2:-aarch64-pc-windows-msvc}"
C="${UTM_COURT_CLI:-$HOME/repos/utm-court/bin/utm-court}"
# The platform id the cell stamps into its receipt is the ARTIFACT's, not the
# court's: an x86_64 build proved on the aarch64 court under emulation is still
# an x86_64 cell, and saying otherwise would make the receipt a lie.
case "$TARGET" in
  aarch64-pc-windows-msvc) PLATFORM_ID=windows-aarch64 ;;
  x86_64-pc-windows-msvc) PLATFORM_ID=windows-x86_64 ;;
  *) echo "unsupported target $TARGET"; exit 1 ;;
esac
S="${WINDOWS_COURT_STAGE:-${TMPDIR:-/tmp}/agenterm-runtime-cell}"
# Each run gets its own guest directory. A DLL the guest has loaded once can
# stay locked long after every visible process is gone, and pushing over it
# fails with "being used by another process" -- which reads like a transport
# fault and is not one. A fresh directory cannot be locked by anything.
G="C:\\agenterm-cell\\$(date -u +%Y%m%dT%H%M%SZ)"
rm -rf "$S" && mkdir -p "$S"

FAST="$R/target/$TARGET/release-fast"
ABI="$R/target/$TARGET/abi-release"
# The five artifacts the cell names, under the exact file names it expects.
cp "$FAST/agenterm-com.exe" "$S/agenterm.com"
cp "$FAST/agenterm.exe" "$S/agenterm.exe"
cp "$FAST/agenterm-cu.exe" "$S/agenterm-cu.exe"
cp "$ABI/agenterm_cu_provider.dll" "$S/agenterm-cu-provider.dll"
cp "$ABI/agenterm.dll" "$S/agenterm.dll"
for f in cu-retirement-cell-smoke cu-setup-cli-smoke cu-setup-runtime-refresh-smoke \
         acu-provider-smoke acu-mcp-provider-smoke acu-power-action-provider-smoke \
         native-acu-composition-smoke; do
  cp "$R/scripts/qjs/$f.qjs" "$S/$f.qjs"
done

"$C" lease "$COURT" >/dev/null 2>&1 || { echo "LEASE FAILED $COURT"; exit 1; }
"$C" wait-ready "$COURT" 240 >/dev/null 2>&1
# A DLL that is still loaded cannot be overwritten, and the failure is silent.
"$C" exec "$COURT" -- cmd.exe /c \
  "taskkill /f /im agenterm.exe /t >nul 2>&1 & taskkill /f /im agenterm-cu.exe /t >nul 2>&1 & exit 0" \
  >/dev/null 2>&1
"$C" exec "$COURT" -- cmd.exe /c "mkdir $G\\runtime" >/dev/null 2>&1
"$C" exec "$COURT" -- cmd.exe /c "mkdir $G\\runtime-control" >/dev/null 2>&1
"$C" exec "$COURT" -- cmd.exe /c "mkdir $G\\runtime-evidence" >/dev/null 2>&1
# `exec` returns when the guest agent accepts a command, not when the guest has
# run it, so a push issued straight after a mkdir can arrive before the
# directory exists and fail with "cannot find the path". This is the barrier.
"$C" exec "$COURT" -- cmd.exe /c "ping -n 4 127.0.0.1" >/dev/null 2>&1

# One artifact per transfer: a combined archive times out where each of these
# lands in seconds.
push_one() {
  for _ in 1 2 3; do
    "$C" push "$COURT" "$1" "$2" >/dev/null 2>&1 && return 0
    "$C" exec "$COURT" -- cmd.exe /c "ping -n 4 127.0.0.1" >/dev/null 2>&1
  done
  echo "PUSH FAILED $(basename "$1")"
  return 1
}
# The binaries go over compressed, one archive per transfer. The 12 MB ACU
# provider fails intermittently as a raw push when it follows other transfers;
# at ~4 MB compressed every artifact lands first time.
for f in agenterm.com agenterm.exe agenterm-cu.exe agenterm-cu-provider.dll agenterm.dll; do
  tar -czf "$S/$f.tgz" -C "$S" "$f" || { echo "TAR FAILED $f"; exit 1; }
  push_one "$S/$f.tgz" "$G\\runtime\\$f.tgz" || exit 1
done
# `&&` rather than `&`: chaining extractions with `&` runs them concurrently and
# a copy can read a file the extraction has not finished writing. That cost two
# rounds once already.
"$C" exec "$COURT" -- cmd.exe /c \
  "cd /d $G\\runtime && tar -xzf agenterm.com.tgz && tar -xzf agenterm.exe.tgz && tar -xzf agenterm-cu.exe.tgz && tar -xzf agenterm-cu-provider.dll.tgz && tar -xzf agenterm.dll.tgz" \
  >/dev/null 2>&1
"$C" exec "$COURT" -- cmd.exe /c "ping -n 10 127.0.0.1" >/dev/null 2>&1
for f in "$S"/*.qjs; do
  push_one "$f" "$G\\runtime-control\\$(basename "$f")" || exit 1
done
# The same two library modules `candidate.yml` ships in runtime-control; the
# gate scripts import `lib/rh_compat` relative to themselves.
"$C" exec "$COURT" -- cmd.exe /c "mkdir $G\\runtime-control\\lib" >/dev/null 2>&1
"$C" exec "$COURT" -- cmd.exe /c "ping -n 4 127.0.0.1" >/dev/null 2>&1
for f in rh_compat test_harness; do
  push_one "$R/scripts/qjs/lib/$f.qjs" "$G\\runtime-control\\lib\\$f.qjs" || exit 1
done

SHA="$(git -C "$R" rev-parse HEAD)"
# The cell binds a receipt to an archive hash; outside CI there is no archive,
# so this names the bytes actually under test rather than inventing one.
ASHA="$(cat "$S"/agenterm.exe "$S"/agenterm-cu.exe "$S"/agenterm-cu-provider.dll | shasum -a 256 | cut -d' ' -f1)"
LOG="$G\\cell.log"
# `exec` returns when the guest agent ACCEPTS the command, not when the guest
# has finished running it. Without a barrier all three steps start at once and
# the cell reads a receipt its own predecessor has not written yet -- which
# surfaces as `..._receipt_missing`, a failure that names a missing file rather
# than the race that caused it. Each step ends by stamping a marker into the
# log, and the caller waits for that marker before starting the next.
step=0
run_cell() {
  step=$((step + 1))
  local marker="CELL_STEP_${step}_DONE"
  "$C" exec "$COURT" -- cmd.exe /c "cd /d $G && $1 >> $LOG 2>&1 & echo $marker >> $LOG"
  for _ in $(seq 1 120); do
    "$C" exec "$COURT" -- cmd.exe /c "ping -n 4 127.0.0.1" >/dev/null 2>&1
    "$C" pull "$COURT" "$LOG" "$S/cell.log" >/dev/null 2>&1 || continue
    grep -q "$marker" "$S/cell.log" && return 0
  done
  echo "STEP $step DID NOT FINISH"
  return 1
}
B='set AGENTERM_NO_ACTIVATE=1 && runtime\agenterm.com cli script run --profile tool'
run_cell "$B --timeout-ms 120000 --max-operations 100000000 --max-output-bytes 262144 runtime-control\\cu-setup-cli-smoke.qjs -- $G runtime\\agenterm-cu.exe runtime-evidence\\setup-$PLATFORM_ID.json $PLATFORM_ID"
run_cell "$B --timeout-ms 120000 --max-operations 100000000 --max-output-bytes 262144 runtime-control\\cu-setup-runtime-refresh-smoke.qjs -- $G runtime\\agenterm-cu.exe runtime-evidence\\setup-refresh-$PLATFORM_ID.json $PLATFORM_ID"
run_cell "$B --timeout-ms 900000 --max-operations 1000000000 runtime-control\\cu-retirement-cell-smoke.qjs -- runtime\\agenterm.com runtime\\agenterm.exe runtime\\agenterm-cu.exe runtime\\agenterm-cu-provider.dll runtime\\agenterm.dll $PLATFORM_ID $SHA $ASHA 1 1 runtime-evidence\\$PLATFORM_ID.json runtime-evidence\\setup-$PLATFORM_ID.json runtime-evidence\\setup-refresh-$PLATFORM_ID.json runtime-control\\acu-provider-smoke.qjs runtime-control\\acu-mcp-provider-smoke.qjs runtime-control\\acu-power-action-provider-smoke.qjs runtime-control\\native-acu-composition-smoke.qjs"

# `exec` returns when the guest agent accepts the command, not when the guest
# shell closes its redirect, so the pull can lose the race to the writer.
for _ in 1 2 3 4 5 6; do
  "$C" pull "$COURT" "$LOG" "$S/cell.log" >/dev/null 2>&1 && break
  "$C" exec "$COURT" -- cmd.exe /c "ping -n 3 127.0.0.1" >/dev/null 2>&1
done
cat "$S/cell.log"
