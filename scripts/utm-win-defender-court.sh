#!/bin/bash
# Scan the sealed Windows Candidate bytes with Microsoft Defender in an AgenTerm
# UTM court and write an `agenterm-defender-court` receipt.
#
# usage: scripts/utm-win-defender-court.sh CANDIDATE_DIR RECEIPT_PATH
#
# CANDIDATE_DIR is a downloaded, extracted `release-candidate-<run>` artifact:
# it must contain the sealed manifest and a payload/ directory holding the exact
# Windows archives. RECEIPT_PATH is where this court writes its receipt; feed
# that receipt to scripts/agenterm-reputation-court.py qualify.
#
# This is AgenTerm's own court caller. It resolves the product-neutral utm-court
# CLI exactly the way scripts/utm-cu-managed-job-court.sh does, leases and
# releases its own VM, and never routes through another product's runner
# scripts. Defender scans statically, so a native ARM Windows court is a valid
# scanner for x86_64 archives and is far more reliable than the emulated x86
# guest on Apple Silicon -- hence the default.

set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/utm-win-defender-court.sh CANDIDATE_DIR RECEIPT_PATH

Scans the sealed Windows Candidate archives with Microsoft Defender inside a
Windows UTM court and writes an agenterm-defender-court receipt.

Environment:
  AGENTERM_DEFENDER_COURT  win-aarch64-desktop (default) or win-x86_64-desktop
  UTM_COURT_CLI            explicit path to the utm-court CLI
  UTM_COURT_HOME           directory containing bin/utm-court
EOF
}

[ "$#" -eq 2 ] || { usage >&2; exit 2; }
CANDIDATE_DIR="$1"
RECEIPT_PATH="$2"
COURT="${AGENTERM_DEFENDER_COURT:-win-aarch64-desktop}"
case "$COURT" in
  win-aarch64-desktop | win-x86_64-desktop) ;;
  *) echo "unsupported Defender court: $COURT" >&2; exit 2 ;;
esac
if [ -n "${AGENTERM_DEFENDER_VERDICT+x}" ]; then
  echo "the Defender verdict is machine-derived; remove AGENTERM_DEFENDER_VERDICT" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
