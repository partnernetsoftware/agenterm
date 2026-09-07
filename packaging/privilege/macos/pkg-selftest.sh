#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BUILDER="$ROOT/packaging/privilege/macos/build-installer-pkg.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/agenterm-macos-pkg.XXXXXX")"
cleanup() {
  local status=$?
  trap - EXIT
  rm -rf "$TMP"
  exit "$status"
}
trap cleanup EXIT
TOOLS="$TMP/tools"
BUNDLE="$TMP/AgenTerm.app"
OUTPUT="$TMP/AgenTerm.fixture.pkg"
IDENTITY="Developer ID Installer: Fixture Only (AAAAAAAAAA)"
LOG="$TMP/tools.log"
PYTHON="$(command -v python3)"
mkdir -p "$TOOLS" "$BUNDLE/Contents/Resources"

cp "$ROOT/packaging/privilege/macos/Info.plist.in" "$BUNDLE/Contents/Info.plist"
sed -i '' 's/@AGENTERM_VERSION@/0.0.0-test/g' "$BUNDLE/Contents/Info.plist"
printf 'fixture helper\n' > "$BUNDLE/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege"
touch "$BUNDLE/.fixture-signed" "$BUNDLE/.fixture-stapled"

cat > "$TOOLS/validate-app-bundle" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'validate\t%s\t%s\n' "$1" "$2" >> "$AGENTERM_PKG_FIXTURE_LOG"
[[ "$1" == --signed-bundle && -f "$2/.fixture-signed" ]]
SH
cat > "$TOOLS/xcrun" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'xcrun\t%s\t%s\t%s\n' "$1" "$2" "$3" >> "$AGENTERM_PKG_FIXTURE_LOG"
[[ "$1" == stapler && "$2" == validate && -f "$3/.fixture-stapled" ]]
SH
cat > "$TOOLS/codesign" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == -d && "$2" == --verbose=4 ]]
printf 'TeamIdentifier=%s\n' "${AGENTERM_PKG_FIXTURE_APP_TEAM:-AAAAAAAAAA}" >&2
SH
cat > "$TOOLS/pkgbuild" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'pkgbuild' >> "$AGENTERM_PKG_FIXTURE_LOG"
printf '\t%s' "$@" >> "$AGENTERM_PKG_FIXTURE_LOG"
printf '\n' >> "$AGENTERM_PKG_FIXTURE_LOG"
output="${!#}"
printf 'unsigned fixture package\n' > "$output"
SH
cat > "$TOOLS/productsign" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'productsign' >> "$AGENTERM_PKG_FIXTURE_LOG"
printf '\t%s' "$@" >> "$AGENTERM_PKG_FIXTURE_LOG"
printf '\n' >> "$AGENTERM_PKG_FIXTURE_LOG"
[[ "$1" == --sign && "$2" == "$AGENTERM_PKG_FIXTURE_IDENTITY" ]]
cp "$3" "$4"
SH
cat > "$TOOLS/pkgutil" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'pkgutil' >> "$AGENTERM_PKG_FIXTURE_LOG"
printf '\t%s' "$@" >> "$AGENTERM_PKG_FIXTURE_LOG"
printf '\n' >> "$AGENTERM_PKG_FIXTURE_LOG"
case "$1" in
  --check-signature)
    printf 'Status: signed by a developer certificate issued by Apple for distribution\n'
    if [[ "${AGENTERM_PKG_FIXTURE_NO_TIMESTAMP:-0}" != 1 ]]; then
      printf 'Signed with a trusted timestamp on: 2000-01-01 00:00:00 +0000\n'
    fi
    printf 'Certificate Chain:\n'
    printf ' 1. %s\n' "${AGENTERM_PKG_FIXTURE_SIGNATURE_IDENTITY:-$AGENTERM_PKG_FIXTURE_IDENTITY}"
    ;;
  --expand)
    mkdir -p "$3"
    cat > "$3/PackageInfo" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<pkg-info identifier="com.partnernetsoftware.agenterm.installer" version="0.0.0-test" install-location="/Applications" auth="root" relocatable="false" postinstall-action="none">
  <payload numberOfFiles="3" installKBytes="1"/>
  <bundle path="./AgenTerm.app" id="com.partnernetsoftware.agenterm"/>
  <strict-identifier><bundle id="com.partnernetsoftware.agenterm"/></strict-identifier>
</pkg-info>
EOF
    : > "$3/Bom"
    : > "$3/Payload"
    if [[ "${AGENTERM_PKG_FIXTURE_SCRIPTS:-0}" == 1 ]]; then
      mkdir "$3/Scripts"
    fi
    ;;
  *) exit 2 ;;
esac
SH
cat > "$TOOLS/lsbom" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  -l)
    if [[ "${AGENTERM_PKG_FIXTURE_SYMLINK:-0}" == 1 ]]; then
      printf './AgenTerm.app/bad-link\n'
    fi
    ;;
  -b|-c) ;;
  -s)
    printf '.\n./AgenTerm.app\n./AgenTerm.app/Contents/Info.plist\n'
    printf './AgenTerm.app/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege\n'
    ;;
  -p)
    [[ "$2" == fmug ]]
    mode=40755
    [[ "${AGENTERM_PKG_FIXTURE_DANGEROUS:-0}" == 1 ]] && mode=40777
    printf '.\t%s\t0\t0\n' "$mode"
    printf './AgenTerm.app\t%s\t0\t0\n' "$mode"
    printf './AgenTerm.app/Contents/Info.plist\t100644\t0\t0\n'
    printf './AgenTerm.app/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege\t100755\t0\t0\n'
    ;;
  *) exit 2 ;;
