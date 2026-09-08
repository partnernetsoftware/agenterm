#!/usr/bin/env bash
# CEO: Linux hit --window point→node honesty with independent AT-SPI name/role
# read-back. Probe points come from get-extents --name only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CU="${AGENTERM_CU_EXE:-$ROOT/target/debug/agenterm-cu}"
export AGENTERM_ABI_LIB="${AGENTERM_ABI_LIB:-$(dirname "$CU")/libagenterm.so}"
export AGENTERM_NO_ACTIVATE=1
RUN="${TMPDIR:-/tmp}/cu-hit-honesty-smoke-$$"
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

probe_named_node() {
  python3 - "$TITLE" "$1" <<'PY'
import json, sys
import pyatspi

title, name = sys.argv[1], sys.argv[2]

def node_name(obj):
    try:
        return obj.name or ""
    except Exception:
        return ""

def node_role(obj):
    try:
        rn = obj.getRoleName()
        if rn and rn.strip():
            return rn
    except Exception:
        pass
    return pyatspi.role.getName(obj.getRole()).lower()

def find_named(root, needle):
    if node_name(root) == needle:
        return root
    try:
        child_count = root.childCount
    except Exception:
        child_count = 0
    for index in range(child_count):
        try:
            child = root.getChildAtIndex(index)
        except Exception:
            child = None
        if child is None:
            continue
        found = find_named(child, needle)
        if found is not None:
            return found
    return None

for app in pyatspi.Registry.getDesktop(0):
    for index in range(app.childCount):
        window = app.getChildAtIndex(index)
        if window is not None and node_name(window) == title:
            node = find_named(app, name)
            if node is None:
                raise SystemExit("probe node missing: " + name)
            print(json.dumps({"name": node_name(node), "role": node_role(node)}))
            raise SystemExit(0)
raise SystemExit("probe window missing")
PY
}

for NAME in "Fixture Press" "Fixture Entry" "Fixture Check" "Fixture Drag Source"; do
  echo "STEP hit --window point from get-extents --name $NAME agrees with independent AT-SPI name/role"
  EXT="$("$CU" --target current --grant observe get-extents --window "$HANDLE" --name "$NAME")"
  read -r X Y < <(python3 -c '
import json, sys
ext = json.load(sys.stdin)["data"]["extents"]
print(ext["x"] + 1, ext["y"] + 1)
' <<<"$EXT")
  HIT="$("$CU" --target current --grant observe hit --window "$HANDLE" --x "$X" --y "$Y" --depth 8 --max-nodes 500)"
  PROBE="$(probe_named_node "$NAME")"
  python3 -c '
import json, sys
hit = json.load(sys.stdin)
probe = json.loads(sys.argv[1])
data = hit["data"]
assert data["resolution"] == "bounded-walk-hit-test"
assert not data.get("truncated")
node = data["node"]
assert node["name"] == probe["name"]
assert node["role"] == probe["role"]
' "$PROBE" <<<"$HIT"
done

echo "PASS: Linux hit --window point→node honesty with independent AT-SPI name/role read-back"
