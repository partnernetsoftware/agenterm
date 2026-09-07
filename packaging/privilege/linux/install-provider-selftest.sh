#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
installer=$script_dir/install-provider.sh
systemctl_fixture=$script_dir/test-systemctl-fixture.sh
policy=$script_dir/com.partnernetsoftware.agenterm.cu.privilege.policy
socket_unit=$script_dir/com.partnernetsoftware.agenterm.cu.privilege.socket
service_unit=$script_dir/com.partnernetsoftware.agenterm.cu.privilege.service
test_root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-provider-install.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

make_generation() {
  generation=$1 directory=$2
  mkdir -p "$directory"
  printf 'provider-%s\n' "$generation" >"$directory/provider"
  sed "s#Apply one identity-bound#Apply $generation identity-bound#" "$policy" >"$directory/policy"
  sed "s#Description=AgenTerm#Description=$generation AgenTerm#" "$socket_unit" >"$directory/socket"
  sed "s#Description=AgenTerm#Description=$generation AgenTerm#" "$service_unit" >"$directory/service"
}

invoke_generation() {
  root=$1 generation_dir=$2 fail_cut=${3-} interrupt_cut=${4-}
  provider_sha=$(sha256_file "$generation_dir/provider")
  policy_sha=$(sha256_file "$generation_dir/policy")
  socket_sha=$(sha256_file "$generation_dir/socket")
  service_sha=$(sha256_file "$generation_dir/service")
  env AGENTERM_INSTALL_PROVIDER_TEST_MODE=1 \
  AGENTERM_INSTALL_PROVIDER_TEST_ROOT="$root" \
  AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC=1 \
  AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL="$systemctl_fixture" \
  AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL_LOG="$root/systemctl.log" \
  AGENTERM_INSTALL_PROVIDER_TEST_POLICY_SOURCE="$generation_dir/policy" \
  AGENTERM_INSTALL_PROVIDER_TEST_SOCKET_SOURCE="$generation_dir/socket" \
  AGENTERM_INSTALL_PROVIDER_TEST_SERVICE_SOURCE="$generation_dir/service" \
  AGENTERM_INSTALL_PROVIDER_TEST_FAIL_AFTER="$fail_cut" \
  AGENTERM_INSTALL_PROVIDER_TEST_INTERRUPT_AFTER="$interrupt_cut" \
    "$installer" install --source "$generation_dir/provider" --sha256 "$provider_sha" \
      --policy-sha256 "$policy_sha" --socket-sha256 "$socket_sha" --service-sha256 "$service_sha"
}

assert_generation() {
  root=$1 generation_dir=$2
  test "$(sha256_file "$root/usr/libexec/agenterm/agenterm-cu")" = "$(sha256_file "$generation_dir/provider")"
  test "$(sha256_file "$root/usr/share/polkit-1/actions/com.partnernetsoftware.agenterm.cu.privilege.policy")" = "$(sha256_file "$generation_dir/policy")"
  test "$(sha256_file "$root/usr/lib/systemd/system/com.partnernetsoftware.agenterm.cu.privilege.socket")" = "$(sha256_file "$generation_dir/socket")"
  test "$(sha256_file "$root/usr/lib/systemd/system/com.partnernetsoftware.agenterm.cu.privilege.service")" = "$(sha256_file "$generation_dir/service")"
  manifest=$root/var/lib/agenterm/cu-privilege/install-manifest
  test "$(sed -n '1p' "$manifest")" = agenterm-cu-provider-manifest-v2
  grep -qx "provider_sha256=$(sha256_file "$generation_dir/provider")" "$manifest"
  grep -qx "policy_sha256=$(sha256_file "$generation_dir/policy")" "$manifest"
  grep -qx "socket_sha256=$(sha256_file "$generation_dir/socket")" "$manifest"
  grep -qx "service_sha256=$(sha256_file "$generation_dir/service")" "$manifest"
  test "$(stat -f '%Lp' "$root/var/lib/agenterm/cu-privilege" 2>/dev/null || stat -c '%a' "$root/var/lib/agenterm/cu-privilege")" = 700
  test "$(stat -f '%Lp' "$manifest" 2>/dev/null || stat -c '%a' "$manifest")" = 600
  test ! -e "$root/var/lib/agenterm/cu-privilege/install-transaction"
  test -z "$(find "$root/var/lib/agenterm/cu-privilege" -name '*.backup' -print -quit)"
  . "$root/systemctl.log.state"
  test "$socket_enabled" = enabled
  test "$socket_active" = active
  test "$service_active" = inactive
}

generation_v1=$test_root/v1
generation_v2=$test_root/v2
make_generation v1 "$generation_v1"
make_generation v2 "$generation_v2"

# Static security contract: one ordinary polkit action, no pkexec annotations;
# one world-connectable root-owned socket and a fixed root service command.
! grep -q 'org.freedesktop.policykit.exec' "$policy"
grep -q '<allow_active>auth_admin</allow_active>' "$policy"
! grep -q 'auth_admin_keep' "$policy"
grep -q '^ListenStream=/run/agenterm/cu-privilege.sock$' "$socket_unit"
grep -q '^SocketMode=0666$' "$socket_unit"
grep -q '^SocketUser=root$' "$socket_unit"
grep -q '^SocketGroup=root$' "$socket_unit"
grep -q '^Accept=no$' "$socket_unit"
grep -q '^ExecStart=/usr/libexec/agenterm/agenterm-cu --agenterm-cu-internal-privilege-broker$' "$service_unit"
grep -q '^User=root$' "$service_unit"
grep -q '^Group=root$' "$service_unit"

