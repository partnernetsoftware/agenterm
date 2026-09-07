#!/bin/sh
set -eu

base=com.partnernetsoftware.agenterm.cu.privilege
policy_name=$base.policy
socket_name=$base.socket
service_name=$base.service
test_mode=${AGENTERM_INSTALL_PROVIDER_TEST_MODE:-0}
test_root=${AGENTERM_INSTALL_PROVIDER_TEST_ROOT:-}
test_no_fsync=${AGENTERM_INSTALL_PROVIDER_TEST_NO_FSYNC:-0}
test_fail_after=${AGENTERM_INSTALL_PROVIDER_TEST_FAIL_AFTER:-}
test_interrupt_after=${AGENTERM_INSTALL_PROVIDER_TEST_INTERRUPT_AFTER:-}
test_systemctl=${AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL:-}
test_policy_source=${AGENTERM_INSTALL_PROVIDER_TEST_POLICY_SOURCE:-}
test_socket_source=${AGENTERM_INSTALL_PROVIDER_TEST_SOCKET_SOURCE:-}
test_service_source=${AGENTERM_INSTALL_PROVIDER_TEST_SERVICE_SOURCE:-}

fail() { echo "install-provider: $*" >&2; exit 2; }
case "$test_mode" in 0|1) ;; *) fail "invalid test mode" ;; esac
case "$test_fail_after" in ""|provider|policy|socket|service|systemctl) ;; *) fail "invalid failure cut" ;; esac
case "$test_interrupt_after" in ""|provider|policy|socket|service|systemctl) ;; *) fail "invalid interruption cut" ;; esac

if test "$test_mode" -eq 0; then
  test -z "$test_root$test_fail_after$test_interrupt_after$test_systemctl$test_policy_source$test_socket_source$test_service_source" || fail "test controls require explicit test mode"
  test "$test_no_fsync" = 0 || fail "test controls require explicit test mode"
  prefix=
  systemctl_bin=/usr/bin/systemctl
