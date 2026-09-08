#!/usr/bin/env bash
# CEO#9: GTK entry/label copy --name; independent clipboard-read proves payload.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-gtk-copy-smoke-$$"
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

SEED="ceo9-gtk-copy-$$-$(date +%s)"

echo "STEP clear clipboard before entry copy court"
"$CU" --target current --grant actuate clipboard-clear --apply >/dev/null

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

echo "PASS: CEO#9 GTK entry/label copy --name with independent clipboard-read (AT-SPI --name, no --coords)"
