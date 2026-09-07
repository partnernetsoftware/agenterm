#!/bin/sh
set -eu

policy_name=com.partnernetsoftware.agenterm.cu.privilege.policy
test_mode=${AGENTERM_INSTALL_PROVIDER_TEST_MODE:-0}
test_root=${AGENTERM_INSTALL_PROVIDER_TEST_ROOT:-}
test_policy_source=${AGENTERM_INSTALL_PROVIDER_TEST_POLICY_SOURCE:-}
test_fail_after_provider=${AGENTERM_INSTALL_PROVIDER_TEST_FAIL_AFTER_PROVIDER:-0}
test_no_fsync=${AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC:-0}

case "$test_mode" in 0|1) ;; *) echo "install-provider: invalid test mode" >&2; exit 2 ;; esac
if test "$test_mode" -ne 1; then
  test -z "$test_root$test_policy_source" || {
    echo "install-provider: test path overrides require explicit test mode" >&2
    exit 2
  }
  test "$test_fail_after_provider" = 0 && test "$test_no_fsync" = 0 || {
    echo "install-provider: test controls require explicit test mode" >&2
    exit 2
  }
  provider_dir=/usr/libexec/agenterm
  policy_dir=/usr/share/polkit-1/actions
  state_dir=/var/lib/agenterm/cu-privilege
