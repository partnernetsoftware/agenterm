#!/usr/bin/env bash
# CEO: Linux clipboard-write/read UTF-8 honesty on X11.
# Acceptance is independent clipboard-read --type after clipboard-write with
# matching type, bytes, sha256 and value; default text view must agree.
# Optional xclip/xsel cross-check when present, otherwise a typed gap.
# No --coords or screenshot degradation.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-linux-clipboard-utf8-smoke-$$"
TEXT_TYPE="text/plain;charset=utf-8"
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

require_ok() {
  local label="$1"
  local payload="$2"
  python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("ok") is True, d' <<<"$payload" \
    || { echo "FAIL: $label" >&2; echo "$payload" >&2; exit 1; }
}

sha256_file() {
  sha256sum "$1" | awk '{print $1}'
}

echo "STEP clear clipboard to admitted empty initial state"
"$CU" --target current --grant actuate clipboard-clear --apply >/dev/null
EMPTY="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard empty inventory" "$EMPTY"
python3 -c 'import json,sys; d=json.load(sys.stdin); data=d["data"]; assert data["types_available"] is True and data["types"]==[], data' <<<"$EMPTY"

TOKEN="CEO-utf8-$$ ≠ 你好
"
printf '%s' "$TOKEN" >"$RUN/token.txt"
DIGEST="$(sha256_file "$RUN/token.txt")"

echo "STEP clipboard-write disposable UTF-8 token"
WRITE="$("$CU" --target current --grant actuate clipboard-write --type "$TEXT_TYPE" --text "$TOKEN")"
require_ok "clipboard-write utf8" "$WRITE"
python3 -c '
import json, sys
digest = sys.argv[1]
d = json.load(sys.stdin)
data = d["data"]
assert d["command"] == "clipboard-write"
assert data["type"] == "text/plain;charset=utf-8"
assert data["verified"] is True
assert data["sha256"] == digest
assert int(data["bytes"]) > 0
' "$DIGEST" <<<"$WRITE"
WRITE_BYTES="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["bytes"])' <<<"$WRITE")"

echo "STEP independent clipboard-read --type text/plain;charset=utf-8"
READ="$("$CU" --target current --grant observe clipboard-read --type "$TEXT_TYPE")"
require_ok "clipboard-read typed utf8" "$READ"
python3 -c '
import json, sys
digest, write_bytes, token_path = sys.argv[1], int(sys.argv[2]), sys.argv[3]
token = open(token_path, "rb").read().decode()
d = json.load(sys.stdin)
data = d["data"]
assert data["type"] == "text/plain;charset=utf-8"
assert data["sha256"] == digest
assert data["value"] == token
assert data["encoding"] == "utf8"
assert int(data["bytes"]) == write_bytes
' "$DIGEST" "$WRITE_BYTES" "$RUN/token.txt" <<<"$READ"

echo "STEP default clipboard-read text view agrees with typed read-back"
TEXT_VIEW="$("$CU" --target current --grant observe clipboard-read)"
require_ok "clipboard-read default text view" "$TEXT_VIEW"
python3 -c '
import json, sys
write_bytes, token_path = int(sys.argv[1]), sys.argv[2]
token = open(token_path, "rb").read().decode()
d = json.load(sys.stdin)
data = d["data"]
assert data["text"] == token
assert data["format"] == "text/plain;charset=utf-8"
assert int(data["bytes"]) == write_bytes
' "$WRITE_BYTES" "$RUN/token.txt" <<<"$TEXT_VIEW"

echo "STEP inventory lists UTF-8 flavour without reading payload bytes"
INVENTORY="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard inventory after write" "$INVENTORY"
python3 -c '
import json, sys
d = json.load(sys.stdin)
data = d["data"]
assert data["payload_read"] is False
types = data["types"]
assert "text/plain;charset=utf-8" in types or "UTF8_STRING" in types, types
' <<<"$INVENTORY"

if command -v xclip >/dev/null 2>&1; then
  echo "STEP independent xclip cross-check"
  HOST="$(xclip -selection clipboard -o)"
  python3 -c 'import sys; exp=open(sys.argv[1],"rb").read().decode(); assert sys.argv[2]==exp, (sys.argv[2], exp)' "$RUN/token.txt" "$HOST"
elif command -v xsel >/dev/null 2>&1; then
  echo "STEP independent xsel cross-check"
  HOST="$(xsel --clipboard --output)"
  python3 -c 'import sys; exp=open(sys.argv[1],"rb").read().decode(); assert sys.argv[2]==exp, (sys.argv[2], exp)' "$RUN/token.txt" "$HOST"
else
  echo "STEP typed-gap: no xclip/xsel on PATH; clipboard-read --type digest read-back is the acceptance path"
fi

echo "STEP clear back to empty and re-verify"
"$CU" --target current --grant actuate clipboard-clear --apply >/dev/null
BACK="$("$CU" --target current --grant observe clipboard-read --metadata-only)"
require_ok "clipboard restore empty inventory" "$BACK"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["data"]["types"]==[], d' <<<"$BACK"

echo "PASS: Linux clipboard-write/read UTF-8 roundtrip with independent typed read-back"
