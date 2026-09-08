#!/usr/bin/env bash
# CEO#10: GTK entry semantic modifier chords by --name; independent read-back.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-entry-send-keys-smoke-$$"
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
  "$CU" --target current --grant actuate clipboard-clear --apply >/dev/null 2>&1 || true
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

SEED="ceo10-keys-$$-$(date +%s)"
MID=4

echo "STEP focus --name Fixture Entry"
"$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry" >/dev/null
"$CU" --target current --grant actuate clipboard-clear --apply >/dev/null

echo "STEP send-text --name plants SEED"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==sys.argv[1]' "$SEED" <<<"$SENT"

echo "STEP set-caret --name away from home"
"$CU" --target current --grant observe,actuate set-caret --window "$HANDLE" --name "Fixture Entry" --offset "$MID" >/dev/null

echo "STEP send-keys ctrl+home --name"
HOMED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- ctrl+home)"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["via"]=="set-caret-offset" and d["data"]["offset"]==0' <<<"$HOMED"
CARET_HOME="$("$CU" --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["offset"]==0' <<<"$CARET_HOME"

echo "STEP send-keys ctrl+end --name"
ENDED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- ctrl+end)"
python3 -c 'import json,sys; seed=len(sys.argv[1]); d=json.load(sys.stdin); assert d["ok"] and d["data"]["offset"]==seed' "$SEED" <<<"$ENDED"
CARET_END="$("$CU" --target current --grant observe get-caret --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; seed=len(sys.argv[1]); d=json.load(sys.stdin); assert d["ok"] and d["data"]["offset"]==seed' "$SEED" <<<"$CARET_END"

echo "STEP send-keys ctrl+a --name selects full SEED"
KEYED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- ctrl+a)"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["via"]=="set-selection"' <<<"$KEYED"
SEL="$("$CU" --target current --grant observe get-selection --window "$HANDLE" --name "Fixture Entry")"
python3 -c '
import json, sys
seed = sys.argv[1]
d = json.load(sys.stdin)
assert d["ok"] and d["data"]["text"] == seed and d["data"]["start"] == 0 and d["data"]["end"] == len(seed)
' "$SEED" <<<"$SEL"

echo "STEP send-keys ctrl+c --name publishes SEED onto native clipboard"
COPIED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- ctrl+c)"
python3 -c 'import json,sys; seed=sys.argv[1]; d=json.load(sys.stdin); assert d["ok"] and d["data"]["via"]=="gettext" and d["data"]["clipboard"] and d["data"]["text"]==seed' "$SEED" <<<"$COPIED"
CLIP="$("$CU" --target current --grant observe clipboard-read)"
python3 -c 'import json,sys; seed=sys.argv[1]; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]==seed' "$SEED" <<<"$CLIP"

echo "STEP send-keys backspace --name clears entry"
CLEARED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- backspace)"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["changed"] is True' <<<"$CLEARED"
TEXT="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]==""' <<<"$TEXT"

echo "STEP send-keys ctrl+v --name pastes clipboard SEED back"
PASTED="$("$CU" --target current --grant observe,actuate send-keys --window "$HANDLE" --name "Fixture Entry" -- ctrl+v)"
python3 -c 'import json,sys; seed=sys.argv[1]; d=json.load(sys.stdin); assert d["ok"] and d["data"]["typed"]==seed and d["data"]["clipboard"]' "$SEED" <<<"$PASTED"
TEXT="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; seed=sys.argv[1]; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]==seed' "$SEED" <<<"$TEXT"

echo "PASS: CEO#10 GTK entry semantic modifier chords ctrl+a/c/v by --name (get-selection/clipboard-read/get-text read-back)"