else
  test -n "$test_root" || {
    echo "install-provider: test mode requires an explicit test root" >&2
    exit 2
  }
  case "$test_root" in /*) ;; *) echo "install-provider: test root must be absolute" >&2; exit 2 ;; esac
  test "$test_root" != / || {
    echo "install-provider: test root must not be /" >&2
    exit 2
  }
  provider_dir=$test_root/usr/libexec/agenterm
  policy_dir=$test_root/usr/share/polkit-1/actions
  state_dir=$test_root/var/lib/agenterm/cu-privilege
fi
provider_path=$provider_dir/agenterm-cu
policy_path=$policy_dir/$policy_name
marker=$state_dir/install-transaction
provider_backup=$state_dir/provider.backup
policy_backup=$state_dir/policy.backup

fail() {
  echo "install-provider: $*" >&2
  exit 2
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    fail "sha256sum or shasum is required"
  fi
}

is_sha256() {
  test "${#1}" -eq 64 || return 1
  case "$1" in
    *[!0-9a-f]*) return 1 ;;
  esac
}

sync_path() {
  if test "$test_mode" -eq 1 && test "$test_no_fsync" -eq 1; then
    return 0
  fi
  sync -f "$1" 2>/dev/null || fail "cannot synchronize $1"
}

install_owned() {
  owned_mode=$1
  owned_source=$2
  owned_destination=$3
  if test "$test_mode" -eq 1; then
    install -m "$owned_mode" "$owned_source" "$owned_destination"
  else
    install -o root -g root -m "$owned_mode" "$owned_source" "$owned_destination"
  fi
}

make_dir() {
  directory_mode=$1
  directory_path=$2
  if test "$test_mode" -eq 1; then
    install -d -m "$directory_mode" "$directory_path"
  else
    install -d -o root -g root -m "$directory_mode" "$directory_path"
  fi
}

restore_one() {
  restore_prior=$1
  restore_backup=$2
  restore_destination=$3
  restore_directory=$4
  restore_mode=$5
  restore_stage=$restore_directory/.agenterm-cu.restore.$$
  if test "$restore_prior" = present; then
    test -f "$restore_backup" && test ! -L "$restore_backup" || return 1
    install_owned "$restore_mode" "$restore_backup" "$restore_stage" || return 1
    sync_path "$restore_stage" || return 1
    mv -f "$restore_stage" "$restore_destination" || return 1
    sync_path "$restore_directory" || return 1
  else
    rm -f "$restore_destination" || return 1
    sync_path "$restore_directory" || return 1
  fi
}

recover_transaction() {
  test -f "$marker" && test ! -L "$marker" || return 1
  marker_header=$(sed -n '1p' "$marker")
  prior_provider=$(sed -n 's/^provider=//p' "$marker")
  prior_policy=$(sed -n 's/^policy=//p' "$marker")
  test "$marker_header" = agenterm-cu-provider-install-v1 || return 1
  case "$prior_provider" in present|absent) ;; *) return 1 ;; esac
  case "$prior_policy" in present|absent) ;; *) return 1 ;; esac
  restore_one "$prior_provider" "$provider_backup" "$provider_path" "$provider_dir" 0755 || return 1
  restore_one "$prior_policy" "$policy_backup" "$policy_path" "$policy_dir" 0644 || return 1
  rm -f "$marker"
  sync_path "$state_dir" || return 1
  rm -f "$provider_backup" "$policy_backup"
  sync_path "$state_dir" || return 1
}

test "$test_mode" -eq 1 || test "$(id -u)" -eq 0 || fail "run through the system package installer as root"
action=${1-}
shift || true

case "$action" in
  install)
    test "${1-}" = "--source" || fail "expected install --source FILE --sha256 DIGEST --policy-sha256 DIGEST"
    source_path=${2-}
    test "${3-}" = "--sha256" || fail "expected install --source FILE --sha256 DIGEST --policy-sha256 DIGEST"
    provider_expected=${4-}
    test "${5-}" = "--policy-sha256" || fail "expected install --source FILE --sha256 DIGEST --policy-sha256 DIGEST"
    policy_expected=${6-}
    test "$#" -eq 6 || fail "unexpected install arguments"
    is_sha256 "$provider_expected" || fail "provider digest must be lowercase SHA-256"
    is_sha256 "$policy_expected" || fail "policy digest must be lowercase SHA-256"
    test -f "$source_path" && test ! -L "$source_path" || fail "source must be a regular non-symlink file"
    actual=$(sha256_file "$source_path")
    test "$actual" = "$provider_expected" || fail "source digest does not match the sealed artifact"

    script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
    policy_source=$script_dir/$policy_name
    if test "$test_mode" -eq 1 && test -n "$test_policy_source"; then
      policy_source=$test_policy_source
    fi
    test -f "$policy_source" && test ! -L "$policy_source" || fail "polkit policy is missing or linked"
    policy_actual=$(sha256_file "$policy_source")
    test "$policy_actual" = "$policy_expected" || fail "policy digest does not match the sealed artifact"

    make_dir 0755 "$provider_dir"
    make_dir 0755 "$policy_dir"
    make_dir 0700 "$state_dir"
    if test -e "$marker" || test -L "$marker"; then
      recover_transaction || fail "cannot recover the interrupted provider installation"
    else
      rm -f "$provider_backup" "$policy_backup"
    fi

    provider_stage=$provider_dir/.agenterm-cu.install.$$
    policy_stage=$policy_dir/.$policy_name.install.$$
    marker_stage=$state_dir/.install-transaction.$$
    transaction_active=0
    on_exit() {
      status=$?
      trap - 0 1 2 15
      rm -f "$provider_stage" "$policy_stage" "$marker_stage"
      if test "$transaction_active" -eq 1; then
        if ! recover_transaction; then
          echo "install-provider: failed to restore interrupted installation" >&2
          exit 3
        fi
      fi
      exit "$status"
    }
    trap on_exit 0
    trap 'exit 129' 1
    trap 'exit 130' 2
    trap 'exit 143' 15

    install_owned 0755 "$source_path" "$provider_stage"
    test "$(sha256_file "$provider_stage")" = "$provider_expected" || fail "staged provider digest changed"
    sync_path "$provider_stage"
    install_owned 0644 "$policy_source" "$policy_stage"
    test "$(sha256_file "$policy_stage")" = "$policy_expected" || fail "staged policy digest changed"
    sync_path "$policy_stage"

    prior_provider=absent
    prior_policy=absent
    if test -e "$provider_path" || test -L "$provider_path"; then
      test -f "$provider_path" && test ! -L "$provider_path" || fail "installed provider is not a regular file"
      install_owned 0755 "$provider_path" "$provider_backup"
      sync_path "$provider_backup"
      prior_provider=present
    fi
    if test -e "$policy_path" || test -L "$policy_path"; then
      test -f "$policy_path" && test ! -L "$policy_path" || fail "installed policy is not a regular file"
      install_owned 0644 "$policy_path" "$policy_backup"
      sync_path "$policy_backup"
      prior_policy=present
    fi
    # The backups and marker live on one protected filesystem. Publish and
    # synchronize them before changing either destination. The provider and
    # policy destinations are on potentially different filesystems, so this is
    # a recoverable two-file transaction, not a cross-directory atomic rename.
    sync_path "$state_dir"
    {
      echo agenterm-cu-provider-install-v1
      echo "provider=$prior_provider"
      echo "policy=$prior_policy"
    } >"$marker_stage"
    if test "$test_mode" -ne 1; then
      chown root:root "$marker_stage"
      chmod 0600 "$marker_stage"
    else
      chmod 0600 "$marker_stage"
    fi
    sync_path "$marker_stage"
    mv -f "$marker_stage" "$marker"
    sync_path "$state_dir"
    transaction_active=1

    mv -f "$provider_stage" "$provider_path"
    sync_path "$provider_dir"
    if test "$test_fail_after_provider" -eq 1; then
      fail "injected failure after provider publication"
    fi
    mv -f "$policy_stage" "$policy_path"
    sync_path "$policy_dir"
    test "$(sha256_file "$provider_path")" = "$provider_expected" || fail "installed provider digest changed"
    test "$(sha256_file "$policy_path")" = "$policy_expected" || fail "installed policy digest changed"

    rm -f "$marker"
    sync_path "$state_dir"
    transaction_active=0
    rm -f "$provider_backup" "$policy_backup"
    sync_path "$state_dir"
    trap - 0 1 2 15
    echo "installed fixed AgenTerm privilege provider"
    ;;
  uninstall)
    purge_state=0
    if test "${1-}" = "--purge-state"; then
      purge_state=1
      shift
    fi
    test "$#" -eq 0 || fail "unexpected uninstall arguments"
    rm -f "$policy_dir/$policy_name"
    rm -f "$provider_path"
    if test "$purge_state" -eq 1; then
      rm -rf "$state_dir"
    fi
    echo "removed fixed AgenTerm privilege provider"
    ;;
  *) fail "expected install or uninstall" ;;
esac
