#!/usr/bin/env bash
# CEO#9: GTK entry set-caret / get-caret roundtrip by --name.
# Independent get-caret must return the set offset.
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

SEED="ceo9-caret-$$-$(date +%s)"
CARET_OFFSET=3

echo "STEP focus --name Fixture Entry"
"$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry" >/dev/null
FOCUSED="$("$CU" --target current --grant observe focused --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["node"]["name"]=="Fixture Entry"' <<<"$FOCUSED"

echo "STEP send-text --name plants SEED"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==sys.argv[1] and d["data"]["addressing"]=="accessibility-tree"' "$SEED" <<<"$SENT"

echo "STEP independent get-caret before set-caret is not the target offset"
BEFORE="$("$CU" --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
target = int(sys.argv[1])
d = json.load(sys.stdin)
assert d["ok"] and d["data"]["via"] == "get-caret-offset"
assert d["data"]["offset"] != target
' "$CARET_OFFSET" <<<"$BEFORE"

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

echo "STEP independent get-caret --name returns the set offset"
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

echo "PASS: CEO#9 GTK entry set-caret / get-caret roundtrip by --name (independent offset read-back)"
