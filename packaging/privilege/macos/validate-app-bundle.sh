#!/usr/bin/env bash
# Static deployment court. --layout validates unsigned rehearsal structure;
# --signed-bundle additionally requires non-ad-hoc Developer ID signatures and
# a shared Team identifier. It never installs, registers, authorizes, or runs.
set -euo pipefail

MODE="${1:?--layout or --signed-bundle required}"
BUNDLE="${2:?bundle required}"
case "$MODE" in --layout|--signed-bundle) ;; *) echo "invalid validation mode" >&2; exit 2;; esac
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
ASSETS="$ROOT/packaging/privilege/macos"
MANIFEST="$ASSETS/deployment.json"
PLIST="$BUNDLE/Contents/Library/LaunchDaemons/com.partnernetsoftware.agenterm.cu.privilege.plist"
HELPER="$BUNDLE/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege"
PROVIDER="$BUNDLE/Contents/MacOS/agenterm-cu-provider.dylib"

python3 - "$MANIFEST" "$BUNDLE" "$PLIST" "$HELPER" "$PROVIDER" <<'PY'
import json, os, plistlib, stat, sys
manifest_path, bundle, plist_path, helper, provider = sys.argv[1:]
with open(manifest_path, encoding="utf-8") as stream:
    m = json.load(stream)
if m.get("schema_version") != 1:
    raise SystemExit("macos_provider_manifest_schema")
if m.get("install_authority") != "root-owned-package-only" or m.get("ordinary_drag_install_supported") is not False:
    raise SystemExit("macos_provider_install_authority")
expected = {
    "bundle_name": "AgenTerm.app",
    "bundle_identifier": "com.partnernetsoftware.agenterm",
    "install_path": "/Applications/AgenTerm.app",
    "helper_label": "com.partnernetsoftware.agenterm.cu.privilege",
    "helper_relative": "Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege",
    "helper_installed": "/Applications/AgenTerm.app/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege",
    "right": "com.partnernetsoftware.agenterm.cu.privilege.process-signal",
    "socket_key": "SystemBroker",
    "socket_path": "/private/var/run/agenterm/cu-privilege.sock",
    "socket_mode": 0o666,
}
actual = {
    "bundle_name": m["app"]["bundle_name"],
    "bundle_identifier": m["app"]["bundle_identifier"],
    "install_path": m["app"]["install_path"],
    "helper_label": m["helper"]["label"],
    "helper_relative": m["helper"]["bundle_relative_path"],
    "helper_installed": m["helper"]["installed_path"],
    "right": m["authorization"]["right"],
    "socket_key": m["launchd"]["socket_key"],
    "socket_path": m["launchd"]["socket_path"],
    "socket_mode": m["launchd"]["socket_mode"],
}
if actual != expected:
    raise SystemExit("macos_provider_manifest_contract")
if "*" in expected["right"] or m["authorization"].get("rule") != "authenticate-admin":
    raise SystemExit("macos_provider_right_must_be_explicit")
if expected["right"] == expected["helper_label"]:
    raise SystemExit("macos_provider_right_must_be_operation_scoped")
if os.path.basename(bundle) != expected["bundle_name"]:
    raise SystemExit("macos_provider_bundle_name")
required = [
    "Contents/Info.plist", "Contents/MacOS/agenterm", "Contents/MacOS/agenterm-cc",
    "Contents/MacOS/agenterm-cu", "Contents/MacOS/libagenterm.dylib",
    "Contents/MacOS/agenterm-cu-provider.dylib",
    expected["helper_relative"], m["launchd"]["plist_relative_path"],
    "Contents/Resources/authorization-right.plist",
    "Contents/Resources/privilege-deployment.json",
]
for relative in required:
    path = os.path.join(bundle, relative)
    st = os.lstat(path)
    if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode):
        raise SystemExit("macos_provider_bundle_entry:" + relative)
if stat.S_IMODE(os.lstat(provider).st_mode) != 0o644:
    raise SystemExit("macos_provider_library_mode")
with open(os.path.join(bundle, "Contents/Resources/privilege-deployment.json"), encoding="utf-8") as stream:
    bundled_manifest = json.load(stream)
if bundled_manifest != m:
    raise SystemExit("macos_provider_bundled_manifest_drift")
for entitlement_name in ("app.entitlements", "helper.entitlements"):
    with open(os.path.join(os.path.dirname(manifest_path), entitlement_name), "rb") as stream:
        entitlements = plistlib.load(stream)
    if entitlements.get("com.apple.security.app-sandbox"):
        raise SystemExit("macos_provider_app_sandbox_unsupported")
with open(os.path.join(bundle, "Contents/Info.plist"), "rb") as stream:
    info = plistlib.load(stream)
if info.get("CFBundleIdentifier") != expected["bundle_identifier"]:
    raise SystemExit("macos_provider_app_identifier")
if info.get("LSMinimumSystemVersion") != "13.0":
    raise SystemExit("macos_provider_minimum_version")
with open(plist_path, "rb") as stream:
    launchd = plistlib.load(stream)
if launchd.get("Label") != expected["helper_label"]:
    raise SystemExit("macos_provider_launchd_label")
