#!/usr/bin/env bash
# CEO: orderwin above/below with stacking read-back on two disposable GTK windows.
# AT-SPI/--name journey only; every actuation is verified from windows inventory z_index.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
RUN="${TMPDIR:-/tmp}/cu-linux-orderwin-smoke-$$"
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

TITLE1="agenterm-linux-fixture-$FIXPID"
TITLE2="agenterm-linux-second-$FIXPID"

read -r H1 H2 Z1 Z2 <<EOF
$(python3 -c '
import json, subprocess, sys
cu = sys.argv[1]
pid = sys.argv[2]
title1 = sys.argv[3]
title2 = sys.argv[4]
payload = json.loads(subprocess.check_output(
    [cu, "--target", "current", "--grant", "observe", "windows", "--pid", pid],
    text=True,
))
rows = payload.get("data") or []
if isinstance(rows, dict):
    rows = rows.get("windows") or []
by_title = {row["title"]: row for row in rows}
main = by_title[title1]
second = by_title[title2]
print(main["handle"], second["handle"], main["z_index"], second["z_index"])
' "$CU" "$FIXPID" "$TITLE1" "$TITLE2")
EOF

echo "STEP initial inventory: main z=$Z1 second z=$Z2"

echo "STEP orderwin above: second above main"
ABOVE="$("$CU" --target current --grant observe,actuate orderwin \
  --window "$H2" --relation above --relative "$H1")"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"], d
data = d["data"]
assert data["verified"] is True
after = data["after"]
assert after["window_z"] < after["relative_z"], after
' <<<"$ABOVE"

read -r Z1 Z2 <<EOF
$(python3 -c '
import json, subprocess, sys
cu = sys.argv[1]
pid = sys.argv[2]
title1 = sys.argv[3]
title2 = sys.argv[4]
payload = json.loads(subprocess.check_output(
    [cu, "--target", "current", "--grant", "observe", "windows", "--pid", pid],
    text=True,
))
rows = payload.get("data") or []
if isinstance(rows, dict):
    rows = rows.get("windows") or []
by_title = {row["title"]: row for row in rows}
print(by_title[title1]["z_index"], by_title[title2]["z_index"])
' "$CU" "$FIXPID" "$TITLE1" "$TITLE2")
EOF
[[ "$Z2" -lt "$Z1" ]] || {
  echo "FAIL: orderwin above inventory read-back: second z=$Z2 main z=$Z1" >&2
  exit 1
}
echo "STEP orderwin above inventory: second z=$Z2 main z=$Z1"

echo "STEP orderwin below: main below second"
BELOW="$("$CU" --target current --grant observe,actuate orderwin \
  --window "$H1" --relation below --relative "$H2")"
python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["ok"], d
data = d["data"]
assert data["verified"] is True
after = data["after"]
assert after["window_z"] > after["relative_z"], after
' <<<"$BELOW"

read -r Z1 Z2 <<EOF
$(python3 -c '
import json, subprocess, sys
cu = sys.argv[1]
pid = sys.argv[2]
title1 = sys.argv[3]
title2 = sys.argv[4]
payload = json.loads(subprocess.check_output(
    [cu, "--target", "current", "--grant", "observe", "windows", "--pid", pid],
    text=True,
))
rows = payload.get("data") or []
if isinstance(rows, dict):
    rows = rows.get("windows") or []
by_title = {row["title"]: row for row in rows}
print(by_title[title1]["z_index"], by_title[title2]["z_index"])
' "$CU" "$FIXPID" "$TITLE1" "$TITLE2")
EOF
[[ "$Z1" -gt "$Z2" ]] || {
  echo "FAIL: orderwin below inventory read-back: main z=$Z1 second z=$Z2" >&2
  exit 1
}
echo "STEP orderwin below inventory: main z=$Z1 second z=$Z2"

echo "PASS: cu-linux-orderwin-smoke (above and below verified from stacking read-back)"
