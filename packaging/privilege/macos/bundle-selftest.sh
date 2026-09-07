#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/agenterm-macos-provider.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"
for name in agenterm agenterm-cc agenterm-cu libagenterm.dylib; do
  printf 'fixture:%s\n' "$name" > "$TMP/bin/$name"
done
cat > "$TMP/bin/agenterm-cu" <<'EOF'
#!/bin/sh
test -f "${AGENTERM_ABI_LIB:?}" || exit 1
cat <<'JSON'
{"ok":true,"data":{"checks":{"abi":{"status":"available","detail":{"major":1,"minor":28,"required_major":1,"required_minor":28,"required_symbols":61}}}}}
JSON
EOF
chmod 0755 "$TMP/bin/agenterm" "$TMP/bin/agenterm-cc" "$TMP/bin/agenterm-cu"
"$ROOT/packaging/privilege/macos/stage-app-bundle.sh" \
  aarch64 "$TMP/bin" "$TMP/AgenTerm.app" 0.0.0-test
"$ROOT/packaging/privilege/macos/validate-app-bundle.sh" --layout "$TMP/AgenTerm.app"
if "$ROOT/packaging/privilege/macos/validate-app-bundle.sh" --signed-bundle "$TMP/AgenTerm.app" >/dev/null 2>&1; then
  echo "unsigned fixture was accepted as deployable" >&2
  exit 1
fi
# An ad-hoc signature is still a development identity. Seal the complete
# fixture to prove the deployable court rejects it rather than equating any
# cryptographic envelope with Developer ID.
codesign --force --sign - --identifier com.partnernetsoftware.agenterm.cu.privilege \
  "$TMP/AgenTerm.app/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege" >/dev/null
codesign --force --sign - "$TMP/AgenTerm.app/Contents/MacOS/libagenterm.dylib" >/dev/null
codesign --force --sign - "$TMP/AgenTerm.app/Contents/MacOS/agenterm-cu" >/dev/null
codesign --force --sign - "$TMP/AgenTerm.app/Contents/MacOS/agenterm-cc" >/dev/null
codesign --force --sign - --identifier com.partnernetsoftware.agenterm \
  "$TMP/AgenTerm.app/Contents/MacOS/agenterm" >/dev/null
codesign --force --sign - --identifier com.partnernetsoftware.agenterm "$TMP/AgenTerm.app" >/dev/null
if "$ROOT/packaging/privilege/macos/validate-app-bundle.sh" --signed-bundle "$TMP/AgenTerm.app" >/dev/null 2>&1; then
  echo "ad-hoc fixture was accepted as deployable" >&2
  exit 1
fi
python3 - "$TMP/AgenTerm.app/Contents/Library/LaunchDaemons/com.partnernetsoftware.agenterm.cu.privilege.plist" <<'PY'
import plistlib, sys
path = sys.argv[1]
with open(path, "rb") as stream:
    value = plistlib.load(stream)
value["Sockets"]["SystemBroker"]["SockPathName"] = "/tmp/unsafe.sock"
with open(path, "wb") as stream:
    plistlib.dump(value, stream)
PY
if "$ROOT/packaging/privilege/macos/validate-app-bundle.sh" --layout "$TMP/AgenTerm.app" >/dev/null 2>&1; then
  echo "drifted socket was accepted" >&2
  exit 1
fi
echo "PASS: macOS SMAppService bundle layout plus unsigned/ad-hoc fail-closed court"
