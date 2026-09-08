#!/usr/bin/env bash
# CEO#7: GTK entry region + select-all by --name; independent get-selection proves slices.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-entry-selection-smoke-$$"
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

REGION="REGION"
PREFIX="pre-"
SUFFIX="-post-$$-$(date +%s)"
SEED="${PREFIX}${REGION}${SUFFIX}"
REGION_START=${#PREFIX}
REGION_END=$((REGION_START + ${#REGION}))

echo "STEP focus --name Fixture Entry"
"$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry" >/dev/null
FOCUSED="$("$CU" --target current --grant observe focused --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["node"]["name"]=="Fixture Entry"' <<<"$FOCUSED"

echo "STEP send-text --name plants SEED"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==sys.argv[1] and d["data"]["addressing"]=="accessibility-tree"' "$SEED" <<<"$SENT"

echo "STEP get-selection before select is not the REGION slice"
BEFORE="$("$CU" --target current --grant observe get-selection --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["via"]=="get-selection" and d["data"].get("text","")!=sys.argv[1]' "$REGION" <<<"$BEFORE"

echo "STEP select --start --end --name selects the interior REGION slice"
SELECTED="$("$CU" --target current --grant observe,actuate select --window "$HANDLE" --name "Fixture Entry" --start "$REGION_START" --end "$REGION_END")"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["addressing"] == "accessibility-tree"
assert data["via"] == "set-selection"
assert data["start"] == int(sys.argv[1])
assert data["end"] == int(sys.argv[2])
' "$REGION_START" "$REGION_END" <<<"$SELECTED"

echo "STEP independent get-selection --name returns the REGION scalar slice"
REGION_SEL="$("$CU" --target current --grant observe get-selection --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
region = sys.argv[1]
start = int(sys.argv[2])
end = int(sys.argv[3])
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["via"] == "get-selection"
assert data["n"] == 1
assert data["start"] == start
assert data["end"] == end
assert data["text"] == region
' "$REGION" "$REGION_START" "$REGION_END" <<<"$REGION_SEL"

echo "STEP send-keys ctrl+a --name performs semantic select-all"
KEYED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- ctrl+a)"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["addressing"]=="accessibility-tree" and d["data"]["via"]=="set-selection"' <<<"$KEYED"

echo "STEP independent get-selection --name returns full SEED slice"
SEL="$("$CU" --target current --grant observe get-selection --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
seed = sys.argv[1]
d = json.load(sys.stdin)
assert d["ok"]
data = d["data"]
assert data["via"] == "get-selection"
assert data["n"] == 1
assert data["start"] == 0
assert data["end"] == len(seed)
assert data["text"] == seed
' "$SEED" <<<"$SEL"

echo "PASS: CEO#7 GTK entry select region + select-all by --name with independent get-selection read-back"
