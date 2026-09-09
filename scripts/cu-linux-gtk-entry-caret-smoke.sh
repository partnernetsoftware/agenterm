#!/usr/bin/env bash
# CEO#9: GTK entry set-caret / get-caret roundtrip by --name.
# Independent get-caret must return the set offset and agree with a separate
# pyatspi Text.CaretOffset probe. Typed usage and host-limit paths must hold
# when the named entry is missing or the target has no Text caret interface.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-entry-caret-smoke-$$"
mkdir -p "$RUN"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "FAIL: DISPLAY is not set" >&2
  exit 1
fi

cleanup() {
  if [[ -n "${FIXPID:-}" ]]; then
    kill -TERM "$FIXPID" 2>/dev/null || true
    wait "$FIXPID" 2>/dev/null || true
  fi
  rm -rf "$RUN"
}
trap cleanup EXIT

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

probe_caret_offset() {
  python3 -c '
import json, sys
import pyatspi

title = sys.argv[1]
pattern = sys.argv[2]

def caret_of_node(obj):
    try:
        text_iface = obj.queryText()
        return int(text_iface.caretOffset)
    except Exception:
        return None

def find_frame():
    desktop = pyatspi.Registry.getDesktop(0)
    for index in range(desktop.childCount):
        app = desktop.getChildAtIndex(index)
        if app is None:
            continue
        for child_index in range(app.childCount):
            window = app.getChildAtIndex(child_index)
            if window is not None and (window.name or "") == title:
                return window
    return None

def named_caret(root, needle):
    hits = []
    def walk(obj):
        name = obj.name or ""
        if needle in name:
            caret = caret_of_node(obj)
            if caret is not None:
                hits.append(caret)
        for index in range(obj.childCount):
            child = obj.getChildAtIndex(index)
            if child is not None:
                walk(child)
    walk(root)
    if len(hits) != 1:
        raise SystemExit("probe_named_count:" + str(len(hits)))
    return hits[0]

frame = find_frame()
if frame is None:
    raise SystemExit("probe_window_missing")
print(named_caret(frame, pattern))
' "$TITLE" "$2"
}

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

echo "STEP get-caret typed-fails usage when --name is missing"
MISSING_NAME="$(cu_reply --target current --grant observe get-caret --window "$HANDLE")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "usage"
assert "--name" in d["error"]["message"]
' "$MISSING_NAME"

echo "STEP get-caret typed-fails a11y_node_not_found when the named entry is missing"
MISSING_NODE="$(cu_reply --target current --grant observe get-caret --window "$HANDLE" --name "No Such Entry")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"] and d["error"]["code"] == "a11y_node_not_found"
' "$MISSING_NODE"

echo "STEP get-caret typed-fails when the named control has no Text caret interface"
UNAVAILABLE="$(cu_reply --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Check")"
python3 -c '
import json, sys
d = json.loads(sys.argv[1])
assert not d["ok"]
code = d["error"]["code"]
assert code in ("a11y_caret_unavailable", "a11y_backend_failed")
' "$UNAVAILABLE"

SEED="ceo9-caret-$$-$(date +%s)"
CARET_OFFSET=7
END_OFFSET="${#SEED}"

echo "STEP focus --name Fixture Entry"
"$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry" >/dev/null
FOCUSED="$("$CU" --target current --grant observe focused --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["node"]["name"]=="Fixture Entry"' <<<"$FOCUSED"

echo "STEP send-text --name plants SEED"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==sys.argv[1] and d["data"]["addressing"]=="accessibility-tree"' "$SEED" <<<"$SENT"

echo "STEP independent get-caret baseline before set-caret (not proof of placement)"
BEFORE="$("$CU" --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"] and d["data"]["via"] == "get-caret-offset"
' <<<"$BEFORE"

echo "STEP set-caret --name --offset N"
PLACED="$("$CU" --target current --grant observe,actuate set-caret --window "$HANDLE" --name "Fixture Entry" --offset "$CARET_OFFSET")"
python3 -c '
import json, sys
target = int(sys.argv[1])
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["addressing"] == "accessibility-tree"
assert data["via"] == "set-caret-offset"
assert data["offset"] == target
' "$CARET_OFFSET" <<<"$PLACED"

echo "STEP independent get-caret --name returns the set offset and agrees with pyatspi CaretOffset"
CARET="$("$CU" --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
target = int(sys.argv[1])
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["via"] == "get-caret-offset"
assert data["offset"] == target
' "$CARET_OFFSET" <<<"$CARET"
PROBE_OFFSET="$(probe_caret_offset "$TITLE" "Fixture Entry")"
[[ "$PROBE_OFFSET" == "$CARET_OFFSET" ]] || {
  echo "FAIL: pyatspi CaretOffset mismatch: cu=$CARET_OFFSET probe=$PROBE_OFFSET" >&2
  exit 1
}

echo "STEP set-caret --name --offset end-of-SEED"
PLACED_END="$("$CU" --target current --grant observe,actuate set-caret --window "$HANDLE" --name "Fixture Entry" --offset "$END_OFFSET")"
python3 -c '
import json, sys
target = int(sys.argv[1])
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["via"] == "set-caret-offset"
assert data["offset"] == target
' "$END_OFFSET" <<<"$PLACED_END"

echo "STEP independent get-caret --name returns end offset and agrees with pyatspi CaretOffset"
CARET_END="$("$CU" --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
target = int(sys.argv[1])
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["via"] == "get-caret-offset"
assert data["offset"] == target
' "$END_OFFSET" <<<"$CARET_END"
PROBE_END="$(probe_caret_offset "$TITLE" "Fixture Entry")"
[[ "$PROBE_END" == "$END_OFFSET" ]] || {
  echo "FAIL: pyatspi CaretOffset end mismatch: cu=$END_OFFSET probe=$PROBE_END" >&2
  exit 1
}

echo "PASS: CEO#9 GTK entry set-caret / get-caret roundtrip by --name with pyatspi CaretOffset read-back and typed host-limit paths"
