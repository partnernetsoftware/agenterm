#!/usr/bin/env bash
# CEO#3: Chrome focus → send-text SEED → get-text == SEED. Read-back only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
ABI="${AGENTERM_ABI_LIB:-$ROOT/target/abi-dev/libagenterm.so}"
RUN="${TMPDIR:-/tmp}/cu-linux-chrome-text-smoke-$$"
PORT=9232
mkdir -p "$RUN"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "FAIL: DISPLAY is not set" >&2
  exit 1
fi

export AGENTERM_NO_ACTIVATE=1
export AGENTERM_ABI_LIB="$ABI"
for key in XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS AT_SPI_BUS_ADDRESS AT_SPI_BUS; do
  if [[ -n "${!key:-}" ]]; then export "$key"; fi
done
if [[ -z "${AT_SPI_BUS_ADDRESS:-}" && -n "${AT_SPI_BUS:-}" ]]; then
  export AT_SPI_BUS_ADDRESS="$AT_SPI_BUS"
fi

cleanup() {
  if [[ -n "${CPID:-}" ]]; then
    kill -TERM "$CPID" 2>/dev/null || true
    wait "$CPID" 2>/dev/null || true
  fi
  rm -rf "$RUN"
}
trap cleanup EXIT

TITLE="CEO3 Chrome Text $$-$(date +%s)"
sed \
  -e "s/311b Chrome GetText Fixture/$TITLE/" \
  -e 's/Prefill HELLO\. Independent get-text --name GetTextField must equal HELLO\./Prefill HELLO for independent get-text read-back./' \
  "$ROOT/fixtures/cu/311b-chrome-gettext.html" >"$RUN/fixture.html"

"$ROOT/scripts/box-chrome-a11y.sh" \
  --user-data-dir="$RUN/profile" \
  --remote-debugging-port="$PORT" \
  --remote-debugging-address=127.0.0.1 \
  --new-window "file://$RUN/fixture.html" >"$RUN/chrome.log" 2>&1 &
CPID=$!

for _ in $(seq 1 80); do
  curl -s --max-time 1 "http://127.0.0.1:$PORT/json/version" | grep -q Browser && break
  sleep 0.25
done
curl -s --max-time 1 "http://127.0.0.1:$PORT/json/version" | grep -q Browser || {
  echo "FAIL: Chrome CDP did not become ready" >&2
  cat "$RUN/chrome.log" >&2
  exit 1
}

WAIT="$("$CU" --target current --grant observe wait --timeout-ms 20000 --window-title-contains "$TITLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["met"]' <<<"$WAIT"

HANDLE="$("$CU" --target current --grant observe windows | python3 -c '
import json, sys
payload = json.load(sys.stdin)
rows = payload.get("data") or []
if isinstance(rows, dict):
    rows = rows.get("windows") or []
want = sys.argv[1]
for row in rows:
    title = str(row.get("title", ""))
    if row.get("app_name") == "chrome" and title.startswith(want):
        print(row["handle"])
        break
' "$TITLE")"
[[ -n "$HANDLE" ]] || { echo "FAIL: Chrome window handle missing for $TITLE" >&2; exit 1; }

SEED="ceo3-chrome-$(date +%s)"

echo "STEP focus --name GetTextField"
FOCUS="$("$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name GetTextField)"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["after"]["name"]=="GetTextField"' <<<"$FOCUS"

echo "STEP send-text --name GetTextField"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name GetTextField -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==sys.argv[1] and d["data"]["addressing"]=="accessibility-tree"' "$SEED" <<<"$SENT"

echo "STEP get-text --name GetTextField"
GOT="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name GetTextField)"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["matched"]["text"]==sys.argv[1]' "$SEED" <<<"$GOT"

echo "PASS: CEO#3 Chrome focus → send-text → get-text (AT-SPI --name, get-text == SEED)"
