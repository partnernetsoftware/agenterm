#!/usr/bin/env bash
# CEO: Linux headless PTY + AgenTerm control-socket honesty with independent read-back.
# Proves pty-start / pty-send / pty-wait / pty-read on a live authority and typed
# pty_job_not_found when the deterministic Unix socket authority is absent. Present
# authority uses independent fuser + connect read-back on the scoped Unix socket.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PRODUCT="${AGENTERM_CU_PRODUCT_EXECUTABLE:-$ROOT/target/abi-dev/agenterm}"
CU="${AGENTERM_CU_EXE:-$ROOT/target/abi-dev/agenterm-cu}"
export AGENTERM_CU_PRODUCT_EXECUTABLE="$PRODUCT"
export AGENTERM_ABI_LIB="${AGENTERM_ABI_LIB:-$(dirname "$CU")/libagenterm.so}"
export AGENTERM_NO_ACTIVATE=1

RUN="${TMPDIR:-/tmp}/cu-linux-pty-smoke-$$"
mkdir -p "$RUN/data" "$RUN/config"
export HOME="$RUN/data"
export XDG_DATA_HOME="$RUN/data"
export XDG_CONFIG_HOME="$RUN/config"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "FAIL: DISPLAY is not set" >&2
  exit 1
fi
if [[ -z "${XDG_RUNTIME_DIR:-}" ]]; then
  echo "FAIL: XDG_RUNTIME_DIR is not set" >&2
  exit 1
fi

JOB="linux-pty-smoke-$$"
TOKEN="PTY_LINUX_READBACK_$$"
STATE_DIR="$XDG_DATA_HOME/agenterm/pty-jobs/$JOB"

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill -TERM "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  "$CU" --target current --grant actuate pty-stop "$JOB" --expect stopped >/dev/null 2>&1 || true
  "$CU" --target current --grant actuate pty-prune "$JOB" --expect stale >/dev/null 2>&1 || true
  rm -rf "$RUN"
}
trap cleanup EXIT

cu_json() {
  "$CU" "$@"
}

assert_code() {
  python3 - "$1" "$2" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
want = sys.argv[2]
code = (payload.get("error") or {}).get("code")
if code != want:
    raise SystemExit(f"expected {want}, got {code}: {payload}")
PY
}

assert_ok_command() {
  python3 - "$1" "$2" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
want = sys.argv[2]
if not payload.get("ok") or payload.get("command") != want:
    raise SystemExit(f"unexpected reply: {payload}")
PY
}

socket_path_for_scope() {
  python3 - "$1" <<'PY'
import json, sys
scope = sys.argv[1]
runtime = __import__("os").environ.get("XDG_RUNTIME_DIR", "")
print(f"{runtime}/agenterm/{scope}.sock")
PY
}

independent_socket_live() {
  local sock="$1"
  test -S "$sock" || { echo "FAIL: independent socket missing: $sock" >&2; return 1; }
  local owner
  owner="$(fuser "$sock" 2>/dev/null | tr -d ' ' || true)"
  [[ -n "$owner" ]] || { echo "FAIL: independent socket has no listener: $sock" >&2; return 1; }
  python3 - "$sock" <<'PY'
import socket, sys
path = sys.argv[1]
conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
conn.settimeout(1.0)
conn.connect(path)
conn.close()
PY
  SERVER_PID="$owner"
}

independent_socket_absent() {
  local sock="$1"
  python3 - "$sock" <<'PY'
import os, socket, sys
path = sys.argv[1]
if os.path.exists(path):
    try:
        conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        conn.settimeout(0.2)
        conn.connect(path)
    except OSError:
        raise SystemExit(0)
    raise SystemExit(f"independent socket still accepts connections: {path}")
if os.path.exists(path):
    raise SystemExit(f"independent socket path still present: {path}")
PY
}

decode_readback_token() {
  python3 - "$1" "$TOKEN" <<'PY'
import base64, json, sys
payload = json.loads(sys.argv[1])
token = sys.argv[2]
data = payload["data"]
raw = base64.b64decode(data["data_base64"])
text = raw.decode("utf-8", errors="replace")
if token not in text:
    raise SystemExit(f"independent read-back missing token in decoded bytes: {text!r}")
if token not in (data.get("utf8") or ""):
    raise SystemExit("pty-read utf8 auxiliary field disagrees with base64 read-back")
PY
}

socket_path_from_transport() {
  python3 - "$1" <<'PY'
import json, sys
detail = (json.loads(sys.argv[1]).get("error") or {}).get("detail") or {}
transport = detail.get("transport") or ""
prefix = "IPC transport Io for unix:"
if not transport.startswith(prefix):
    raise SystemExit(f"missing unix transport detail: {transport}")
rest = transport[len(prefix):]
if ": " not in rest:
    raise SystemExit(f"transport omitted socket path suffix: {transport}")
sock = rest.rsplit(": ", 1)[0]
if not sock.endswith(".sock"):
    raise SystemExit(f"unexpected socket path in transport: {sock}")
print(sock)
PY
}

