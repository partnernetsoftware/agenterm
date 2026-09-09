#!/usr/bin/env bash
# CEO#9: clipboard-write UTF-8 seed then paste --name into GTK entry.
# Independent get-text proves get-text == SEED. Typed usage and host-limit
# paths must hold when --window/--name are missing or the AT-SPI bus is
# unavailable; miss paths must not mutate the entry.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-entry-paste-smoke-$$"
TEXT_TYPE="text/plain;charset=utf-8"
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

echo "STEP paste typed-fails usage when --window is missing"
MISSING_WINDOW="$(cu_reply --target current --grant observe,actuate paste --name "Fixture Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "usage"
assert "--window" in d["error"]["message"]
' "$MISSING_WINDOW"

echo "STEP paste typed-fails usage when --role is set without --name"
ROLE_ONLY="$(cu_reply --target current --grant observe,actuate paste --window 1 --role entry)"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "usage"
assert "--name" in d["error"]["message"]
' "$ROLE_ONLY"

echo "STEP paste typed-fails usage when --window handle is zero"
ZERO_HANDLE="$(cu_reply --target current --grant observe,actuate paste --window 0 --name "Fixture Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "usage"
assert "--window" in d["error"]["message"]
' "$ZERO_HANDLE"

echo "STEP paste typed-fails host-limit when AT-SPI bus address is invalid"
HOST_LIMIT="$(AT_SPI_BUS_ADDRESS=invalid cu_reply --target current --grant observe,actuate paste --window 1 --name "Fixture Entry")"
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

echo "STEP paste typed-fails a11y_node_not_found when the named entry is missing"
MISSING_NODE="$(cu_reply --target current --grant observe,actuate paste --window "$HANDLE" --name "No Such Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "a11y_node_not_found"
' "$MISSING_NODE"

echo "STEP independent get-text proves the entry stayed at the fixture seed after the miss"
SEED_CHECK="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]=="seed" and d["data"]["via"]=="gettext"' <<<"$SEED_CHECK"

echo "STEP paste typed-fails a11y_text_unavailable when the target has no EditableText interface"
"$CU" --target current --grant actuate clipboard-write --type "$TEXT_TYPE" --text "probe-unavailable" >/dev/null
UNAVAILABLE="$(cu_reply --target current --grant observe,actuate paste --window "$HANDLE" --name "Fixture Check")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "a11y_text_unavailable"
' "$UNAVAILABLE"

echo "STEP independent get-text proves the unavailable paste did not mutate the entry"
AFTER_UNAVAILABLE="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]=="seed" and d["data"]["via"]=="gettext"' <<<"$AFTER_UNAVAILABLE"

SEED="ceo9-paste-$$-$(date +%s) ≠ 你好"

echo "STEP focus --name Fixture Entry"
"$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry" >/dev/null
FOCUSED="$("$CU" --target current --grant observe focused --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["node"]["name"]=="Fixture Entry"' <<<"$FOCUSED"

echo "STEP clear entry so paste read-back is unambiguous"
"$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "" >/dev/null
CLEARED="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]=="" and d["data"]["via"]=="gettext"' <<<"$CLEARED"

echo "STEP clipboard-clear before publishing UTF-8 SEED"
"$CU" --target current --grant observe,actuate clipboard-clear --apply >/dev/null

echo "STEP clipboard-write publishes UTF-8 SEED"
WRITTEN="$("$CU" --target current --grant actuate clipboard-write --type "$TEXT_TYPE" --text "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["type"]==sys.argv[1] and d["data"]["verified"] is True' "$TEXT_TYPE" <<<"$WRITTEN"

echo "STEP paste --name writes clipboard SEED through AT-SPI"
PASTED="$("$CU" --target current --grant observe,actuate paste --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); seed=sys.argv[1]; assert d["ok"] and d["data"]["typed"]==seed and d["data"]["addressing"]=="accessibility-tree" and d["data"]["clipboard"] is True' "$SEED" <<<"$PASTED"

echo "STEP independent get-text --name reads SEED back"
GOT="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); seed=sys.argv[1]; assert d["ok"] and d["data"]["text"]==seed and d["data"]["via"]=="gettext"' "$SEED" <<<"$GOT"

echo "PASS: CEO#9 GTK entry clipboard-write → paste --name → get-text with usage/miss/host-limit honesty (AT-SPI --name, independent get-text == SEED)"
