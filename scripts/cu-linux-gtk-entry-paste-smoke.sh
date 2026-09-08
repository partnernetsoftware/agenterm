#!/usr/bin/env bash
# CEO#9: clipboard-write UTF-8 seed then paste --name into GTK entry.
# Independent get-text proves get-text == SEED. AT-SPI --name only.
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

SEED="ceo9-paste-$$-$(date +%s) ≠ 你好"

echo "STEP focus --name Fixture Entry"
"$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry" >/dev/null
FOCUSED="$("$CU" --target current --grant observe focused --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["node"]["name"]=="Fixture Entry"' <<<"$FOCUSED"

echo "STEP clear entry so paste read-back is unambiguous"
"$CU" --target current --grant observe,actuate send-text --window "$HANDLE" --name "Fixture Entry" -- "" >/dev/null
CLEARED="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["text"]==""' <<<"$CLEARED"

echo "STEP clipboard-write publishes UTF-8 SEED"
WRITTEN="$("$CU" --target current --grant actuate clipboard-write --type "$TEXT_TYPE" --text "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["type"]==sys.argv[1] and d["data"]["verified"] is True' "$TEXT_TYPE" <<<"$WRITTEN"

echo "STEP paste --name writes clipboard SEED through AT-SPI"
PASTED="$("$CU" --target current --grant observe,actuate paste --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); seed=sys.argv[1]; assert d["ok"] and d["data"]["typed"]==seed and d["data"]["addressing"]=="accessibility-tree" and d["data"]["clipboard"] is True' "$SEED" <<<"$PASTED"

echo "STEP independent get-text --name reads SEED back"
GOT="$("$CU" --target current --grant observe get-text --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); seed=sys.argv[1]; assert d["ok"] and d["data"]["text"]==seed and d["data"]["via"]=="gettext"' "$SEED" <<<"$GOT"

echo "PASS: CEO#9 GTK entry clipboard-write → paste --name → get-text (AT-SPI --name, get-text == SEED)"
