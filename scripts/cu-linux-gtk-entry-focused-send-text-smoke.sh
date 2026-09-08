#!/usr/bin/env bash
# CEO: focus --name Fixture Entry then send-text --window (no --name).
# Independent get-text --window (no --name) proves read-back == SEED.
# AT-SPI live-probe only; no --coords or screenshot degradation.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-entry-focused-send-smoke-$$"
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

SEED="ceo-focused-send-$$-$(date +%s)"

echo "STEP focus --name Fixture Entry"
FOCUS="$("$CU" --target current --grant observe,actuate focus --window "$HANDLE" --name "Fixture Entry")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["verified"] is True' <<<"$FOCUS"
FOCUSED="$("$CU" --target current --grant observe focused --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["node"]["name"]=="Fixture Entry"' <<<"$FOCUSED"

echo "STEP send-text --window without --name writes SEED into the focused control"
SENT="$("$CU" --target current --grant observe,actuate send-text --window "$HANDLE" -- "$SEED")"
python3 -c 'import json,sys; d=json.load(sys.stdin); seed=sys.argv[1]; assert d["ok"] and d["data"]["typed"]==seed and d["data"]["addressing"]=="accessibility-tree" and d["data"]["action"]=="send-text"' "$SEED" <<<"$SENT"

echo "STEP independent get-text --window without --name reads SEED back"
GOT="$("$CU" --target current --grant observe get-text --window "$HANDLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); seed=sys.argv[1]; assert d["ok"] and d["data"]["text"]==seed and d["data"]["via"]=="gettext"' "$SEED" <<<"$GOT"

echo "PASS: CEO GTK entry focus --name → send-text --window → get-text --window (AT-SPI focused path)"
