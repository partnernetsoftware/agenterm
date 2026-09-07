#!/usr/bin/env bash
# Build a signed, root-owned Installer component package around one already
# Developer-ID-signed and stapled AgenTerm.app. This script never installs,
# registers, authorizes, notarizes, or invokes code from the bundle.
set -euo pipefail
umask 077

usage() {
  echo "usage: $0 --bundle AgenTerm.app --output AgenTerm.pkg --installer-identity 'Developer ID Installer: ...'" >&2
  exit 2
}

fail() {
  echo "$1" >&2
  exit "${2:-1}"
}

BUNDLE=""
OUTPUT=""
IDENTITY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bundle)
      [[ -z "$BUNDLE" && $# -ge 2 ]] || usage
      BUNDLE="$2"
      shift 2
      ;;
    --output)
      [[ -z "$OUTPUT" && $# -ge 2 ]] || usage
      OUTPUT="$2"
      shift 2
      ;;
    --installer-identity)
      [[ -z "$IDENTITY" && $# -ge 2 ]] || usage
      IDENTITY="$2"
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ -n "$BUNDLE" && -n "$OUTPUT" && -n "$IDENTITY" ]] || usage

[[ "$IDENTITY" =~ ^Developer\ ID\ Installer:\ .+\ \([A-Z0-9]{10}\)$ ]] ||
  fail "installer_identity_must_be_explicit_developer_id_installer" 2
case "$IDENTITY" in *$'\n'*|*$'\r'*|*$'\t'*) fail "installer_identity_contains_control_character" 2 ;; esac
[[ -d "$BUNDLE" && ! -L "$BUNDLE" && "$(/usr/bin/basename "$BUNDLE")" == "AgenTerm.app" ]] ||
  fail "bundle_must_be_regular_AgenTerm_app" 2
[[ "$(/usr/bin/basename "$OUTPUT")" == *.pkg ]] || fail "output_must_end_in_pkg" 2
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || fail "output_already_exists"
OUTPUT_DIR="$(/usr/bin/dirname "$OUTPUT")"
[[ -d "$OUTPUT_DIR" && ! -L "$OUTPUT_DIR" ]] || fail "output_parent_must_be_a_real_directory" 2

ROOT="$(cd "$(/usr/bin/dirname "$0")/../../.." && pwd)"
ASSETS="$ROOT/packaging/privilege/macos"
COMPONENT_PLIST="$ASSETS/component.plist"
DEPLOYMENT="$ASSETS/deployment.json"
PYTHON=/usr/bin/python3

# Fixture injection is deliberately incapable of publishing an artifact. It is
# available only to pkg-selftest.sh, and a successful fixture run deletes its
# candidate and reports deployable=false.
SELFTEST="${AGENTERM_PKG_SELFTEST:-0}"
if [[ "$SELFTEST" == 0 ]]; then
  VALIDATE="$ASSETS/validate-app-bundle.sh"
  XCRUN=/usr/bin/xcrun
  PKGBUILD=/usr/bin/pkgbuild
  PRODUCTSIGN=/usr/bin/productsign
  PKGUTIL=/usr/sbin/pkgutil
  LSBOM=/usr/bin/lsbom
  CODESIGN=/usr/bin/codesign
elif [[ "$SELFTEST" == 1 ]]; then
  TOOLS="${AGENTERM_PKG_SELFTEST_TOOLS:-}"
  [[ -n "$TOOLS" && -d "$TOOLS" && ! -L "$TOOLS" ]] || fail "fixture_tools_directory_required" 2
  VALIDATE="$TOOLS/validate-app-bundle"
  XCRUN="$TOOLS/xcrun"
  PKGBUILD="$TOOLS/pkgbuild"
  PRODUCTSIGN="$TOOLS/productsign"
  PKGUTIL="$TOOLS/pkgutil"
  LSBOM="$TOOLS/lsbom"
  CODESIGN="$TOOLS/codesign"
  for tool in "$VALIDATE" "$XCRUN" "$PKGBUILD" "$PRODUCTSIGN" "$PKGUTIL" "$LSBOM" "$CODESIGN"; do
    [[ -f "$tool" && -x "$tool" && ! -L "$tool" ]] || fail "fixture_tool_missing_or_unsafe:$tool" 2
  done
else
  fail "invalid_AGENTERM_PKG_SELFTEST" 2
fi

for tool in "$VALIDATE" "$XCRUN" "$PKGBUILD" "$PRODUCTSIGN" "$PKGUTIL" "$LSBOM" "$CODESIGN" "$PYTHON" /usr/bin/ditto /usr/bin/plutil; do
  [[ -x "$tool" ]] || fail "required_tool_missing:$tool"
done

/usr/bin/plutil -lint "$COMPONENT_PLIST" >/dev/null
"$PYTHON" - "$COMPONENT_PLIST" "$DEPLOYMENT" <<'PY'
import json, plistlib, sys
with open(sys.argv[1], "rb") as stream:
    value = plistlib.load(stream)
expected = [{
    "RootRelativeBundlePath": "AgenTerm.app",
    "BundleIsRelocatable": False,
    "BundleIsVersionChecked": True,
    "BundleHasStrictIdentifier": True,
    "BundleOverwriteAction": "upgrade",
}]
if value != expected:
    raise SystemExit("installer_component_contract_drift")
for item in value:
    if any("script" in key.lower() for key in item):
        raise SystemExit("installer_component_script_forbidden")
with open(sys.argv[2], encoding="utf-8") as stream:
    deployment = json.load(stream)
if deployment.get("schema_version") != 1:
    raise SystemExit("installer_deployment_schema")
if deployment.get("installer") != {
    "package_identifier": "com.partnernetsoftware.agenterm.installer",
    "install_location": "/Applications",
    "ownership": "root:wheel",
    "scripts_allowed": False,
    "requires_stapled_app": True,
    "requires_developer_id_installer": True,
    "requires_trusted_timestamp": True,
    "package_notarization_required_for_release": True,
}:
    raise SystemExit("installer_deployment_contract_drift")
PY

# A valid Developer ID envelope is necessary but insufficient: the app must
# carry a locally verifiable stapled notarization ticket before it enters the
# root-owned package payload.
"$VALIDATE" --signed-bundle "$BUNDLE" >/dev/null
"$XCRUN" stapler validate "$BUNDLE" >/dev/null
APP_TEAM="$("$CODESIGN" -d --verbose=4 "$BUNDLE" 2>&1 | /usr/bin/sed -n 's/^TeamIdentifier=//p' | /usr/bin/head -n 1)"
INSTALLER_TEAM="${IDENTITY%)}"
INSTALLER_TEAM="${INSTALLER_TEAM##*(}"
[[ "$APP_TEAM" =~ ^[A-Z0-9]{10}$ && "$APP_TEAM" == "$INSTALLER_TEAM" ]] ||
  fail "installer_and_app_team_mismatch"

VERSION="$(/usr/bin/plutil -extract CFBundleShortVersionString raw \
  "$BUNDLE/Contents/Info.plist")"
case "$VERSION" in *[!0-9A-Za-z.-]*|'') fail "invalid_bundle_version" ;; esac