else
  test -n "$test_root" || fail "test mode requires an explicit test root"
  case "$test_root" in /*) ;; *) fail "test root must be absolute" ;; esac
  test "$test_root" != / || fail "test root must not be /"
  test -n "$test_systemctl" && test -x "$test_systemctl" || fail "test mode requires an executable systemctl seam"
  prefix=$test_root
  systemctl_bin=$test_systemctl
fi

provider_dir=$prefix/usr/libexec/agenterm
policy_dir=$prefix/usr/share/polkit-1/actions
unit_dir=$prefix/usr/lib/systemd/system
state_dir=$prefix/var/lib/agenterm/cu-privilege
provider_path=$provider_dir/agenterm-cu
policy_path=$policy_dir/$policy_name
socket_path=$unit_dir/$socket_name
service_path=$unit_dir/$service_name
marker=$state_dir/install-transaction
manifest=$state_dir/install-manifest

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
  else fail "sha256sum or shasum is required"; fi
}
is_sha256() { test "${#1}" -eq 64 && case "$1" in *[!0-9a-f]*) false ;; *) true ;; esac; }
sync_path() {
  if test "$test_mode" -eq 1 && test "$test_no_fsync" -eq 1; then return 0; fi
  sync -f "$1" 2>/dev/null || fail "cannot synchronize $1"
}
install_owned() {
  io_mode=$1 io_source=$2 io_destination=$3
  if test "$test_mode" -eq 1; then install -m "$io_mode" "$io_source" "$io_destination"
  else install -o root -g root -m "$io_mode" "$io_source" "$io_destination"; fi
}
make_dir() {
  md_mode=$1 md_path=$2
  if test "$test_mode" -eq 1; then install -d -m "$md_mode" "$md_path"
  else install -d -o root -g root -m "$md_mode" "$md_path"; fi
}
artifact_source() {
  case "$1" in
    provider) printf '%s\n' "$source_path" ;;
    policy) printf '%s\n' "${test_policy_source:-$script_dir/$policy_name}" ;;
    socket) printf '%s\n' "${test_socket_source:-$script_dir/$socket_name}" ;;
    service) printf '%s\n' "${test_service_source:-$script_dir/$service_name}" ;;
  esac
}
artifact_path() {
  case "$1" in
    provider) printf '%s\n' "$provider_path" ;;
    policy) printf '%s\n' "$policy_path" ;;
    socket) printf '%s\n' "$socket_path" ;;
    service) printf '%s\n' "$service_path" ;;
  esac
}
artifact_dir() {
  case "$1" in provider) printf '%s\n' "$provider_dir" ;; policy) printf '%s\n' "$policy_dir" ;; *) printf '%s\n' "$unit_dir" ;; esac
}
artifact_mode() { case "$1" in provider) printf '0755\n' ;; *) printf '0644\n' ;; esac; }
backup_path() { printf '%s/%s.backup\n' "$state_dir" "$1"; }

restore_one() {
  ro_name=$1 ro_prior=$2 ro_destination=$(artifact_path "$1") ro_directory=$(artifact_dir "$1") ro_mode=$(artifact_mode "$1")
  ro_backup=$(backup_path "$1")
  ro_stage=$ro_directory/.agenterm-cu.restore.$ro_name.$$
  if test "$ro_prior" = present; then
    test -f "$ro_backup" && test ! -L "$ro_backup" || return 1
    install_owned "$ro_mode" "$ro_backup" "$ro_stage" || return 1
    sync_path "$ro_stage" || return 1
    mv -f "$ro_stage" "$ro_destination" || return 1
  else
    rm -f "$ro_destination" || return 1
  fi
  sync_path "$ro_directory" || return 1
}

recover_transaction() {
  test -f "$marker" && test ! -L "$marker" || return 1
  test "$(sed -n '1p' "$marker")" = agenterm-cu-provider-install-v2 || return 1
  for name in provider policy socket service; do
    prior=$(sed -n "s/^$name=//p" "$marker")
    case "$prior" in present|absent) ;; *) return 1 ;; esac
    restore_one "$name" "$prior" || return 1
  done
  prior_manifest=$(sed -n 's/^manifest=//p' "$marker")
  prior_socket_enabled=$(sed -n 's/^socket_enabled=//p' "$marker")
  prior_socket_active=$(sed -n 's/^socket_active=//p' "$marker")
  prior_service_active=$(sed -n 's/^service_active=//p' "$marker")
  case "$prior_manifest" in present|absent) ;; *) return 1 ;; esac
  case "$prior_socket_enabled" in enabled|disabled) ;; *) return 1 ;; esac
  case "$prior_socket_active" in active|inactive) ;; *) return 1 ;; esac
  case "$prior_service_active" in active|inactive) ;; *) return 1 ;; esac
  restore_stage=$state_dir/.install-manifest.restore.$$
  if test "$prior_manifest" = present; then
    test -f "$state_dir/manifest.backup" && test ! -L "$state_dir/manifest.backup" || return 1
    install_owned 0600 "$state_dir/manifest.backup" "$restore_stage" || return 1
    sync_path "$restore_stage" || return 1
    mv -f "$restore_stage" "$manifest" || return 1
  else
    rm -f "$manifest" || return 1
  fi
  "$systemctl_bin" daemon-reload || return 1
  if test "$prior_socket_enabled" = enabled; then
    "$systemctl_bin" enable "$socket_name" || return 1
  else
    "$systemctl_bin" disable "$socket_name" >/dev/null 2>&1 || true
  fi
  if test "$prior_socket_active" = active; then
    "$systemctl_bin" start "$socket_name" || return 1
  else
    "$systemctl_bin" stop "$socket_name" >/dev/null 2>&1 || true
  fi
  if test "$prior_service_active" = active; then
    "$systemctl_bin" start "$service_name" || return 1
  else
    "$systemctl_bin" stop "$service_name" >/dev/null 2>&1 || true
  fi
  rm -f "$marker"
  sync_path "$state_dir" || return 1
  rm -f "$state_dir"/*.backup
  sync_path "$state_dir" || return 1
}

inject_cut() {
  name=$1
  if test "$test_mode" -eq 1 && test "$test_interrupt_after" = "$name"; then
    trap - 0 1 2 15
    kill -KILL $$
  fi
  if test "$test_mode" -eq 1 && test "$test_fail_after" = "$name"; then fail "injected failure after $name publication"; fi
}

test "$test_mode" -eq 1 || test "$(id -u)" -eq 0 || fail "run through the system package installer as root"
action=${1-}; shift || true
case "$action" in
  install)
    test "$#" -eq 10 || fail "expected install --source FILE --sha256 DIGEST --policy-sha256 DIGEST --socket-sha256 DIGEST --service-sha256 DIGEST"
    test "${1-}" = --source || fail "expected --source"; source_path=${2-}
    test "${3-}" = --sha256 || fail "expected --sha256"; provider_expected=${4-}
    test "${5-}" = --policy-sha256 || fail "expected --policy-sha256"; policy_expected=${6-}
    test "${7-}" = --socket-sha256 || fail "expected --socket-sha256"; socket_expected=${8-}
    test "${9-}" = --service-sha256 || fail "expected --service-sha256"; service_expected=${10-}
    for digest in "$provider_expected" "$policy_expected" "$socket_expected" "$service_expected"; do
      is_sha256 "$digest" || fail "artifact digest must be lowercase SHA-256"
    done
    test -f "$source_path" && test ! -L "$source_path" || fail "source must be a regular non-symlink file"
    script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
    for name in provider policy socket service; do
      source=$(artifact_source "$name")
      test -f "$source" && test ! -L "$source" || fail "$name source is missing or linked"
      eval "expected=\${${name}_expected}"
      test "$(sha256_file "$source")" = "$expected" || fail "$name digest does not match the sealed artifact"
    done

    make_dir 0755 "$provider_dir"; make_dir 0755 "$policy_dir"; make_dir 0755 "$unit_dir"; make_dir 0700 "$state_dir"
    if test -e "$marker" || test -L "$marker"; then recover_transaction || fail "cannot recover the interrupted provider installation"
    else rm -f "$state_dir"/*.backup; fi

    transaction_active=0
    stages=
    cleanup() {
      status=$?
      trap - 0 1 2 15
      for stage in $stages; do rm -f "$stage"; done
      if test "$transaction_active" -eq 1; then
        recover_transaction || { echo "install-provider: failed to restore interrupted installation" >&2; exit 3; }
      fi
      exit "$status"
    }
    trap cleanup 0; trap 'exit 129' 1; trap 'exit 130' 2; trap 'exit 143' 15

    for name in provider policy socket service; do
      source=$(artifact_source "$name"); directory=$(artifact_dir "$name"); mode=$(artifact_mode "$name")
      stage=$directory/.agenterm-cu.install.$name.$$
      stages="$stages $stage"
      install_owned "$mode" "$source" "$stage"
      eval "expected=\${${name}_expected}"
      test "$(sha256_file "$stage")" = "$expected" || fail "staged $name digest changed"
      sync_path "$stage"
    done

    marker_stage=$state_dir/.install-transaction.$$
    manifest_stage=$state_dir/.install-manifest.$$
    stages="$stages $marker_stage $manifest_stage"
    {
      echo agenterm-cu-provider-manifest-v2
      echo "provider_sha256=$provider_expected"
      echo "policy_sha256=$policy_expected"
      echo "socket_sha256=$socket_expected"
      echo "service_sha256=$service_expected"
    } >"$manifest_stage"
    chmod 0600 "$manifest_stage"; sync_path "$manifest_stage"

    for name in provider policy socket service; do
      destination=$(artifact_path "$name"); backup=$(backup_path "$name"); mode=$(artifact_mode "$name")
      eval "prior_$name=absent"
      if test -e "$destination" || test -L "$destination"; then
        test -f "$destination" && test ! -L "$destination" || fail "installed $name is not a regular file"
        install_owned "$mode" "$destination" "$backup"; sync_path "$backup"; eval "prior_$name=present"
      fi
    done
    prior_manifest=absent
    if test -e "$manifest" || test -L "$manifest"; then
      test -f "$manifest" && test ! -L "$manifest" || fail "installed manifest is not a regular file"
      install_owned 0600 "$manifest" "$state_dir/manifest.backup"; sync_path "$state_dir/manifest.backup"; prior_manifest=present
    fi
    prior_socket_enabled=disabled
    if "$systemctl_bin" is-enabled "$socket_name" >/dev/null 2>&1; then prior_socket_enabled=enabled; fi
    prior_socket_active=inactive
    if "$systemctl_bin" is-active "$socket_name" >/dev/null 2>&1; then prior_socket_active=active; fi
    prior_service_active=inactive
    if "$systemctl_bin" is-active "$service_name" >/dev/null 2>&1; then prior_service_active=active; fi
    sync_path "$state_dir"
    {
      echo agenterm-cu-provider-install-v2
      echo "provider=$prior_provider"
      echo "policy=$prior_policy"
      echo "socket=$prior_socket"
      echo "service=$prior_service"
      echo "manifest=$prior_manifest"
      echo "socket_enabled=$prior_socket_enabled"
      echo "socket_active=$prior_socket_active"
      echo "service_active=$prior_service_active"
    } >"$marker_stage"
    chmod 0600 "$marker_stage"; sync_path "$marker_stage"; mv -f "$marker_stage" "$marker"; sync_path "$state_dir"
    transaction_active=1

    # Quiesce the old generation only after the durable recovery marker is
    # published. No old broker may execute while the four new files appear.
    "$systemctl_bin" stop "$socket_name" >/dev/null 2>&1 || true
    "$systemctl_bin" stop "$service_name" >/dev/null 2>&1 || true

    for name in provider policy socket service; do
      directory=$(artifact_dir "$name"); destination=$(artifact_path "$name"); stage=$directory/.agenterm-cu.install.$name.$$
      mv -f "$stage" "$destination"; sync_path "$directory"; inject_cut "$name"
    done
    mv -f "$manifest_stage" "$manifest"; sync_path "$state_dir"

    "$systemctl_bin" daemon-reload
    "$systemctl_bin" enable "$socket_name"
    "$systemctl_bin" start "$socket_name"
    if test "$test_mode" -eq 1 && test "$test_interrupt_after" = systemctl; then
      trap - 0 1 2 15
      kill -KILL $$
    fi
    if test "$test_mode" -eq 1 && test "$test_fail_after" = systemctl; then fail "injected failure after systemctl"; fi

    for name in provider policy socket service; do
      destination=$(artifact_path "$name"); eval "expected=\${${name}_expected}"
      test "$(sha256_file "$destination")" = "$expected" || fail "installed $name digest changed"
    done
    rm -f "$marker"; sync_path "$state_dir"; transaction_active=0
    rm -f "$state_dir"/*.backup; sync_path "$state_dir"
    trap - 0 1 2 15
    echo "installed socket-activated AgenTerm privilege broker"
    ;;
  uninstall)
    purge_state=0
    if test "${1-}" = --purge-state; then purge_state=1; shift; fi
    test "$#" -eq 0 || fail "unexpected uninstall arguments"
    if test -e "$marker" || test -L "$marker"; then
      recover_transaction || fail "cannot recover the interrupted provider installation"
    fi
    "$systemctl_bin" stop "$service_name" >/dev/null 2>&1 || true
    "$systemctl_bin" disable --now "$socket_name"
    rm -f "$policy_path" "$provider_path" "$socket_path" "$service_path"
    "$systemctl_bin" daemon-reload
    if test "$purge_state" -eq 1; then rm -rf "$state_dir"; fi
    echo "removed socket-activated AgenTerm privilege broker"
    ;;
  *) fail "expected install or uninstall" ;;
esac