if launchd.get("BundleProgram") != expected["helper_relative"]:
    raise SystemExit("macos_provider_bundle_program")
if launchd.get("ProgramArguments") != [expected["helper_label"], m["helper"]["argument"]]:
    raise SystemExit("macos_provider_arguments")
if launchd.get("AssociatedBundleIdentifiers") != [expected["bundle_identifier"]]:
    raise SystemExit("macos_provider_associated_bundle")
if "ProcessType" in launchd:
    raise SystemExit("macos_provider_process_type_must_use_launchd_default")
sockets = launchd.get("Sockets", {})
if set(sockets) != {expected["socket_key"]}:
    raise SystemExit("macos_provider_socket_key")
socket = sockets[expected["socket_key"]]
if socket.get("SockPathName") != expected["socket_path"] or socket.get("SockPathMode") != expected["socket_mode"]:
    raise SystemExit("macos_provider_socket_contract")
with open(os.path.join(bundle, "Contents/Resources/authorization-right.plist"), "rb") as stream:
    right = plistlib.load(stream)
if right.get("class") != "user" or right.get("group") != "admin" or right.get("shared") is not False or right.get("timeout") != 0:
    raise SystemExit("macos_provider_right_rule")
if os.path.getsize(helper) != os.path.getsize(os.path.join(bundle, "Contents/MacOS/agenterm-cu")):
    raise SystemExit("macos_provider_helper_bytes")
with open(helper, "rb") as a, open(os.path.join(bundle, "Contents/MacOS/agenterm-cu"), "rb") as b:
    if a.read() != b.read():
        raise SystemExit("macos_provider_helper_bytes")
PY

for plist in "$BUNDLE/Contents/Info.plist" "$PLIST" \
  "$BUNDLE/Contents/Resources/authorization-right.plist"; do
  /usr/bin/plutil -lint "$plist" >/dev/null
done

if [[ "$MODE" == --layout ]]; then
  echo "MACOS_PROVIDER_LAYOUT_OK deployable=false"
  exit 0
fi

for path in "$HELPER" "$PROVIDER" "$BUNDLE"; do
  /usr/bin/codesign --verify --strict --verbose=2 "$path" >/dev/null 2>&1 || {
    echo "macos_provider_signature_invalid: $path" >&2
    exit 1
  }
done

signature_field() {
  local path="$1" field="$2"
  /usr/bin/codesign -d --verbose=4 "$path" 2>&1 | sed -n "s/^${field}=//p" | head -n 1
}
APP_TEAM="$(signature_field "$BUNDLE" TeamIdentifier)"
HELPER_TEAM="$(signature_field "$HELPER" TeamIdentifier)"
PROVIDER_TEAM="$(signature_field "$PROVIDER" TeamIdentifier)"
APP_AUTHORITY="$(signature_field "$BUNDLE" Authority)"
HELPER_AUTHORITY="$(signature_field "$HELPER" Authority)"
PROVIDER_AUTHORITY="$(signature_field "$PROVIDER" Authority)"
[[ "$APP_TEAM" =~ ^[A-Z0-9]{10}$ && "$APP_TEAM" == "$HELPER_TEAM" && \
  "$APP_TEAM" == "$PROVIDER_TEAM" ]] || {
  echo "macos_provider_team_mismatch" >&2; exit 1;
}
if [[ -n "${AGENTERM_APPLE_TEAM_ID:-}" && "$APP_TEAM" != "$AGENTERM_APPLE_TEAM_ID" ]]; then
  echo "macos_provider_expected_team_mismatch" >&2
  exit 1
fi
[[ "$APP_AUTHORITY" == "Developer ID Application:"* && \
  "$HELPER_AUTHORITY" == "Developer ID Application:"* && \
  "$PROVIDER_AUTHORITY" == "Developer ID Application:"* ]] || {
  echo "macos_provider_developer_id_required" >&2; exit 1;
}
python3 - "$MANIFEST" "$APP_TEAM" > "${BUNDLE}.requirements.tmp" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as stream:
    m = json.load(stream)
team = sys.argv[2]
for key in ("app_requirement_template", "helper_requirement_template"):
    print(m["signing"][key].replace("@APPLE_TEAM_ID@", team))
PY
trap 'rm -f "${BUNDLE}.requirements.tmp"' EXIT
APP_REQ="$(sed -n '1p' "${BUNDLE}.requirements.tmp")"
HELPER_REQ="$(sed -n '2p' "${BUNDLE}.requirements.tmp")"
/usr/bin/codesign -R="=$APP_REQ" --verify "$BUNDLE" >/dev/null 2>&1 || {
  echo "macos_provider_app_requirement_mismatch" >&2; exit 1;
}
/usr/bin/codesign -R="=$HELPER_REQ" --verify "$HELPER" >/dev/null 2>&1 || {
  echo "macos_provider_helper_requirement_mismatch" >&2; exit 1;
}
rm -f "${BUNDLE}.requirements.tmp"
trap - EXIT
echo "MACOS_PROVIDER_SIGNED_BUNDLE_OK team_consistent=true install_required=true"
