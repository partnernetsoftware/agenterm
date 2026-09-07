#!/usr/bin/env bash
# Deterministically stage the non-secret SMAppService app layout. This script
# never signs, installs, registers, authorizes, or invokes the helper.
set -euo pipefail

ARCH="${1:?ARCH required}"
BIN_DIR="${2:?BIN_DIR required}"
OUTPUT="${3:?OUTPUT bundle required}"
VERSION="${4:?VERSION required}"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
ASSETS="$ROOT/packaging/privilege/macos"

case "$ARCH" in aarch64|x86_64) ;; *) echo "unsupported macOS architecture: $ARCH" >&2; exit 2;; esac
case "$VERSION" in *[!0-9A-Za-z.-]*|'') echo "invalid bundle version" >&2; exit 2;; esac
[[ "$(basename "$OUTPUT")" == "AgenTerm.app" ]] || {
  echo "output must be the fixed AgenTerm.app bundle" >&2; exit 2;
}
case "$OUTPUT" in
  /Applications/*|/System/*|/Library/*)
    echo "staging never writes an installed system location" >&2
    exit 2
    ;;
esac
for name in agenterm agenterm-cc agenterm-cu libagenterm.dylib; do
  [[ -f "$BIN_DIR/$name" && ! -L "$BIN_DIR/$name" ]] || {
    echo "missing regular macOS artifact: $BIN_DIR/$name" >&2
    exit 1
  }
done
"$ROOT/packaging/verify-cu-abi.sh" \
  "$BIN_DIR/agenterm-cu" "$BIN_DIR/libagenterm.dylib"

NEXT="${OUTPUT}.next.$$"
rm -rf "$NEXT"
trap 'rm -rf "$NEXT"' EXIT
mkdir -p "$NEXT/Contents/MacOS" "$NEXT/Contents/Resources" "$NEXT/Contents/Library/LaunchDaemons"
sed "s/@AGENTERM_VERSION@/$VERSION/g" "$ASSETS/Info.plist.in" > "$NEXT/Contents/Info.plist"
cp "$ASSETS/com.partnernetsoftware.agenterm.cu.privilege.plist" \
  "$NEXT/Contents/Library/LaunchDaemons/com.partnernetsoftware.agenterm.cu.privilege.plist"
cp "$ASSETS/authorization-right.plist" "$NEXT/Contents/Resources/authorization-right.plist"
cp "$ASSETS/deployment.json" "$NEXT/Contents/Resources/privilege-deployment.json"
cp "$BIN_DIR/agenterm" "$NEXT/Contents/MacOS/agenterm"
cp "$BIN_DIR/agenterm-cc" "$NEXT/Contents/MacOS/agenterm-cc"
cp "$BIN_DIR/agenterm-cu" "$NEXT/Contents/MacOS/agenterm-cu"
cp "$BIN_DIR/libagenterm.dylib" "$NEXT/Contents/MacOS/libagenterm.dylib"
cp "$BIN_DIR/agenterm-cu" \
  "$NEXT/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege"
chmod 0755 "$NEXT/Contents/MacOS/agenterm" "$NEXT/Contents/MacOS/agenterm-cc" \
  "$NEXT/Contents/MacOS/agenterm-cu" \
  "$NEXT/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege"
chmod 0644 "$NEXT/Contents/MacOS/libagenterm.dylib" "$NEXT/Contents/Info.plist" \
  "$NEXT/Contents/Library/LaunchDaemons/com.partnernetsoftware.agenterm.cu.privilege.plist" \
  "$NEXT/Contents/Resources/authorization-right.plist" \
  "$NEXT/Contents/Resources/privilege-deployment.json"
/usr/bin/plutil -lint "$NEXT/Contents/Info.plist" >/dev/null
/usr/bin/plutil -lint \
  "$NEXT/Contents/Library/LaunchDaemons/com.partnernetsoftware.agenterm.cu.privilege.plist" >/dev/null
/usr/bin/plutil -lint "$NEXT/Contents/Resources/authorization-right.plist" >/dev/null
rm -rf "$OUTPUT"
mv "$NEXT" "$OUTPUT"
trap - EXIT
