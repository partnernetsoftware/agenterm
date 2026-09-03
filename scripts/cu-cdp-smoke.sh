#!/usr/bin/env bash
# cu-cdp-smoke — end-to-end CDP smoke for `agenterm-cu page targets` and
# `page-js --target-id | --target-url | --target-title` against a THROWAWAY
# Chromium-family browser. It never touches the user's running browser: a
# fresh --user-data-dir under mktemp, a free loopback port, headless, and the
# process plus the directory are removed on EXIT (also on failure / Ctrl-C).
#
# Evidence (one dated line per run; PASS / FAIL(reason) / SKIP(reason)):
#   2026-09-03  FAIL  Brave Origin headless (macOS): DevTools /json answered
#               curl in 0.6 s with Content-Length, but every cu CDP verb
#               replied unsupported "CDP HTTP read failed" after 2 s.
#               Root cause (read-only finding for the Rust owner):
#               page_js::http_get_json sends `Connection: close` and reads
#               to EOF, while Chrome's DevTools HTTP server keeps the socket
#               open (a raw-socket probe saw 590 bytes then a timeout; an
#               HTTP/1.0 request got 0 bytes). The reader must honour
#               Content-Length. Steps 1-3 (launch, two tabs, A active) PASS.
#   2026-09-03  PASS  Brave Origin headless (macOS): after page_js.rs read_http_body
#               honours Content-Length / chunked framing, page targets + page-js
#               --target-title (background tab, not-found, ambiguous) all answered.
#
# What it proves (all on 127.0.0.1:<free port>, browser started with
# --remote-debugging-port, the only way any cu CDP verb can work):
#   1. `page targets --port N` lists both data: tabs (titles cu-smoke-A / B).
#   2. `page-js --port N --target-title cu-smoke-B --expression document.title`
#      returns "cu-smoke-B" while A is the active (first /json) page, and the
#      bare `page-js` (no selector) returns "cu-smoke-A" -- a background tab is
#      evaluated in place, never selected.
#   3. `--target-title cu-smoke-Z` is typed cdp_target_not_found.
#   4. `--target-title cu-smoke` (matches both) is typed cdp_target_ambiguous
#      with both candidates in error.detail.
#
# Browser: $CU_CDP_BROWSER (an executable), else /Applications/Brave Origin.app,
# else /Applications/Google Chrome.app; none present is a typed SKIP.
# Headless refuses more than one URL on argv and Brave Origin replaces the
# argv URL with its own startup page, so both tabs are opened through the
# DevTools HTTP API (PUT /json/new) and A is made active with /json/activate.
#
# Binary: $AGENTERM_CU, else `agenterm-cu` on PATH, else target/debug or
# target/abi-dev. This script never builds (other lanes may own Cargo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