assert_pty_host_limit_detail() {
  python3 - "$1" "$2" "$3" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
job = sys.argv[2]
phase = sys.argv[3]
detail = (payload.get("error") or {}).get("detail") or {}
if detail.get("control") != "unavailable" or detail.get("authority") != "unreachable":
    raise SystemExit(f"missing control-unavailable detail: {detail}")
if detail.get("limit") != "host" or detail.get("group") != "pty":
    raise SystemExit(f"missing host-limit detail: {detail}")
if detail.get("mechanism") != "agenterm-unix-control-socket":
    raise SystemExit(f"missing mechanism detail: {detail}")
if detail.get("os") != "linux":
    raise SystemExit(f"missing os detail: {detail}")
want_instance = f"ephemeral:acu-pty-{job}"
if detail.get("instance") != want_instance:
    raise SystemExit(f"instance detail mismatch: {detail.get('instance')} != {want_instance}")
alternatives = detail.get("alternatives")
if not isinstance(alternatives, list) or len(alternatives) < 2:
    raise SystemExit(f"missing alternatives detail: {detail}")
if not any("pty-start" in row for row in alternatives):
    raise SystemExit(f"missing pty-start alternative: {alternatives}")
transport = detail.get("transport") or ""
prefix = "IPC transport Io for unix:"
if not transport.startswith(prefix) or ": " not in transport[len(prefix):]:
    raise SystemExit(f"missing unix transport detail: {transport}")
if phase == "missing":
    if "No such file or directory" not in transport:
        raise SystemExit(f"missing-sock transport should report ENOENT: {transport}")
elif phase == "stale":
    if "Connection refused" not in transport:
        raise SystemExit(f"stale-sock transport should report ECONNREFUSED: {transport}")
else:
    raise SystemExit(f"unknown pty host-limit phase: {phase}")
PY
}

independent_missing_sock_absent() {
  local sock="$1"
  [[ ! -e "$sock" ]] || {
    echo "FAIL: missing authority socket path already exists on disk: $sock" >&2
    return 1
  }
  independent_socket_absent "$sock"
}

independent_stale_sock_unreachable() {
  local sock="$1"
  test -S "$sock" || { echo "FAIL: stale socket node missing on disk: $sock" >&2; return 1; }
  local owner
  owner="$(fuser "$sock" 2>/dev/null | tr -d ' ' || true)"
  [[ -z "$owner" ]] || {
    echo "FAIL: stale socket still has a listener: $sock pid=$owner" >&2
    return 1
  }
  independent_socket_absent "$sock"
}

assert_present_pty_status_readback() {
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
job, scope, sock = sys.argv[2], sys.argv[3], sys.argv[4]
data = payload.get("data") or {}
if payload.get("ok") is not True or payload.get("command") != "pty-status":
    raise SystemExit(f"unexpected pty-status reply: {payload}")
if data.get("name") != job:
    raise SystemExit(f"pty-status name mismatch: {data.get('name')} != {job}")
if data.get("server_scope_id") != scope:
    raise SystemExit(f"pty-status scope mismatch: {data.get('server_scope_id')} != {scope}")
want_instance = f"ephemeral:acu-pty-{job}"
if data.get("instance") != want_instance:
    raise SystemExit(f"pty-status instance mismatch: {data.get('instance')} != {want_instance}")
if data.get("dead") is True:
    raise SystemExit(f"present authority reported dead: {data}")
runtime = __import__("os").environ.get("XDG_RUNTIME_DIR", "")
expected_sock = f"{runtime}/agenterm/{scope}.sock"
if expected_sock != sock:
    raise SystemExit(f"present socket path mismatch: {expected_sock} != {sock}")
PY
}

independent_runtime_socket_matches() {
  local scope="$1"
  local expected="$2"
  python3 - "$scope" "$expected" <<'PY'
import glob, os, sys
scope, expected = sys.argv[1], sys.argv[2]
runtime = os.environ.get("XDG_RUNTIME_DIR", "")
pattern = os.path.join(runtime, "agenterm", f"{scope}.sock")
matches = [path for path in glob.glob(pattern) if os.path.exists(path)]
if len(matches) != 1:
    raise SystemExit(f"independent runtime socket count for {scope}: {matches}")
if matches[0] != expected:
    raise SystemExit(f"independent runtime socket path mismatch: {matches[0]} != {expected}")
PY
}