esac
SH
chmod 0755 "$TOOLS"/*

run_fixture() {
  AGENTERM_PKG_SELFTEST=1 \
  AGENTERM_PKG_SELFTEST_TOOLS="$TOOLS" \
  AGENTERM_PKG_FIXTURE_LOG="$LOG" \
  AGENTERM_PKG_FIXTURE_IDENTITY="$IDENTITY" \
    "$BUILDER" --bundle "$BUNDLE" --output "$OUTPUT" --installer-identity "$IDENTITY"
}

assert_rejected() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "fixture unexpectedly accepted: $label" >&2
    exit 1
  fi
}

assert_rejected invalid-identity env \
  AGENTERM_PKG_SELFTEST=1 AGENTERM_PKG_SELFTEST_TOOLS="$TOOLS" \
  "$BUILDER" --bundle "$BUNDLE" --output "$OUTPUT" --installer-identity "Developer ID Application: Wrong"
assert_rejected wrong-bundle-name env \
  AGENTERM_PKG_SELFTEST=1 AGENTERM_PKG_SELFTEST_TOOLS="$TOOLS" \
  "$BUILDER" --bundle "$TMP/Wrong.app" --output "$OUTPUT" --installer-identity "$IDENTITY"
assert_rejected wrong-output-suffix env \
  AGENTERM_PKG_SELFTEST=1 AGENTERM_PKG_SELFTEST_TOOLS="$TOOLS" \
  "$BUILDER" --bundle "$BUNDLE" --output "$TMP/output.zip" --installer-identity "$IDENTITY"

printf 'keep me\n' > "$OUTPUT"
assert_rejected no-overwrite run_fixture
[[ "$(cat "$OUTPUT")" == "keep me" ]] || { echo "existing output changed" >&2; exit 1; }
rm "$OUTPUT"

rm "$BUNDLE/.fixture-signed"
assert_rejected unsigned run_fixture
touch "$BUNDLE/.fixture-signed"
rm "$BUNDLE/.fixture-stapled"
assert_rejected unstapled run_fixture
touch "$BUNDLE/.fixture-stapled"

AGENTERM_PKG_FIXTURE_SCRIPTS=1 assert_rejected installer-scripts run_fixture
AGENTERM_PKG_FIXTURE_SYMLINK=1 assert_rejected payload-symlink run_fixture
AGENTERM_PKG_FIXTURE_DANGEROUS=1 assert_rejected writable-payload run_fixture
AGENTERM_PKG_FIXTURE_APP_TEAM=BBBBBBBBBB assert_rejected app-installer-team-mismatch run_fixture
AGENTERM_PKG_FIXTURE_SIGNATURE_IDENTITY="Developer ID Installer: Other Fixture (AAAAAAAAAA)" \
  assert_rejected signature-identity-mismatch run_fixture
AGENTERM_PKG_FIXTURE_NO_TIMESTAMP=1 assert_rejected signature-timestamp-missing run_fixture

: > "$LOG"
RESULT="$(run_fixture)"
[[ "$RESULT" == "MACOS_INSTALLER_FIXTURE_OK deployable=false artifact_written=false" ]] || {
  echo "fixture run claimed an unexpected result: $RESULT" >&2
  exit 1
}
[[ ! -e "$OUTPUT" ]] || { echo "fixture mode published an artifact" >&2; exit 1; }

"$ROOT/packaging/privilege/macos/validate-app-bundle.sh" --signed-bundle "$BUNDLE" \
  >/dev/null 2>&1 && { echo "synthetic bundle reached production signed-bundle success" >&2; exit 1; }

"$ROOT/packaging/privilege/macos/build-installer-pkg.sh" --bundle "$BUNDLE" \
  --output "$OUTPUT" --installer-identity "$IDENTITY" \
  >/dev/null 2>&1 && { echo "synthetic bundle reached production package success" >&2; exit 1; }

"$PYTHON" - "$LOG" "$ROOT" "$TMP" <<'PY'
import sys
log = open(sys.argv[1], encoding="utf-8").read().splitlines()
root, temp = sys.argv[2:]
pkgbuild = [line.split("\t") for line in log if line.startswith("pkgbuild\t")]
if len(pkgbuild) != 1:
    raise SystemExit("fixture_pkgbuild_call_count")
args = pkgbuild[0][1:]
expected_pairs = {
    "--root": temp + "/.agenterm-installer.",
    "--component-plist": root + "/packaging/privilege/macos/component.plist",
    "--identifier": "com.partnernetsoftware.agenterm.installer",
    "--version": "0.0.0-test",
    "--install-location": "/Applications",
    "--ownership": "recommended",
}
for flag, expected in expected_pairs.items():
    index = args.index(flag)
    actual = args[index + 1]
    if flag == "--root":
        if not actual.startswith(expected) or not actual.endswith("/payload"):
            raise SystemExit("fixture_pkgbuild_root")
    elif actual != expected:
        raise SystemExit("fixture_pkgbuild_argument:" + flag)
if "--scripts" in args:
    raise SystemExit("fixture_pkgbuild_scripts_forbidden")
if len([line for line in log if line.startswith("validate\t--signed-bundle\t")]) != 2:
    raise SystemExit("fixture_validate_copy_boundary")
if len([line for line in log if line.startswith("xcrun\tstapler\tvalidate\t")]) != 2:
    raise SystemExit("fixture_stapler_copy_boundary")
PY

echo "PASS: macOS root-owned Installer PKG build contract is fail-closed (fixture-only; no package published)"
