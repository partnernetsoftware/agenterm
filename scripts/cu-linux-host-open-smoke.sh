#!/usr/bin/env bash
# CEO: Linux host-open --app via desktop entry and PATH; independent read-back only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DISPLAY="${DISPLAY:-:2}"
export DISPLAY
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/xdg-runtime-box-2}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "FAIL: DISPLAY is not set" >&2
  exit 1
fi

exec env AGENTERM_BOOTSTRAP_TASK=cu-linux-host-open-smoke "$ROOT/scripts/bootstrap.sh"
