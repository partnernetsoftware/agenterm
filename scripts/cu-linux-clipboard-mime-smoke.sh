#!/usr/bin/env bash
# CEO: clipboard-write/read roundtrip for text/html and image/png on Linux X11.
# Acceptance is independent clipboard-read after write (digest + type inventory).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-linux-clipboard-mime-smoke-$$"
mkdir -p "$RUN"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "FAIL: DISPLAY is not set" >&2
  exit 1
fi

cleanup() {
  "$CU" --target current --grant actuate clipboard-clear --apply >/dev/null 2>&1 || true
  rm -rf "$RUN"
}
trap cleanup EXIT

json_field() {
  python3 -c 'import json,sys; d=json.load(sys.stdin); print(d'"$1"')'
}

require_ok() {
  local label="$1"
  local payload="$2"
  python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("ok") is True, d' <<<"$payload" \
    || { echo "FAIL: $label" >&2; echo "$payload" >&2; exit 1; }
}

sha256_file() {
  sha256sum "$1" | awk '{print $1}'
}

echo "STEP clear any prior clipboard state"
"$CU" --target current --grant actuate clipboard-clear --apply >/dev/null

echo "STEP prove the clipboard is exactly empty before any write"
INVENTORY="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard metadata probe" "$INVENTORY"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["types_available"] is True and data["types"]==[], data' <<<"$INVENTORY"

HTML='<p>CEO clipboard html court '"$$"'</p>'
printf '%s' "$HTML" >"$RUN/court.html"
HTML_DIGEST="$(sha256_file "$RUN/court.html")"

echo "STEP clipboard-write text/html"
WRITE_HTML="$("$CU" --target current --grant actuate clipboard-write --type text/html --path "$RUN/court.html")"
require_ok "clipboard-write text/html" "$WRITE_HTML"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["type"]=="text/html" and data["verified"] is True and data["sha256"]==sys.argv[1], data' \
  "$HTML_DIGEST" <<<"$WRITE_HTML"

echo "STEP independent clipboard-read --type text/html"
READ_HTML="$("$CU" --target current --grant observe clipboard-read --type text/html)"
require_ok "clipboard-read text/html" "$READ_HTML"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["type"]=="text/html" and data["sha256"]==sys.argv[1] and data["value"]==sys.argv[2], data' \
  "$HTML_DIGEST" "$HTML" <<<"$READ_HTML"

echo "STEP inventory lists text/html without reading payload bytes"
AFTER_HTML="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard inventory after html write" "$AFTER_HTML"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["payload_read"] is False and "text/html" in data["types"], data' <<<"$AFTER_HTML"

python3 -c 'import sys; open(sys.argv[1],"wb").write(bytes.fromhex("89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de0000000c49444154789c6360606000000004000127000b0000000049454e44ae426082"))' "$RUN/1x1.png"
PNG_DIGEST="$(sha256_file "$RUN/1x1.png")"

echo "STEP clipboard-write image/png"
WRITE_PNG="$("$CU" --target current --grant actuate clipboard-write --type image/png --path "$RUN/1x1.png")"
require_ok "clipboard-write image/png" "$WRITE_PNG"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["type"]=="image/png" and data["verified"] is True and data["sha256"]==sys.argv[1], data' \
  "$PNG_DIGEST" <<<"$WRITE_PNG"

echo "STEP independent clipboard-read --type image/png"
READ_PNG="$("$CU" --target current --grant observe clipboard-read --type image/png)"
require_ok "clipboard-read image/png" "$READ_PNG"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["type"]=="image/png" and data["sha256"]==sys.argv[1] and data["encoding"]=="base64", data' \
  "$PNG_DIGEST" <<<"$READ_PNG"

echo "STEP inventory lists image/png"
AFTER_PNG="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard inventory after png write" "$AFTER_PNG"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert "image/png" in data["types"], data' <<<"$AFTER_PNG"

echo "STEP clear back to empty and re-verify"
CLEAR="$("$CU" --target current --grant actuate clipboard-clear --apply)"
require_ok "clipboard-clear --apply" "$CLEAR"
EMPTY="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard empty inventory" "$EMPTY"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["types"]==[], data' <<<"$EMPTY"

echo "PASS: CEO Linux clipboard text/html and image/png roundtrip with independent read-back"
