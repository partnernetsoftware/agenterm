#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
installer=$script_dir/install-provider.sh
policy=$script_dir/com.partnernetsoftware.agenterm.cu.privilege.policy
test_root=$(mktemp -d "${TMPDIR:-/tmp}/agenterm-provider-install.XXXXXX")
fixture=$test_root/provider-fixture

cleanup() {
  rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

invoke() {
  AGENTERM_INSTALL_PROVIDER_TEST_MODE=1 \
  AGENTERM_INSTALL_PROVIDER_TEST_ROOT="$test_root/root" \
  AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC=1 \
    "$installer" "$@"
}

printf '%s\n' provider-v1 >"$fixture"
provider_v1=$(sha256_file "$fixture")
policy_digest=$(sha256_file "$policy")
invoke install --source "$fixture" --sha256 "$provider_v1" --policy-sha256 "$policy_digest"
provider_path=$test_root/root/usr/libexec/agenterm/agenterm-cu
policy_path=$test_root/root/usr/share/polkit-1/actions/com.partnernetsoftware.agenterm.cu.privilege.policy
test "$(sha256_file "$provider_path")" = "$provider_v1"
test "$(sha256_file "$policy_path")" = "$policy_digest"

printf '%s\n' provider-v2 >"$fixture"
provider_v2=$(sha256_file "$fixture")
set +e
AGENTERM_INSTALL_PROVIDER_TEST_MODE=1 \
AGENTERM_INSTALL_PROVIDER_TEST_ROOT="$test_root/root" \
AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC=1 \
AGENTERM_INSTALL_PROVIDER_TEST_FAIL_AFTER_PROVIDER=1 \
  "$installer" install --source "$fixture" --sha256 "$provider_v2" --policy-sha256 "$policy_digest"
injected_status=$?
set -e
test "$injected_status" -eq 2
test "$(sha256_file "$provider_path")" = "$provider_v1"
test "$(sha256_file "$policy_path")" = "$policy_digest"
test ! -e "$test_root/root/var/lib/agenterm/cu-privilege/install-transaction"

first_root=$test_root/first-install
set +e
AGENTERM_INSTALL_PROVIDER_TEST_MODE=1 \
AGENTERM_INSTALL_PROVIDER_TEST_ROOT="$first_root" \
AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC=1 \
AGENTERM_INSTALL_PROVIDER_TEST_FAIL_AFTER_PROVIDER=1 \
  "$installer" install --source "$fixture" --sha256 "$provider_v2" --policy-sha256 "$policy_digest"
first_status=$?
set -e
test "$first_status" -eq 2
test ! -e "$first_root/usr/libexec/agenterm/agenterm-cu"
test ! -e "$first_root/usr/share/polkit-1/actions/com.partnernetsoftware.agenterm.cu.privilege.policy"

set +e
invoke install --source "$fixture" --sha256 "$provider_v2" --policy-sha256 "$(printf '%064d' 0)"
digest_status=$?
set -e
test "$digest_status" -eq 2

echo "PASS: Linux privilege provider install is digest-sealed and crash-recoverable"