echo "STEP missing authority typed-fails pty_job_not_found before any state exists"
[[ ! -e "$STATE_DIR" ]] || { echo "FAIL: state dir pre-exists: $STATE_DIR" >&2; exit 1; }
MISSING_STATUS="$(cu_json --target current --grant observe pty-status "$JOB" 2>&1 || true)"
assert_code "$MISSING_STATUS" "pty_job_not_found"
assert_pty_host_limit_detail "$MISSING_STATUS" "$JOB" "missing"
MISSING_READ="$(cu_json --target current --grant observe pty-read "$JOB" 2>&1 || true)"
assert_code "$MISSING_READ" "pty_job_not_found"
assert_pty_host_limit_detail "$MISSING_READ" "$JOB" "missing"
MISSING_SOCK="$(socket_path_from_transport "$MISSING_READ")"
MISSING_STATUS_SOCK="$(socket_path_from_transport "$MISSING_STATUS")"
[[ "$MISSING_STATUS_SOCK" == "$MISSING_SOCK" ]] || {
  echo "FAIL: missing transport socket disagrees across observe verbs: $MISSING_STATUS_SOCK != $MISSING_SOCK" >&2
  exit 1
}
independent_missing_sock_absent "$MISSING_SOCK"

echo "STEP pty-start spawns one headless authority; independent socket read-back matches server_scope_id"
START="$(cu_json --target current --grant actuate pty-start "$JOB")"
assert_ok_command "$START" "pty-start"
SCOPE="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["status"]["server_scope_id"])' <<<"$START")"
SOCK="$(socket_path_for_scope "$SCOPE")"
[[ "$MISSING_SOCK" == "$SOCK" ]] || {
  echo "FAIL: missing transport socket disagrees with scoped path after start: $MISSING_SOCK != $SOCK" >&2
  exit 1
}
independent_runtime_socket_matches "$SCOPE" "$SOCK"
independent_socket_live "$SOCK"
PRESENT_STATUS="$(cu_json --target current --grant observe pty-status "$JOB")"
assert_present_pty_status_readback "$PRESENT_STATUS" "$JOB" "$SCOPE" "$SOCK"

echo "STEP pty-send / pty-wait / pty-read agree on one literal token with independent base64 decode"
cu_json --target current --grant actuate pty-send "$JOB" -- $'printf "'"$TOKEN"$'\\n"\n' >/dev/null
WAIT="$(cu_json --target current --grant observe pty-wait "$JOB" --contains "$TOKEN" --cursor earliest --timeout-ms 10000)"
assert_ok_command "$WAIT" "pty-wait"
READ="$(cu_json --target current --grant observe pty-read "$JOB" --cursor earliest --max-bytes 4096)"
assert_ok_command "$READ" "pty-read"
decode_readback_token "$READ"

echo "STEP killing the server leaves pty-list stale and observe verbs typed-unavailable with independent socket read-back"
kill -9 "$SERVER_PID"
unset SERVER_PID
sleep 0.3
LIST="$(cu_json --target current --grant observe pty-list)"
python3 - "$LIST" "$JOB" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
job = sys.argv[2]
rows = payload["data"]["jobs"]
match = [row for row in rows if row.get("name") == job]
if len(match) != 1 or match[0].get("state") != "stale":
    raise SystemExit(f"expected one stale row: {rows}")
PY
STALE_READ="$(cu_json --target current --grant observe pty-read "$JOB" 2>&1 || true)"
assert_code "$STALE_READ" "pty_job_not_found"
assert_pty_host_limit_detail "$STALE_READ" "$JOB" "stale"
STALE_STATUS="$(cu_json --target current --grant observe pty-status "$JOB" 2>&1 || true)"
assert_code "$STALE_STATUS" "pty_job_not_found"
assert_pty_host_limit_detail "$STALE_STATUS" "$JOB" "stale"
STALE_SOCK="$(socket_path_from_transport "$STALE_READ")"
STALE_STATUS_SOCK="$(socket_path_from_transport "$STALE_STATUS")"
[[ "$STALE_SOCK" == "$SOCK" ]] || {
  echo "FAIL: stale transport socket disagrees with scoped path: $STALE_SOCK != $SOCK" >&2
  exit 1
}
[[ "$STALE_STATUS_SOCK" == "$STALE_SOCK" ]] || {
  echo "FAIL: stale transport socket disagrees across observe verbs: $STALE_STATUS_SOCK != $STALE_SOCK" >&2
  exit 1
}
independent_stale_sock_unreachable "$STALE_SOCK"

echo "STEP pty-stop + pty-prune reclaim stale state"
cu_json --target current --grant actuate pty-stop "$JOB" --expect stopped >/dev/null 2>&1 || true
PRUNE="$(cu_json --target current --grant actuate pty-prune "$JOB" --expect stale)"
assert_ok_command "$PRUNE" "pty-prune"
[[ ! -e "$STATE_DIR" ]] || { echo "FAIL: state dir remains after prune: $STATE_DIR" >&2; exit 1; }

echo "PASS: Linux pty missing/stale/present sock host-limit read-back with independent transport, instance, and socket-node proof"
