#!/usr/bin/env bash
# Run agenterm-cu's core observe surface on a real Windows guest.
#
# This is not the registered `cu-windows-smoke` journey (that one needs a
# GUI fixture compiled inside the guest). It is the recipe that actually
# executed the Windows adapter for the first time on 2026-09-01, kept so
# the result is reproducible rather than a claim in a commit message.
#
# It reaches Windows through the sibling minicon project's court driver,
# because that is where the entry point lives: the VM is registered with a
# **QEMU guest agent** adapter, which speaks over virtio-serial rather than
# any TCP service. Scanning for SSH / WinRM / SMB finds nothing and proves
# nothing.
#
# Two guest facts this encodes, both of which cost time to learn:
#   * The guest is Windows on ARM. Its registry says
#     PROCESSOR_ARCHITECTURE=ARM64 while the agent's own environment says
#     AMD64, because the agent is an emulated x64 process. Believe the
#     registry: the aarch64-pc-windows-msvc build is the right one.
#   * The agent runs in session 0, which has no desktop, so `windows`
#     returns an empty list there. The verbs are run through a one-shot
#     scheduled task in the interactive session instead.
set -euo pipefail

COURT="${COURT:-win-aarch64-desktop}"
MINICON="${MINICON:-$HOME/repos/minicon}"
DRIVER="$MINICON/scripts/utm-court.sh"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXE="$ROOT/target/aarch64-pc-windows-msvc/debug/agenterm-cu.exe"
DLL="$ROOT/target/aarch64-pc-windows-msvc/abi-dev/agenterm.dll"

if [[ ! -x "$DRIVER" ]]; then
  echo "SKIP: no court driver at $DRIVER; this host has no way into a Windows guest" >&2
  exit 0
fi
if [[ ! -f "$EXE" || ! -f "$DLL" ]]; then
  echo "Build the pair first:" >&2
  echo "  cargo xwin build --target aarch64-pc-windows-msvc -p agenterm-cu --bin agenterm-cu" >&2
  echo "  cargo xwin build --target aarch64-pc-windows-msvc -p agenterm-abi --profile abi-dev" >&2
  exit 2
fi

USER_NAME="${GUEST_USER:-minicon}"
say() { printf '== %s\n' "$*"; }

say "start $COURT and wait for its guest agent"
"$DRIVER" start "$COURT" >/dev/null
"$DRIVER" wait-ready "$COURT" 420 >/dev/null

say "push the cross-built pair"
"$DRIVER" exec "$COURT" -- cmd.exe /d /c "mkdir C:\\agt" >/dev/null 2>&1 || true
"$DRIVER" push "$COURT" "$EXE" 'C:\agt\agenterm-cu.exe'
"$DRIVER" push "$COURT" "$DLL" 'C:\agt\agenterm.dll'

say "install the payload that runs in the interactive session"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
cat >"$work/payload.ps1" <<'PS1'
$out = "C:\Windows\Temp\cu-court.txt"
$cu  = "C:\agt\agenterm-cu.exe"
"" | Out-File -Encoding utf8 $out
function Try-Run($label, $argv, $limitSec) {
  $tmp = [IO.Path]::GetTempFileName()
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $cu -ArgumentList $argv -NoNewWindow -PassThru -RedirectStandardOutput $tmp -RedirectStandardError "$tmp.err"
  if (-not $p.WaitForExit($limitSec * 1000)) {
    $ws = [math]::Round($p.WorkingSet64/1MB)
    try { $p.Kill() } catch {}
    ("{0,-16} HUNG after {1}s (working set {2} MB)" -f $label,$limitSec,$ws) | Out-File -Append -Encoding utf8 $out
  } else {
    $body = (Get-Content $tmp -Raw); if (-not $body) { $body = (Get-Content "$tmp.err" -Raw) }
    if ($body -and $body.Length -gt 240) { $body = $body.Substring(0,240) }
    ("{0,-16} {1}ms :: {2}" -f $label,$sw.ElapsedMilliseconds,($body -replace "`r`n"," ")) | Out-File -Append -Encoding utf8 $out
  }
  Remove-Item $tmp,"$tmp.err" -ErrorAction SilentlyContinue
}
Start-Process notepad.exe | Out-Null
Start-Sleep -Seconds 5
$raw = (& $cu --target current --grant observe windows --app notepad | Out-String)
$h = [regex]::Match($raw,'"handle":(\d+)').Groups[1].Value
if (-not $h) { "NO NOTEPAD WINDOW" | Out-File -Append -Encoding utf8 $out; exit 1 }
("notepad handle " + $h) | Out-File -Append -Encoding utf8 $out
Try-Run "windows"      @("--target","current","--grant","observe","windows") 30
Try-Run "tree"         @("--target","current","--grant","observe","tree","--window",$h,"--max-nodes","40") 30
Try-Run "focused"      @("--target","current","--grant","observe","focused","--window",$h) 30
Try-Run "menu inspect" @("--target","current","--grant","observe","menu","inspect","--window",$h) 30
Try-Run "screenshot"   @("--target","current","--grant","observe","screenshot","--out","C:\Windows\Temp\np.png","--window",$h) 30
Try-Run "capabilities" @("--target","current","--grant","observe","capabilities") 30
Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process -Force
PS1
printf '@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File C:\\Windows\\Temp\\payload.ps1\r\n' >"$work/payload.bat"
"$DRIVER" push "$COURT" "$work/payload.ps1" 'C:\Windows\Temp\payload.ps1'
"$DRIVER" push "$COURT" "$work/payload.bat" 'C:\Windows\Temp\payload.bat'

say "run it in the interactive session (the agent's own session has no desktop)"
"$DRIVER" exec "$COURT" -- cmd.exe /d /c \
  "schtasks /create /tn agtcourt /tr C:\\Windows\\Temp\\payload.bat /sc once /st 00:00 /ru $USER_NAME /it /f" >/dev/null 2>&1
"$DRIVER" exec "$COURT" -- cmd.exe /d /c "schtasks /run /tn agtcourt" >/dev/null 2>&1

say "collect"
for _ in $(seq 1 30); do
  sleep 5
  if "$DRIVER" pull "$COURT" 'C:\Windows\Temp\cu-court.txt' - 2>/dev/null | grep -q capabilities; then break; fi
done
"$DRIVER" pull "$COURT" 'C:\Windows\Temp\cu-court.txt' -
"$DRIVER" exec "$COURT" -- cmd.exe /d /c "schtasks /delete /tn agtcourt /f" >/dev/null 2>&1 || true

say "release the court"
"$DRIVER" release "$COURT" >/dev/null || true