skip() { echo "SKIP: cu-cdp-smoke: $*" >&2; exit 0; }
fail() { echo "FAIL: cu-cdp-smoke: $*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || skip "macOS only (browser bundle discovery)"
command -v python3 >/dev/null 2>&1 || skip "python3 not found"
command -v curl >/dev/null 2>&1 || skip "curl not found"

CU="${AGENTERM_CU:-}"
if [[ -z "$CU" ]]; then
  if command -v agenterm-cu >/dev/null 2>&1; then CU="$(command -v agenterm-cu)"
  elif [[ -x "$ROOT/target/debug/agenterm-cu" ]]; then CU="$ROOT/target/debug/agenterm-cu"
  elif [[ -x "$ROOT/target/abi-dev/agenterm-cu" ]]; then CU="$ROOT/target/abi-dev/agenterm-cu"
  else skip "no agenterm-cu binary (set AGENTERM_CU or build the crate)"; fi
fi

BROWSER="${CU_CDP_BROWSER:-}"
if [[ -z "$BROWSER" ]]; then
  for candidate in \
    "/Applications/Brave Origin.app/Contents/MacOS/Brave Origin" \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"; do
    if [[ -x "$candidate" ]]; then BROWSER="$candidate"; break; fi
  done
fi
[[ -n "$BROWSER" && -x "$BROWSER" ]] || skip "no Chromium-family browser (Brave Origin / Google Chrome) installed"
BROWSER_NAME="$(basename "$BROWSER")"

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
UD="$(mktemp -d "${TMPDIR:-/tmp}/cu-cdp-smoke.XXXXXX")"
export AGENTERM_CU_AUDIT_PATH="$UD/audit.jsonl"
BPID=""
cleanup() {
  if [[ -n "$BPID" ]] && kill -0 "$BPID" 2>/dev/null; then
    kill "$BPID" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$BPID" 2>/dev/null || break; sleep 0.2; done
    kill -9 "$BPID" 2>/dev/null || true
    wait "$BPID" 2>/dev/null || true
  fi
  rm -rf "$UD"
}
trap cleanup EXIT INT TERM

BASE="http://127.0.0.1:$PORT"
"$BROWSER" \
  --user-data-dir="$UD/profile" \
  --remote-debugging-port="$PORT" \
  --no-first-run --no-default-browser-check --disable-gpu \
  --headless=new about:blank >"$UD/browser.log" 2>&1 &
BPID=$!

ready=0
for _ in $(seq 1 100); do
  if curl -sf "$BASE/json/version" >/dev/null 2>&1; then ready=1; break; fi
  kill -0 "$BPID" 2>/dev/null || break
  sleep 0.2
done
[[ "$ready" == 1 ]] || fail "$BROWSER_NAME did not answer /json/version on $PORT ($(head -c 300 "$UD/browser.log" | tr '\n' ' '))"
echo "STEP 1 browser: $BROWSER_NAME headless, throwaway profile, CDP on 127.0.0.1:$PORT"

# Two tabs with distinct titles, both created through DevTools (see header).
curl -s -X PUT "$BASE/json/new?data:text/html,<title>cu-smoke-A</title>A" >/dev/null
curl -s -X PUT "$BASE/json/new?data:text/html,<title>cu-smoke-B</title>B" >/dev/null

# Wait until both titles are published, then make A the active (first) page.
ids=""
for _ in $(seq 1 50); do
  ids="$(curl -s "$BASE/json" | python3 -c '
import json, sys
a = b = None
for t in json.load(sys.stdin):
    if t.get("type") != "page": continue
    if t.get("title") == "cu-smoke-A": a = t["id"]
    if t.get("title") == "cu-smoke-B": b = t["id"]
print(f"{a} {b}" if a and b else "")')"
  [[ -n "$ids" ]] && break
  sleep 0.2
done
[[ -n "$ids" ]] || fail "tabs cu-smoke-A / cu-smoke-B did not appear in /json"
ID_A="${ids%% *}"; ID_B="${ids##* }"
curl -s "$BASE/json/activate/$ID_A" >/dev/null
first="$(curl -s "$BASE/json" | python3 -c 'import json,sys; print([t for t in json.load(sys.stdin) if t.get("type")=="page"][0]["title"])')"
[[ "$first" == "cu-smoke-A" ]] || fail "expected cu-smoke-A to be the first /json page after activate, got: $first"
echo "STEP 2 tabs: A=$ID_A (active) B=$ID_B"

cu() { "$CU" --target current --grant observe "$@"; }

# JSON helpers: field <json> <dotted.path>; expects <json> <path> <value>.
jf() {
  python3 -c '
import json, sys
cur = json.loads(sys.argv[1])
for part in sys.argv[2].split("."):
    cur = cur[int(part)] if isinstance(cur, list) else cur.get(part)
    if cur is None: break
print("" if cur is None else (json.dumps(cur) if isinstance(cur, (dict, list)) else cur))' "$1" "$2"
}

# 1. page targets lists both.
OUT="$(cu page targets --port "$PORT")"
[[ "$(jf "$OUT" ok)" == "True" ]] || fail "page targets: $OUT"
titles="$(jf "$OUT" data.targets | python3 -c 'import json,sys; print(" ".join(sorted(t["title"] for t in json.load(sys.stdin) if t.get("type")=="page")))')"
[[ "$titles" == *"cu-smoke-A"* && "$titles" == *"cu-smoke-B"* ]] || fail "page targets missed a tab: $titles"
echo "STEP 3 page targets --port $PORT: pages=$(jf "$OUT" data.pages) titles=[$titles]"

# 2. background tab by title; bare selector keeps the active page.
OUT="$(cu page-js --port "$PORT" --target-title cu-smoke-B --expression document.title)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.value)" == "cu-smoke-B" ]] || fail "page-js --target-title cu-smoke-B: $OUT"
[[ "$(jf "$OUT" data.target.id)" == "$ID_B" ]] || fail "page-js echoed the wrong target: $(jf "$OUT" data.target)"
OUT2="$(cu page-js --port "$PORT" --expression document.title)"
[[ "$(jf "$OUT2" ok)" == "True" && "$(jf "$OUT2" data.value)" == "cu-smoke-A" ]] || fail "bare page-js should evaluate the active page A: $OUT2"
first="$(curl -s "$BASE/json" | python3 -c 'import json,sys; print([t for t in json.load(sys.stdin) if t.get("type")=="page"][0]["title"])')"
[[ "$first" == "cu-smoke-A" ]] || fail "evaluating B changed the active page to: $first"
echo "STEP 4 page-js --target-title cu-smoke-B -> cu-smoke-B (background); bare page-js -> cu-smoke-A; A still active"

# 3. no match is typed.
OUT="$(cu page-js --port "$PORT" --target-title cu-smoke-Z --expression document.title)" || true
[[ "$(jf "$OUT" ok)" == "False" && "$(jf "$OUT" error.code)" == "cdp_target_not_found" ]] || fail "expected cdp_target_not_found: $OUT"
echo "STEP 5 --target-title cu-smoke-Z -> cdp_target_not_found"

# 4. two matches are typed with both candidates.
OUT="$(cu page-js --port "$PORT" --target-title cu-smoke --expression document.title)" || true
[[ "$(jf "$OUT" ok)" == "False" && "$(jf "$OUT" error.code)" == "cdp_target_ambiguous" ]] || fail "expected cdp_target_ambiguous: $OUT"
ncand="$(jf "$OUT" error.detail.candidates | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')"
[[ "$ncand" -ge 2 ]] || fail "cdp_target_ambiguous should list both candidates: $OUT"
echo "STEP 6 --target-title cu-smoke -> cdp_target_ambiguous ($ncand candidates)"

echo "PASS: cu-cdp-smoke ($BROWSER_NAME headless, port $PORT, 2 tabs, 4 verbs checked)"