SCRATCH="$(/usr/bin/mktemp -d "$OUTPUT_DIR/.agenterm-installer.XXXXXX")"
cleanup() {
  local status=$?
  trap - EXIT
  /bin/rm -rf "$SCRATCH"
  exit "$status"
}
trap cleanup EXIT
PAYLOAD_ROOT="$SCRATCH/payload"
/bin/mkdir -p "$PAYLOAD_ROOT"
/usr/bin/ditto --rsrc --extattr --noqtn "$BUNDLE" "$PAYLOAD_ROOT/AgenTerm.app"

# Recheck the copied bytes/ticket so the package never relies on preservation
# assumptions about the staging copy.
"$VALIDATE" --signed-bundle "$PAYLOAD_ROOT/AgenTerm.app" >/dev/null
"$XCRUN" stapler validate "$PAYLOAD_ROOT/AgenTerm.app" >/dev/null

UNSIGNED_PKG="$SCRATCH/unsigned.pkg"
SIGNED_PKG="$SCRATCH/signed.pkg"
"$PKGBUILD" \
  --root "$PAYLOAD_ROOT" \
  --component-plist "$COMPONENT_PLIST" \
  --identifier com.partnernetsoftware.agenterm.installer \
  --version "$VERSION" \
  --install-location /Applications \
  --ownership recommended \
  "$UNSIGNED_PKG" >/dev/null
"$PRODUCTSIGN" --sign "$IDENTITY" "$UNSIGNED_PKG" "$SIGNED_PKG" >/dev/null
[[ -f "$SIGNED_PKG" && ! -L "$SIGNED_PKG" ]] || fail "signed_package_missing"

SIGNATURE_REPORT="$SCRATCH/signature.txt"
"$PKGUTIL" --check-signature "$SIGNED_PKG" >"$SIGNATURE_REPORT"
"$PYTHON" - "$SIGNATURE_REPORT" "$IDENTITY" <<'PY'
import sys
lines = [line.strip() for line in open(sys.argv[1], encoding="utf-8", errors="strict")]
certificates = [line[3:] for line in lines if line.startswith("1. ")]
if certificates != [sys.argv[2]]:
    raise SystemExit("installer_signature_identity_mismatch")
if not any("Status: signed" in line for line in lines):
    raise SystemExit("installer_signature_not_valid")
if not any(line.startswith("Signed with a trusted timestamp") for line in lines):
    raise SystemExit("installer_signature_timestamp_missing")
PY

EXPANDED="$SCRATCH/expanded"
"$PKGUTIL" --expand "$SIGNED_PKG" "$EXPANDED" >/dev/null
[[ -d "$EXPANDED" && ! -L "$EXPANDED" ]] || fail "expanded_package_missing"
if /usr/bin/find "$EXPANDED" -type l -print -quit | /usr/bin/grep -q .; then
  fail "installer_container_symlink_forbidden"
fi
if /usr/bin/find "$EXPANDED" -type d -name Scripts -print -quit | /usr/bin/grep -q .; then
  fail "installer_scripts_forbidden"
