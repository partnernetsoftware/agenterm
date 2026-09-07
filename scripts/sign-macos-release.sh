#!/usr/bin/env bash
# Sign every macOS release dynamic library and executable with a Developer ID
# Application identity. Libraries go first so the final executable signatures
# bind an already signed runtime payload.
#
# Usage: sign-macos-release.sh ARCH BIN_DIR
# Required environment: APPLE_SIGNING_IDENTITY
set -euo pipefail

ARCH="${1:?ARCH required}"
BIN_DIR="${2:?BIN_DIR required}"
IDENTITY="${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY required}"
PYTHON="${PYTHON:-python3}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/scripts/artifacts.json"
APP_BUNDLE="$BIN_DIR/AgenTerm.app"
APP_IDENTIFIER="com.partnernetsoftware.agenterm"
HELPER_IDENTIFIER="com.partnernetsoftware.agenterm.cu.privilege"
HELPER="$APP_BUNDLE/Contents/Resources/$HELPER_IDENTIFIER"
PROVIDER="$APP_BUNDLE/Contents/MacOS/agenterm-cu-provider.dylib"
TEAM_ID="${AGENTERM_APPLE_TEAM_ID:?AGENTERM_APPLE_TEAM_ID required at build and signing time}"

if [[ ! "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]]; then
  echo "AGENTERM_APPLE_TEAM_ID must be a 10-character Apple Team identifier." >&2
  exit 1
fi

if [[ ! -d "$BIN_DIR" ]]; then
  echo "Binary directory not found: $BIN_DIR" >&2
  exit 1
fi

ARTIFACT_NAMES=()
while IFS= read -r name; do
  name="${name%$'\r'}"
  ARTIFACT_NAMES+=("$name")
done < <(
  "$PYTHON" - "$MANIFEST" "$ARCH" <<'PY'
import json
import sys

manifest_path, arch = sys.argv[1:3]
with open(manifest_path, encoding="utf-8") as handle:
    manifest = json.load(handle)

matches = [
    platform
    for platform in manifest.get("platforms", [])
    if platform.get("os") == "macos" and platform.get("arch") == arch
]
if len(matches) != 1:
    sys.exit(f"Expected one macos-{arch} manifest entry, found {len(matches)}")
for artifact in matches[0].get("libraries", []) + matches[0].get("executables", []):
    name = artifact.get("name")
    if not name:
        sys.exit("macOS artifact entry is missing name")
    print(name)
PY
)

if [[ ${#ARTIFACT_NAMES[@]} -eq 0 ]]; then
  echo "No macOS artifacts listed for architecture $ARCH" >&2
  exit 1
fi

for name in "${ARTIFACT_NAMES[@]}"; do
  path="$BIN_DIR/$name"
  if [[ ! -f "$path" ]]; then
    echo "Missing macOS artifact: $path" >&2
    exit 1
  fi
  codesign \
    --force \
    --sign "$IDENTITY" \
    --options runtime \
    --timestamp \
    "$path"
  codesign --verify --strict --verbose=2 "$path"
done

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
"$ROOT/packaging/privilege/macos/stage-app-bundle.sh" \
  "$ARCH" "$BIN_DIR" "$APP_BUNDLE" "$VERSION"

# Sign every nested code object before sealing the outer app. The helper gets
# its fixed identifier; no certificate or Team identifier is stored in source.
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  "$APP_BUNDLE/Contents/MacOS/libagenterm.dylib"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  "$PROVIDER"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --identifier "$HELPER_IDENTIFIER" \
  --entitlements "$ROOT/packaging/privilege/macos/helper.entitlements" \
  "$HELPER"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --identifier "com.partnernetsoftware.agenterm.cu" \
  "$APP_BUNDLE/Contents/MacOS/agenterm-cu"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --identifier "com.partnernetsoftware.agenterm.cc" \
  "$APP_BUNDLE/Contents/MacOS/agenterm-cc"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --identifier "$APP_IDENTIFIER" \
  "$APP_BUNDLE/Contents/MacOS/agenterm"
codesign --force --sign "$IDENTITY" --options runtime --timestamp \
  --identifier "$APP_IDENTIFIER" \
  --entitlements "$ROOT/packaging/privilege/macos/app.entitlements" \
  "$APP_BUNDLE"
"$ROOT/packaging/privilege/macos/validate-app-bundle.sh" --signed-bundle "$APP_BUNDLE"

echo "==> signed ${#ARTIFACT_NAMES[@]} flat macOS artifact(s) and AgenTerm.app for $ARCH"
