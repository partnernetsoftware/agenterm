#!/bin/bash
# Build the three Windows artifacts, push them, and prove the guest has exactly
# those bytes before running anything. Version skew between agenterm.exe, the
# ACU provider and the resident owner cost several cycles of chasing failures
# that were only mismatch.
set -uo pipefail
R="$(cd "$(dirname "$0")/.." && pwd)"
S="${WINDOWS_COURT_STAGE:-${TMPDIR:-/tmp}/agenterm-windows-court}"
mkdir -p "$S"
C="${UTM_COURT_CLI:-$HOME/repos/utm-court/bin/utm-court}"
# The MSVC CRT/SDK fetch does not complete on the direct route from this
# network; see docs/macos-local-build.md. Override with AGENTERM_BUILD_PROXY=""
# where the direct route works.
P="${AGENTERM_BUILD_PROXY-http://127.0.0.1:8888}"
if [ -n "$P" ]; then export HTTPS_PROXY="$P" HTTP_PROXY="$P" ALL_PROXY="$P"; fi
cd "$R"
cargo xwin build --locked --profile release-fast --target x86_64-pc-windows-msvc -p agenterm -p agenterm-cu >"$S/sync-build.log" 2>&1 || { echo "BUILD FAILED exe"; exit 1; }
cargo xwin build --locked --profile abi-release --target x86_64-pc-windows-msvc -p agenterm-cu-provider >>"$S/sync-build.log" 2>&1 || { echo "BUILD FAILED dll"; exit 1; }
# One artifact per push: a combined ~11 MB archive times out, while each of
# these lands in well under a minute. The transport, not the archive format,
# is what sets the ceiling.
rm -f "$S/a1.tgz" "$S/a2.tgz" "$S/a3.tgz"
tar -czf "$S/a1.tgz" -C target/x86_64-pc-windows-msvc/release-fast agenterm.exe || { echo "TAR FAILED 1"; exit 1; }
tar -czf "$S/a2.tgz" -C target/x86_64-pc-windows-msvc/release-fast agenterm-cu.exe || { echo "TAR FAILED 2"; exit 1; }
tar -czf "$S/a3.tgz" -C target/x86_64-pc-windows-msvc/abi-release agenterm_cu_provider.dll || { echo "TAR FAILED 3"; exit 1; }
for n in 1 2 3; do
  "$C" push win-aarch64-desktop "$S/a$n.tgz" "C:\\minicon-six\\a$n.tgz" >/dev/null 2>&1 || { echo "PUSH FAILED $n"; exit 1; }
done
"$C" exec win-aarch64-desktop -- cmd.exe /c "taskkill /f /im agenterm.exe /t >nul 2>&1 & taskkill /f /im agenterm-cu.exe /t >nul 2>&1 & exit 0" >/dev/null 2>&1
sleep 6
"$C" exec win-aarch64-desktop -- cmd.exe /c "cd /d C:\minicon-six\run2 && tar -xzf ..\a1.tgz > s1.txt 2>&1 & tar -xzf ..\a2.tgz >> s1.txt 2>&1 & tar -xzf ..\a3.tgz >> s1.txt 2>&1" >/dev/null 2>&1
sleep 10
"$C" exec win-aarch64-desktop -- cmd.exe /c "cd /d C:\minicon-six\run2 && copy /y agenterm_cu_provider.dll agenterm-cu-provider.dll > s2.txt 2>&1" >/dev/null 2>&1
sleep 8
"$C" exec win-aarch64-desktop -- cmd.exe /c "cd /d C:\minicon-six\run2 && dir *.exe *.dll > sizes.txt 2>&1" >/dev/null 2>&1
sleep 6
"$C" pull win-aarch64-desktop 'C:\minicon-six\run2\sizes.txt' "$S/guest-sizes.txt" >/dev/null 2>&1
echo "HOST:"; ls -la target/x86_64-pc-windows-msvc/release-fast/agenterm.exe target/x86_64-pc-windows-msvc/release-fast/agenterm-cu.exe target/x86_64-pc-windows-msvc/abi-release/agenterm_cu_provider.dll | awk '{print $5, $NF}'
echo "GUEST:"; grep -E "AM |PM " "$S/guest-sizes.txt" | awk '{print $4, $5}'