success_root=$test_root/success
invoke_generation "$success_root" "$generation_v1"
assert_generation "$success_root" "$generation_v1"
grep -qx 'daemon-reload' "$success_root/systemctl.log"
grep -qx 'enable com.partnernetsoftware.agenterm.cu.privilege.socket' "$success_root/systemctl.log"
grep -qx 'start com.partnernetsoftware.agenterm.cu.privilege.socket' "$success_root/systemctl.log"
! grep -q '^enable .*service' "$success_root/systemctl.log"
awk '
  /stop com.partnernetsoftware.agenterm.cu.privilege.socket/ { socket_stop=NR }
  /stop com.partnernetsoftware.agenterm.cu.privilege.service/ { service_stop=NR }
  /daemon-reload/ { reload=NR }
  /enable com.partnernetsoftware.agenterm.cu.privilege.socket/ { enable=NR }
  /start com.partnernetsoftware.agenterm.cu.privilege.socket/ { start=NR }
  END { exit !(socket_stop < service_stop && service_stop < reload && reload < enable && enable < start) }
' "$success_root/systemctl.log"

# Every four-artifact publication cut rolls the complete old generation back.
for cut in provider policy socket service systemctl; do
  root=$test_root/fail-$cut
  invoke_generation "$root" "$generation_v1" >/dev/null
  set +e
  invoke_generation "$root" "$generation_v2" "$cut" >/dev/null 2>&1
  status=$?
  set -e
  test "$status" -eq 2
  assert_generation "$root" "$generation_v1"
done

# SIGKILL leaves the durable marker; the next install recovers the whole old
# generation before starting and then publishes one complete new generation.
for cut in provider policy socket service systemctl; do
  root=$test_root/interrupt-$cut
  invoke_generation "$root" "$generation_v1" >/dev/null
  set +e
  invoke_generation "$root" "$generation_v2" "" "$cut" >/dev/null 2>&1
  status=$?
  set -e
  test "$status" -ne 0
  test -f "$root/var/lib/agenterm/cu-privilege/install-transaction"
  . "$root/systemctl.log.state"
  if test "$cut" = systemctl; then
    test "$socket_active" = active
  else
    test "$socket_active" = inactive
    test "$service_active" = inactive
  fi
  invoke_generation "$root" "$generation_v2" >/dev/null
  assert_generation "$root" "$generation_v2"
done

first_root=$test_root/first-install
set +e
invoke_generation "$first_root" "$generation_v2" policy >/dev/null 2>&1
status=$?
set -e
test "$status" -eq 2
test ! -e "$first_root/usr/libexec/agenterm/agenterm-cu"
test ! -e "$first_root/usr/share/polkit-1/actions/com.partnernetsoftware.agenterm.cu.privilege.policy"
test ! -e "$first_root/usr/lib/systemd/system/com.partnernetsoftware.agenterm.cu.privilege.socket"
test ! -e "$first_root/usr/lib/systemd/system/com.partnernetsoftware.agenterm.cu.privilege.service"
. "$first_root/systemctl.log.state"
test "$socket_enabled" = disabled
test "$socket_active" = inactive
test "$service_active" = inactive

bad_root=$test_root/bad-digest
set +e
AGENTERM_INSTALL_PROVIDER_TEST_MODE=1 \
AGENTERM_INSTALL_PROVIDER_TEST_ROOT="$bad_root" \
AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC=1 \
AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL="$systemctl_fixture" \
AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL_LOG="$bad_root.log" \
  "$installer" install --source "$generation_v1/provider" --sha256 "$(sha256_file "$generation_v1/provider")" \
  --policy-sha256 "$(sha256_file "$policy")" --socket-sha256 "$(printf '%064d' 0)" \
  --service-sha256 "$(sha256_file "$service_unit")" >/dev/null 2>&1
status=$?
set -e
test "$status" -eq 2

uninstall_root=$test_root/uninstall
invoke_generation "$uninstall_root" "$generation_v1" >/dev/null
env AGENTERM_INSTALL_PROVIDER_TEST_MODE=1 \
AGENTERM_INSTALL_PROVIDER_TEST_ROOT="$uninstall_root" \
AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC=1 \
AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL="$systemctl_fixture" \
AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL_LOG="$uninstall_root/systemctl.log" \
  "$installer" uninstall --purge-state >/dev/null
test ! -e "$uninstall_root/usr/libexec/agenterm/agenterm-cu"
test ! -e "$uninstall_root/usr/share/polkit-1/actions/com.partnernetsoftware.agenterm.cu.privilege.policy"
test ! -e "$uninstall_root/usr/lib/systemd/system/com.partnernetsoftware.agenterm.cu.privilege.socket"
test ! -e "$uninstall_root/usr/lib/systemd/system/com.partnernetsoftware.agenterm.cu.privilege.service"
test ! -e "$uninstall_root/var/lib/agenterm/cu-privilege"
. "$uninstall_root/systemctl.log.state"
test "$socket_enabled" = disabled
test "$socket_active" = inactive
test "$service_active" = inactive

echo "PASS: Linux privilege broker install is four-artifact digest-sealed and crash-recoverable"
