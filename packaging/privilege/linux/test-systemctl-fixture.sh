#!/bin/sh
set -eu
test -n "${AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL_LOG:-}" || exit 97
state=$AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL_LOG.state
socket_enabled=disabled
socket_active=inactive
service_active=inactive
if test -f "$state"; then . "$state"; fi
printf '%s\n' "$*" >>"$AGENTERM_INSTALL_PROVIDER_TEST_SYSTEMCTL_LOG"
unit=com.partnernetsoftware.agenterm.cu.privilege.socket
service=com.partnernetsoftware.agenterm.cu.privilege.service
case "${1-}:${2-}" in
  is-enabled:$unit) test "$socket_enabled" = enabled; exit ;;
  is-active:$unit) test "$socket_active" = active; exit ;;
  is-active:$service) test "$service_active" = active; exit ;;
  enable:$unit) socket_enabled=enabled ;;
  disable:$unit) socket_enabled=disabled ;;
  start:$unit) socket_active=active ;;
  stop:$unit) socket_active=inactive ;;
  start:$service) service_active=active ;;
  stop:$service) service_active=inactive ;;
  daemon-reload:) ;;
  disable:--now)
    test "${3-}" = "$unit" || exit 98
    socket_enabled=disabled
    socket_active=inactive
    ;;
  *) exit 98 ;;
esac
{
  printf 'socket_enabled=%s\n' "$socket_enabled"
  printf 'socket_active=%s\n' "$socket_active"
  printf 'service_active=%s\n' "$service_active"
} >"$state"
