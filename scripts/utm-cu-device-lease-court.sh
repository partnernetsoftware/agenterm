#!/bin/bash
# Run the public device lease/I/O qjswasm journey in one native Linux UTM court.

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: scripts/utm-cu-device-lease-court.sh lnx-{aarch64,x86_64}-desktop PROFILE_DIR" >&2
  exit 2
fi
case "$1" in
  lnx-aarch64-desktop|lnx-x86_64-desktop) ;;
  *) echo "device lease fixture requires a native Linux UTM court" >&2; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export AGENTERM_UTM_TASK=cu-device-lease-smoke
export AGENTERM_UTM_EVIDENCE=cu.device-lease
export AGENTERM_UTM_PASS_LINE='PASS: device claim, replay, authority refusal, exact I/O, renewal, release and session cleanup'
exec "$SCRIPT_DIR/utm-cu-managed-job-court.sh" "$@"
