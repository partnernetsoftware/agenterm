#!/bin/bash
# Stage six-cell release payloads and compile the experimental APE launcher.
# cosmocc → dist/agenterm-ape.com ; else host cc → dist/agenterm-ape-loader
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BUILD="${AGENTERM_SIX_CELL_BUILD:-$ROOT/target}"
COSMO="${COSMOCC_DIR:-$HOME/cosmocc}"
DIST="$HERE/dist"
CELLS="$DIST/cells"

artifact_dir() {
  target="$1"
  profile="$2"
  leaf="release"
  if [ "$profile" = "dev" ]; then leaf="debug"; fi
  echo "$BUILD/$target/$leaf"
}

stage() {
  cell="$1"
  src="$2"
  leaf="$3"
  mkdir -p "$CELLS/$cell"
  if [[ ! -f "$src" ]]; then
    echo "missing $src" >&2
    exit 1
  fi
  cp "$src" "$CELLS/$cell/$leaf"
}

PROFILE="${AGENTERM_APE_PROFILE:-release-fast}"
case "$PROFILE" in
  dev|release-fast|release) ;;
  *) echo "invalid AGENTERM_APE_PROFILE: $PROFILE" >&2; exit 2 ;;
esac

rm -rf "$DIST"
mkdir -p "$CELLS"

stage osx-aarch64 "$(artifact_dir aarch64-apple-darwin "$PROFILE")/agenterm" agenterm
stage osx-x86_64 "$(artifact_dir x86_64-apple-darwin "$PROFILE")/agenterm" agenterm
stage lnx-aarch64 "$(artifact_dir aarch64-unknown-linux-gnu "$PROFILE")/agenterm" agenterm
stage lnx-x86_64 "$(artifact_dir x86_64-unknown-linux-gnu "$PROFILE")/agenterm" agenterm
stage win-aarch64 "$(artifact_dir aarch64-pc-windows-msvc "$PROFILE")/agenterm.exe" agenterm.exe
stage win-x86_64 "$(artifact_dir x86_64-pc-windows-msvc "$PROFILE")/agenterm.exe" agenterm.exe

if [[ -x "$COSMO/bin/cosmocc" ]]; then
  echo "[pack] cosmocc → dist/agenterm-ape.com"
  set +e
  cosmo_log=$("$COSMO/bin/cosmocc" -Os -static -o "$DIST/agenterm-ape.com" "$HERE/loader.c" 2>&1)
  cosmo_rc=$?
  set -e
  if [[ "$cosmo_rc" -ne 0 ]]; then
    printf '%s\n' "$cosmo_log" >&2
    exit "$cosmo_rc"
  fi
  chmod 0755 "$DIST/agenterm-ape.com"
  (
    cd "$DIST"
    zip -q -r agenterm-ape.com cells
    zip -A agenterm-ape.com
  )
  sha256sum "$DIST/agenterm-ape.com" | awk '{print $1}' >"$DIST/agenterm-ape.com.sha256"
  echo "[pack] zip overlay cells/ + zipalign"
else
  echo "[pack] no cosmocc; host-cc dispatcher"
  cc -O2 -o "$DIST/agenterm-ape-loader" "$HERE/loader.c"
fi

echo "[pack] staged:"
find "$CELLS" -type f -exec ls -lh {} \;
echo "[pack] OK $DIST"