fi

PACKAGE_INFO=()
while IFS= read -r path; do PACKAGE_INFO+=("$path"); done < <(/usr/bin/find "$EXPANDED" -type f -name PackageInfo -print)
BOMS=()
while IFS= read -r path; do BOMS+=("$path"); done < <(/usr/bin/find "$EXPANDED" -type f -name Bom -print)
[[ ${#PACKAGE_INFO[@]} -eq 1 && ${#BOMS[@]} -eq 1 ]] || fail "installer_metadata_cardinality"
"$PYTHON" - "${PACKAGE_INFO[0]}" "$VERSION" <<'PY'
import sys, xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
if root.tag != "pkg-info":
    raise SystemExit("installer_package_info_root")
expected = {
    "identifier": "com.partnernetsoftware.agenterm.installer",
    "version": sys.argv[2],
    "install-location": "/Applications",
    "auth": "root",
    "relocatable": "false",
    "postinstall-action": "none",
}
if any(root.attrib.get(key) != value for key, value in expected.items()):
    raise SystemExit("installer_package_info_contract")
if any("script" in element.tag.lower() for element in root.iter()):
    raise SystemExit("installer_package_info_script_forbidden")
bundles = root.findall("bundle")
if len(bundles) != 1 or bundles[0].get("path") != "./AgenTerm.app" or bundles[0].get("id") != "com.partnernetsoftware.agenterm":
    raise SystemExit("installer_package_info_bundle")
strict = root.findall("strict-identifier/bundle")
if len(strict) != 1 or strict[0].get("id") != "com.partnernetsoftware.agenterm":
    raise SystemExit("installer_package_info_strict_identifier")
PY

SYMLINKS="$SCRATCH/bom-symlinks.txt"
BLOCKS="$SCRATCH/bom-blocks.txt"
CHARS="$SCRATCH/bom-chars.txt"
PATHS="$SCRATCH/bom-paths.txt"
MODES="$SCRATCH/bom-modes.txt"
"$LSBOM" -l "${BOMS[0]}" >"$SYMLINKS"
"$LSBOM" -b "${BOMS[0]}" >"$BLOCKS"
"$LSBOM" -c "${BOMS[0]}" >"$CHARS"
"$LSBOM" -s "${BOMS[0]}" >"$PATHS"
"$LSBOM" -p fmug "${BOMS[0]}" >"$MODES"
[[ ! -s "$SYMLINKS" ]] || fail "installer_payload_symlink_forbidden"
[[ ! -s "$BLOCKS" && ! -s "$CHARS" ]] || fail "installer_payload_device_forbidden"
"$PYTHON" - "$PATHS" "$MODES" <<'PY'
import sys
paths = [line.rstrip("\n") for line in open(sys.argv[1], encoding="utf-8", errors="strict")]
if not paths:
    raise SystemExit("installer_bom_empty")
normalized = []
for path in paths:
    while path.startswith("./"):
        path = path[2:]
    if path == ".":
        path = ""
    if path.startswith("/") or path == ".." or path.startswith("../") or "/../" in path:
        raise SystemExit("installer_bom_path_escape")
    if path and path != "AgenTerm.app" and not path.startswith("AgenTerm.app/"):
        raise SystemExit("installer_bom_unexpected_path:" + path)
    normalized.append(path)
required = {
    "AgenTerm.app",
    "AgenTerm.app/Contents/Info.plist",
    "AgenTerm.app/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege",
}
if not required.issubset(set(normalized)):
    raise SystemExit("installer_bom_required_payload_missing")
mode_rows = [line.rstrip("\n").split("\t") for line in open(sys.argv[2], encoding="utf-8", errors="strict")]
if len(mode_rows) != len(paths):
    raise SystemExit("installer_bom_mode_cardinality")
for row in mode_rows:
    if len(row) != 4:
        raise SystemExit("installer_bom_mode_shape")
    path, mode_text, uid_text, gid_text = row
    try:
        mode = int(mode_text, 8)
        uid = int(uid_text, 10)
        gid = int(gid_text, 10)
    except ValueError as error:
        raise SystemExit("installer_bom_mode_parse") from error
    if uid != 0 or gid != 0:
        raise SystemExit("installer_bom_not_root_owned:" + path)
    if mode & 0o22 or mode & 0o6000:
        raise SystemExit("installer_bom_writable_or_privileged:" + path)
PY

if [[ "$SELFTEST" == 1 ]]; then
  /bin/rm -f "$SIGNED_PKG"
  echo "MACOS_INSTALLER_FIXTURE_OK deployable=false artifact_written=false"
  exit 0
fi

# A hard link in the destination directory is the atomic no-overwrite publish
# primitive. Unlike mv -n, EEXIST is unambiguously reported as failure.
/bin/ln "$SIGNED_PKG" "$OUTPUT" || fail "output_publish_failed"
/bin/rm -f "$SIGNED_PKG"
echo "MACOS_INSTALLER_PKG_BUILT pkg=$OUTPUT signed=true app_stapled=true pkg_notarized=false"