case "$CANDIDATE_DIR" in /*) ;; *) CANDIDATE_DIR="$REPO_ROOT/$CANDIDATE_DIR" ;; esac
case "$RECEIPT_PATH" in /*) ;; *) RECEIPT_PATH="$REPO_ROOT/$RECEIPT_PATH" ;; esac
[ -d "$CANDIDATE_DIR" ] || { echo "candidate directory is missing" >&2; exit 2; }

MANIFEST="$(
  find "$CANDIDATE_DIR" -maxdepth 2 -name '*-candidate-manifest.json' -print |
    LC_ALL=C sort | head -n1
)"
[ -n "$MANIFEST" ] || { echo "sealed candidate manifest is missing" >&2; exit 2; }

# The manifest is the authority for which bytes must be scanned. Never glob the
# payload directory: an extra or renamed archive must be a court failure, not a
# silently wider scan.
SELECTION="$(python3 - "$MANIFEST" "$CANDIDATE_DIR" <<'PY'
import json, pathlib, sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8-sig"))
root = pathlib.Path(sys.argv[2])
rows = []
for asset in manifest.get("assets", []):
    if asset.get("os") != "windows":
        continue
    name = asset["name"]
    matches = [path for path in root.rglob(name) if path.is_file()]
    if len(matches) != 1:
        sys.exit(f"expected exactly one {name} under the candidate, found {len(matches)}")
    rows.append((f"windows-{asset['arch']}", asset["sha256"], str(matches[0])))
if len(rows) != 2:
    sys.exit(f"expected two Windows archives in the sealed manifest, found {len(rows)}")
print(manifest["source_sha"])
print(manifest["version"])
print(manifest["run"]["id"])
print(manifest["run"]["attempt"])
for platform_id, digest, path in sorted(rows):
    print(f"{platform_id}\t{digest}\t{path}")
PY
)"
SOURCE_SHA="$(printf '%s\n' "$SELECTION" | sed -n '1p')"
VERSION="$(printf '%s\n' "$SELECTION" | sed -n '2p')"
CANDIDATE_RUN_ID="$(printf '%s\n' "$SELECTION" | sed -n '3p')"
CANDIDATE_RUN_ATTEMPT="$(printf '%s\n' "$SELECTION" | sed -n '4p')"
ASSET_ROWS="$(printf '%s\n' "$SELECTION" | sed -n '5,$p')"
[ -n "$ASSET_ROWS" ] || { echo "no Windows archives selected" >&2; exit 2; }

is_real_court_cli() {
  [ -n "$1" ] && [ -x "$1" ] && [ -f "$1" ] &&
    grep -q 'Uniform, product-neutral lifecycle' "$1" 2>/dev/null
}

resolve_court_cli() {
  for candidate in \
    "${UTM_COURT_CLI:-}" \
    "${UTM_COURT_HOME:-}/bin/utm-court" \
    "$REPO_ROOT/../utm-court/bin/utm-court" \
    "$HOME/repos/utm-court/bin/utm-court"; do
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

ready_timeout=180
interactive_timeout=360
result_timeout=900
if [ "$COURT" = win-x86_64-desktop ]; then
  # Fully emulated on Apple Silicon: every phase needs a wider budget.
  ready_timeout=600
  interactive_timeout=600
  result_timeout=1800
fi

echo "Defender court phase: lease $COURT"
UTM_COURT_CLEANUP_READY_TIMEOUT="${UTM_COURT_CLEANUP_READY_TIMEOUT:-$ready_timeout}" \
  "$COURT_CLI" lease "$COURT" --disposable >/dev/null
LEASED=1
echo "Defender court phase: transport-ready ${ready_timeout}s"
"$COURT_CLI" wait-ready "$COURT" "$ready_timeout" >/dev/null
# Windows courts are driven through the login-session job agent. utm-court's
# direct interactive execution verb is Linux-only and answers a Windows court
# with "currently Linux-only". This is the same job.pending.ps1 / job.ready
# protocol that scripts/utm-cu-managed-job-court.sh uses for its Windows cells,
# and it stays the primary path.
#
# It is not, however, guaranteed to be running on a disposable court: on
# win-aarch64-desktop the detached session agent did not claim its nonce within
# 360s, which blocked the reputation gate end to end. Defender scans files
# statically -- MpCmdRun reads the bytes and never executes them -- so the scan
# carries no console, desktop or job-object semantics and stays valid when it is
# driven through the guest agent instead. The session-0 caution that governs the
# PTY and managed-job courts does not transfer to a static file scan.
#
# So: try the login session, fall back to the guest agent, and record which path
# produced the verdict in the receipt. A silent fallback would hide a host
# denial inside a clean result; a named one does not.
echo "Defender court phase: interactive-ready ${interactive_timeout}s"
SCAN_PATH=login-session
if ! "$COURT_CLI" interactive-ready "$COURT" "$interactive_timeout" \
    >"$SCRATCH/interactive-ready.json" 2>"$SCRATCH/interactive-ready.err"; then
  SCAN_PATH=guest-agent
  echo "Defender court phase: login-session agent unavailable, scanning through the guest agent"
  sed 's/^/  /' "$SCRATCH/interactive-ready.err" >&2 || true
fi
AGENT_ROOT="$("$COURT_CLI" windows-agent-root)"
JOB="$AGENT_ROOT\\job.pending.ps1"
READY_FLAG="$AGENT_ROOT\\job.ready"

# A freshly leased court answers its first guest-agent calls with transient
# host errors (OSStatus -10004 / -2700) until the channel settles. Retry rather
# than failing the whole court on a startup race.
court_retry() {
  _attempt=1
  until "$COURT_CLI" "$@"; do
    [ "$_attempt" -ge 5 ] && return 1
    _attempt=$((_attempt + 1))
    sleep 8
  done
  return 0
}

RUN_ID="${SOURCE_SHA:0:12}-$COURT-$$-$RANDOM"
PREFIX="$WINDOWS_ROOT\\agenterm-defender-$RUN_ID"
GUEST_REPORT="$PREFIX-report.json"
GUEST_EXIT="$PREFIX.exit"
GUEST_LOG="$PREFIX.log"
LOCAL_REPORT="$SCRATCH/report.json"
LOCAL_EXIT="$SCRATCH/run.exit"

PUSHED_SPEC="$SCRATCH/assets.tsv"
: >"$PUSHED_SPEC"
while IFS=$'\t' read -r platform_id digest path; do
  [ -n "$platform_id" ] || continue
  actual="$(shasum -a 256 "$path" | awk '{print $1}')"
  if [ "$actual" != "$digest" ]; then
    echo "local archive does not match the sealed manifest: $platform_id" >&2
    exit 1
  fi
  guest_file="$PREFIX-$platform_id.zip"
  echo "Defender court phase: payload-transfer $platform_id"
  UTM_COURT_TRANSFER_TIMEOUT=300 \
    court_retry push "$COURT" "$path" "$guest_file"
  printf '%s\t%s\t%s\n' "$platform_id" "$digest" "$guest_file" >>"$PUSHED_SPEC"
done <<<"$ASSET_ROWS"

# The scan runs with -DisableRemediation so a detection is reported rather than
# silently quarantined, and the post-scan hash proves the bytes were untouched.
SCAN_SCRIPT="$SCRATCH/scan.ps1"
{
  printf '%s\n' '$ErrorActionPreference = "Stop"'
  printf "\$spec = '%s'\n" "$PREFIX-assets.tsv"
  printf "\$report = '%s'\n" "$GUEST_REPORT"
  cat <<'PS1'
$programFiles = [Environment]::GetFolderPath('ProgramFiles')
$mp = Join-Path $programFiles 'Windows Defender\MpCmdRun.exe'
if (-not (Test-Path -LiteralPath $mp)) {
  $platform = Join-Path $env:ProgramData 'Microsoft\Windows Defender\Platform'
  $mp = (Get-ChildItem -Path $platform -Filter MpCmdRun.exe -Recurse -ErrorAction Stop |
    Sort-Object FullName -Descending | Select-Object -First 1).FullName
}
if (-not $mp) { throw 'MpCmdRun.exe was not found' }
$status = Get-MpComputerStatus
$rows = @()
foreach ($line in Get-Content -LiteralPath $spec) {
  if ([string]::IsNullOrWhiteSpace($line)) { continue }
  $parts = $line -split "`t", 3
  if ($parts.Count -ne 3) { throw 'invalid asset row' }
  $platformId = $parts[0]
  $expected = $parts[1]
  $file = $parts[2]
  $before = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
  if ($before -ne $expected) { throw "pushed archive digest drift for $platformId" }
  $output = & $mp -Scan -ScanType 3 -File $file -DisableRemediation 2>&1 | Out-String
  $code = $LASTEXITCODE
  $after = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
  # MpCmdRun exit 0 = no threat, 2 = threat found, anything else = scan error.
  if ($code -ne 0) { throw "Defender scan of $platformId returned $code`n$output" }
  $rows += [ordered]@{
    platform_id       = $platformId
    sha256            = $before
    post_scan_sha256  = $after
    threats_detected  = 0
    scan_exit_code    = $code
  }
}
$payload = [ordered]@{
  scanner = [ordered]@{
    product_version   = $status.AMProductVersion
    signature_version = $status.AntivirusSignatureVersion
    engine_version    = $status.AMEngineVersion
  }
  assets = $rows
}
[IO.File]::WriteAllText($report, ($payload | ConvertTo-Json -Depth 6), (New-Object Text.UTF8Encoding $false))
PS1
} >"$SCAN_SCRIPT"

court_retry push "$COURT" "$PUSHED_SPEC" "$PREFIX-assets.tsv"
court_retry push "$COURT" "$SCAN_SCRIPT" "$PREFIX-scan.ps1"

# The wrapper records the scan's exit code as a guest file, so the receipt-
# bearing contract below is identical no matter how the wrapper was launched.
RUNNER="$SCRATCH/run.ps1"
printf '%s\n' \
  '$ErrorActionPreference = "Stop"' \
  "\$scan = '$PREFIX-scan.ps1'" \
  "\$log = '$GUEST_LOG'" \
  "\$result = '$GUEST_EXIT'" \
  '$resultTmp = $result + ".tmp"' \
  '$exitCode = 1' \
  'try {' \
  '  & powershell -NoProfile -ExecutionPolicy Bypass -File $scan *> $log' \
  '  $exitCode = $LASTEXITCODE' \
  '} catch {' \
  '  $_ | Out-String | Add-Content -LiteralPath $log' \
  '  $exitCode = 1' \
  '} finally {' \
  '  [IO.File]::WriteAllText($resultTmp, [string]$exitCode)' \
  '  Move-Item -LiteralPath $resultTmp -Destination $result -Force' \
  '}' \
  'exit $exitCode' >"$RUNNER"
if [ "$SCAN_PATH" = login-session ]; then
  court_retry push "$COURT" "$RUNNER" "$JOB"
  printf ready | court_retry push "$COURT" - "$READY_FLAG"
else
  court_retry push "$COURT" "$RUNNER" "$PREFIX-run.ps1"
  court_retry exec "$COURT" -- \
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PREFIX-run.ps1" \
    >/dev/null 2>&1 || true
fi

echo "Defender court phase: scan (${result_timeout}s)"
deadline=$((SECONDS + result_timeout))
while :; do
  : >"$LOCAL_EXIT"
  "$COURT_CLI" pull "$COURT" "$GUEST_EXIT" "$LOCAL_EXIT" >/dev/null 2>&1 || true
  [ -s "$LOCAL_EXIT" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    "$COURT_CLI" pull "$COURT" "$GUEST_LOG" "$SCRATCH/scan.log" >/dev/null 2>&1 &&
      cat "$SCRATCH/scan.log" >&2 || true
    echo "Defender scan timed out after ${result_timeout}s" >&2
    exit 1
  fi
  sleep 2
done
RUN_RC="$(tr -d '\r\n ' <"$LOCAL_EXIT")"
case "$RUN_RC" in '' | *[!0-9]*) echo "invalid guest exit receipt" >&2; exit 1 ;; esac
if [ "$RUN_RC" -ne 0 ]; then
  "$COURT_CLI" pull "$COURT" "$GUEST_LOG" "$SCRATCH/scan.log" >/dev/null 2>&1 &&
    cat "$SCRATCH/scan.log" >&2 || true
  echo "Defender scan failed in the guest with exit $RUN_RC" >&2
  exit 1
fi
"$COURT_CLI" pull "$COURT" "$GUEST_REPORT" "$LOCAL_REPORT"

# PowerShell 5 redirection and Windows tooling can emit UTF-16/BOM text; the
# receipt is a byte protocol, so normalize only this pulled copy.
python3 - "$LOCAL_REPORT" <<'PY'
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

python3 - "$LOCAL_REPORT" "$RECEIPT_PATH" "$SOURCE_SHA" "$VERSION" \
  "$CANDIDATE_RUN_ID" "$CANDIDATE_RUN_ATTEMPT" "$COURT" "$SCAN_PATH" <<'PY'
import datetime, json, os, pathlib, sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
assets = report["assets"]
if isinstance(assets, dict):
    assets = [assets]
if not assets:
    sys.exit("Defender court scanned nothing")
for row in assets:
    if row["sha256"] != row["post_scan_sha256"]:
        sys.exit(f"Defender altered the bytes of {row['platform_id']}")
    if row.get("threats_detected"):
        sys.exit(f"Defender reported a detection on {row['platform_id']}")
receipt = {
    "schema_version": 1,
    "kind": "agenterm-defender-court",
    "source_sha": sys.argv[3],
    "version": sys.argv[4],
    "candidate_run": {"id": int(sys.argv[5]), "attempt": int(sys.argv[6])},
    "court": sys.argv[7],
    # Which guest path drove the scan: a fallback must be legible in the
    # receipt, never folded into a clean verdict.
    "scan_path": sys.argv[8],
    "scanner": report["scanner"],
    "verdict": "clean",
    "assets": sorted(assets, key=lambda row: row["platform_id"]),
    "completed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
output = pathlib.Path(sys.argv[2])
output.parent.mkdir(parents=True, exist_ok=True)
temporary = output.with_name(output.name + ".tmp")
temporary.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
os.replace(temporary, output)
print(f"CLEAN agenterm Defender receipt assets={len(assets)} court={sys.argv[7]}")
PY

# The lease is --disposable, so the guest is restored on release; the pushed
# archives do not outlive this invocation and need no explicit guest cleanup.
