#!/usr/bin/env bash
# CEO#9: GTK entry/label copy --name; independent clipboard-read proves payload.
# Typed usage and host-limit paths must hold when --window/--name are missing
# or the AT-SPI bus is unavailable; miss paths must not publish onto the
# clipboard (independent clipboard-read read-back).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-copy-smoke-$$"
TEXT_TYPE="text/plain;charset=utf-8"
MISS_SENTINEL="ceo9-copy-miss-sentinel"
mkdir -p "$RUN"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "FAIL: DISPLAY is not set" >&2
  exit 1
fi

last_json_line() {
  python3 -c '
import json, sys
lines = sys.stdin.read().strip().splitlines()
for line in reversed(lines):
    line = line.strip()
    if line.startswith("{"):
        print(line)
        break
'
}

cu_reply() {
  "$CU" "$@" 2>&1 | last_json_line || true
}

cleanup() {
  if [[ -n "${FIXPID:-}" ]]; then
    kill -TERM "$FIXPID" 2>/dev/null || true
    wait "$FIXPID" 2>/dev/null || true
  fi
  "$CU" --target current --grant actuate clipboard-clear --apply >/dev/null 2>&1 || true
  rm -rf "$RUN"
}
trap cleanup EXIT

echo "STEP copy typed-fails usage when --window is missing"
MISSING_WINDOW="$(cu_reply --target current --grant observe,actuate copy --name "Fixture Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "invalid_input"
assert "--window" in d["error"]["message"]
' "$MISSING_WINDOW"

echo "STEP copy typed-fails usage when --role is set without --name"
ROLE_ONLY="$(cu_reply --target current --grant observe,actuate copy --window 1 --role entry)"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "invalid_input"
assert "--name" in d["error"]["message"]
' "$ROLE_ONLY"

echo "STEP copy typed-fails usage when --window handle is zero"
ZERO_HANDLE="$(cu_reply --target current --grant observe,actuate copy --window 0 --name "Fixture Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "invalid_input"
assert "--window" in d["error"]["message"]
' "$ZERO_HANDLE"

echo "STEP copy typed-fails host-limit when AT-SPI bus address is invalid"
HOST_LIMIT="$(AT_SPI_BUS_ADDRESS=invalid cu_reply --target current --grant observe,actuate copy --window 1 --name "Fixture Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"]
assert d["error"]["code"] in ("a11y_backend_failed", "unsupported", "bad_handle")
' "$HOST_LIMIT"

export GTK_MODULES=gail:atk-bridge
python3 "$ROOT/examples/python/agenterm_atspi_fixture.py" >"$RUN/fixture.log" 2>&1 &
FIXPID=$!
for _ in $(seq 1 60); do
  if grep -q "^ready $FIXPID$" "$RUN/fixture.log" 2>/dev/null; then break; fi
  sleep 0.1
done
grep -q "^ready $FIXPID$" "$RUN/fixture.log" || {
  echo "FAIL: fixture did not become ready" >&2
  cat "$RUN/fixture.log" >&2
  exit 1
}

TITLE="agenterm-linux-fixture-$FIXPID"
HANDLE="$("$CU" --target current --grant observe windows --pid "$FIXPID" | python3 -c '
import json, sys
payload = json.load(sys.stdin)
rows = payload.get("data") or []
if isinstance(rows, dict):
    rows = rows.get("windows") or []
want = sys.argv[1]
for row in rows:
    if row.get("title") == want:
        print(row["handle"])
        break
' "$TITLE")"
[[ -n "$HANDLE" ]] || { echo "FAIL: window handle missing for $TITLE" >&2; exit 1; }

echo "STEP clipboard-clear then clipboard-write publishes a miss sentinel"
"$CU" --target current --grant observe,actuate clipboard-clear --apply >/dev/null
"$CU" --target current --grant actuate clipboard-write --type "$TEXT_TYPE" --text "$MISS_SENTINEL" >/dev/null
SENTINEL_READ="$("$CU" --target current --grant observe clipboard-read)"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"] and d["data"]["text"] == sys.argv[1]
assert d["data"]["types_available"] is True
assert d["data"]["format"] == "text/plain;charset=utf-8"
' "$MISS_SENTINEL" <<<"$SENTINEL_READ"

echo "STEP copy typed-fails a11y_node_not_found when the named entry is missing"
MISSING_NODE="$(cu_reply --target current --grant observe,actuate copy --window "$HANDLE" --name "No Such Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "a11y_node_not_found"
' "$MISSING_NODE"

echo "STEP independent clipboard-read proves the miss did not publish onto the clipboard"
AFTER_MISS="$("$CU" --target current --grant observe clipboard-read)"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"] and d["data"]["text"] == sys.argv[1]
assert d["data"]["types_available"] is True
assert d["data"]["format"] == "text/plain;charset=utf-8"
' "$MISS_SENTINEL" <<<"$AFTER_MISS"

SEED="ceo9-gtk-copy-$$-$(date +%s)"

echo "STEP clear clipboard before entry copy court"
"$CU" --target current --grant observe,actuate clipboard-clear --apply >/dev/null

echo "STEP send-text --name plants SEED in the GTK entry"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==sys.argv[1] and d["data"]["addressing"]=="accessibility-tree"' "$SEED" <<<"$SENT"

echo "STEP copy --name publishes entry GetText onto native clipboard"
COPY="$("$CU" --target current --grant observe,actuate copy --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["via"]=="gettext" and d["data"]["clipboard"] is True and d["data"]["addressing"]=="accessibility-tree"' <<<"$COPY"

echo "STEP independent clipboard-read proves entry payload == SEED"
READ="$("$CU" --target current --grant observe clipboard-read)"
python3 -c '
import json, sys
seed = sys.argv[1]
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["text"] == seed
assert data["types_available"] is True
assert data["format"] == "text/plain;charset=utf-8"
' "$SEED" <<<"$READ"

echo "STEP copy --name on GTK label publishes label text onto clipboard"
LABEL_COPY="$("$CU" --target current --grant observe,actuate copy --window "$HANDLE" --name "menu idle")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["via"]=="gettext" and d["data"]["clipboard"] is True' <<<"$LABEL_COPY"

echo "STEP independent clipboard-read proves label payload == menu idle"
LABEL_READ="$("$CU" --target current --grant observe clipboard-read)"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["text"] == "menu idle"
assert data["types_available"] is True
assert data["format"] == "text/plain;charset=utf-8"
' <<<"$LABEL_READ"

echo "PASS: CEO#9 GTK entry/label copy --name with independent clipboard-read and typed usage/miss/host-limit honesty (AT-SPI --name, no --coords)"
