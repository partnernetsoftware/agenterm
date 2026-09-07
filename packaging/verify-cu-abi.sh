#!/usr/bin/env bash
# Verify one native agenterm-cu/libagenterm delivery pair through CU's public,
# structured readiness report. This performs diagnosis only; it does not launch
# the GUI, request consent, or mutate setup/runtime state.
set -euo pipefail

CU="${1:?agenterm-cu executable required}"
ABI="${2:?libagenterm dynamic library required}"

fail() {
  echo "verify-cu-abi: $*" >&2
  exit 1
}

[[ -f "$CU" && ! -L "$CU" && -x "$CU" ]] ||
  fail "CU must be a regular executable: $CU"
[[ -f "$ABI" && ! -L "$ABI" && -s "$ABI" ]] ||
  fail "ABI library must be a non-empty regular file: $ABI"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"

REPORT="$(mktemp "${TMPDIR:-/tmp}/agenterm-cu-abi.XXXXXXXX")"
trap 'rm -f "$REPORT"' EXIT HUP INT TERM
if ! AGENTERM_ABI_LIB="$ABI" AGENTERM_NO_ACTIVATE=1 \
  "$CU" --target current --grant observe doctor >"$REPORT"; then
  fail "CU doctor rejected the colocated ABI library"
fi

python3 - "$REPORT" <<'PY' || exit 1
import json
import sys

path = sys.argv[1]
try:
    with open(path, encoding="utf-8") as stream:
        reply = json.load(stream)
    abi = reply["data"]["checks"]["abi"]
    detail = abi["detail"]
except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
    raise SystemExit(f"verify-cu-abi: invalid structured doctor reply: {error}")

actual = (detail.get("major"), detail.get("minor"))
required = (detail.get("required_major"), detail.get("required_minor"))
if reply.get("ok") is not True or abi.get("status") != "available":
    raise SystemExit("verify-cu-abi: ABI readiness is not available")
if actual != required:
    raise SystemExit(
        "verify-cu-abi: packaged ABI does not exactly match CU requirements: "
        f"library={actual[0]}.{actual[1]} required={required[0]}.{required[1]}"
    )
if actual != (1, 28):
    raise SystemExit(
        f"verify-cu-abi: release contract requires ABI 1.28, got {actual[0]}.{actual[1]}"
    )
symbols = detail.get("required_symbols")
if not isinstance(symbols, int) or isinstance(symbols, bool) or symbols <= 0:
    raise SystemExit("verify-cu-abi: required-symbol readiness was not proved")
print(f"CU_ABI_OK abi={actual[0]}.{actual[1]} required_symbols={symbols}")
PY
