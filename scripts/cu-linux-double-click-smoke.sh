#!/usr/bin/env bash
# CEO: Linux GTK double-click by --name via click --count 2 and dclick alias.
# Independent pyatspi read-back only; CU tree is not used for effect proof.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-double-click-smoke-$$"
mkdir -p "$RUN"

export AGENTERM_NO_ACTIVATE=1
export AGENTERM_ABI_LIB="$(dirname "$CU")/libagenterm.so"

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

WAIT="$("$CU" --target current --grant observe wait --timeout-ms 15000 --window-title-contains "$TITLE")"
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d.get("ok") and d.get("data",{}).get("met")' <<<"$WAIT"

atspi_press_label() {
  python3 -c '
import sys
import pyatspi
title = sys.argv[1]
desktop = pyatspi.Registry.getDesktop(0)
def find_frame():
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app is None:
            continue
        for j in range(app.childCount):
            w = app.getChildAtIndex(j)
            if w is not None and w.name == title:
                return w
    return None
def walk(node):
    if node is None:
        return None
    name = node.name or ""
    if name.startswith("pressed "):
        return name
    for k in range(node.childCount):
        hit = walk(node.getChildAtIndex(k))
        if hit is not None:
            return hit
    return None
frame = find_frame()
if frame is None:
    raise SystemExit("cu_linux_double_click_atspi_no_frame")
value = walk(frame)
if value is None:
    raise SystemExit("cu_linux_double_click_atspi_probe_miss")
print(value)
' "$1"
}

wait_press_label() {
  local title="$1"
  local expected="$2"
  local value=""
  for _ in $(seq 1 80); do
    value="$(atspi_press_label "$title")"
    if [[ "$value" == "$expected" ]]; then
      echo "$value"
      return 0
    fi
    sleep 0.1
  done
  value="$(atspi_press_label "$title")"
  [[ "$value" == "$expected" ]] || {
    echo "FAIL: independent read-back expected $expected got $value" >&2
    exit 1
  }
  echo "$value"
}

actuate_until_label() {
  local expected="$1"
  shift
  local round value
  for round in 1 2; do
    local reply
    reply="$("$CU" --target current --grant observe,actuate "$@")"
    python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["ok"] and d["data"]["performed"] and d["data"]["addressing"]=="accessibility-tree" and d["data"]["clicks"]==2' <<<"$reply"
    value="$(atspi_press_label "$TITLE")"
    if [[ "$value" == "$expected" ]]; then
      echo "$value"
      return 0
    fi
    sleep 0.2
  done
  wait_press_label "$TITLE" "$expected"
}

BEFORE="$(atspi_press_label "$TITLE")"
[[ "$BEFORE" == "pressed 0" ]] || { echo "FAIL: baseline $BEFORE" >&2; exit 1; }

echo "STEP click --name Fixture Press --count 2"
AFTER_CLICK="$(actuate_until_label "pressed 2" click --window "$HANDLE" --name "Fixture Press" --count 2)"
[[ "$AFTER_CLICK" == "pressed 2" ]] || { echo "FAIL: after click --count 2 $AFTER_CLICK" >&2; exit 1; }

echo "STEP dclick --name Fixture Press"
AFTER_DCLICK="$(actuate_until_label "pressed 4" dclick --window "$HANDLE" --name "Fixture Press")"
[[ "$AFTER_DCLICK" == "pressed 4" ]] || { echo "FAIL: after dclick $AFTER_DCLICK" >&2; exit 1; }

echo "PASS: Linux GTK double-click by --name (independent pyatspi read-back, no --coords)"
