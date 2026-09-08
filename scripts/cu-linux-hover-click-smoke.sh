#!/usr/bin/env bash
# CEO#1: hover then click the same GTK button by --name. Read-back only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-hover-click-smoke-$$"
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

label_text() {
  "$CU" --target current --grant observe tree --window "$HANDLE" --depth 8 | python3 -c '
import json, sys
nodes = json.load(sys.stdin)["data"]["nodes"]
for node in nodes:
    if node.get("role") == "label" and str(node.get("name", "")).startswith(sys.argv[1]):
        print(node["name"])
        break
' "$1"
}

BEFORE="$(label_text "pressed ")"
[[ "$BEFORE" == "pressed 0" ]] || { echo "FAIL: baseline $BEFORE" >&2; exit 1; }

echo "STEP hover --name Fixture Press"
HOVER="$("$CU" --target current --grant observe,actuate hover --window "$HANDLE" --name "Fixture Press")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["performed"] and d["data"]["addressing"]=="accessibility-tree"' <<<"$HOVER"

MID="$(label_text "pressed ")"
[[ "$MID" == "pressed 0" ]] || { echo "FAIL: after hover only $MID" >&2; exit 1; }

echo "STEP click --name Fixture Press"
CLICK="$("$CU" --target current --grant observe,actuate click --window "$HANDLE" --name "Fixture Press")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["performed"] and d["data"]["addressing"]=="accessibility-tree"' <<<"$CLICK"

AFTER="$(label_text "pressed ")"
[[ "$AFTER" == "pressed 1" ]] || { echo "FAIL: after hover+click label is $AFTER" >&2; exit 1; }

echo "PASS: CEO#1 Linux AT-SPI hover then click by --name (count label read-back)"
