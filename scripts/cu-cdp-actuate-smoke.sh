#!/usr/bin/env bash
# cu-cdp-actuate-smoke — proof that agenterm-cu reads AND acts on a
# BACKGROUND tab over CDP without changing which tab or window is active.
# Runs against a THROWAWAY Chromium-family browser only: a fresh
# --user-data-dir under mktemp, a free loopback port, headless, and the
# process plus the directory are removed on EXIT (also on failure / Ctrl-C).
# The user's running browser is never touched, quit, or asked for a port.
#
# Evidence (one dated line per run; PASS / FAIL(reason) / SKIP(reason);
# never a host path):
#   2026-09-03  PASS  Brave Origin headless (macOS): two data: tabs, A active,
#               every CDP verb run on B (page nav to the fixture page, page
#               text, page find by text / selector / role, page fill --clear,
#               page click by text, page hover + page scroll with trusted
#               event/offset read-back, page fill --node + --submit, page
#               screenshot); after each verb /json still listed A first and
#               `windows --focused` was unchanged; the button's onclick
#               mutated the DOM and was read back through page-js; receipts
#               listed page-nav / page-fill / page-click completed.
#   2026-09-04  PASS  Google Chrome headless (macOS): the same background-tab
#               court plus page-hover trusted-event target read-back and
#               page-scroll exact-container event/offset read-back and
#               page-drag trusted held-sequence/business read-back and
#               page-dialog closed-event/redaction and page-files exact
#               FileList read-back; `--match` unique selection plus typed
#               ambiguity; node and viewport-point click; current-focus type;
#               18 active-target + front-window
#               invariants stayed green; eight
#               actuator receipt kinds completed.
#
# What it proves, in order (all on 127.0.0.1:<free port>):
#   1. Two tabs: A (active, first /json page) and B (background).
#   2. `page nav --target-id B --url data:...` loads the fixture page in B
#      (verified by Page.loadEventFired, final_title read back).
#   3. `page text --match cu-actuate-B` selects B across title/URL/description,
#      returns backend "cdp" rows and fails typed when a pattern hits both tabs.
#   4. `page find` by --text (lifted to the button), --selector, --role;
#      cdp_node_not_found on a miss; cdp_node_ambiguous when four nodes
#      match a click selector (nothing dispatched).
#   5. `page fill --selector '#q' --text hello --clear` verified by .value
#      read-back.
#   6. `page click --text Go` performed + verified: the button's onclick
#      rewrote #out, which page-js reads back as "clicked:hello".
#   7. `page hover` verifies the trusted mousemove event target; `page
#      scroll` verifies the exact container offset changed.
#   8. `page files` sets one local fixture without a picker and verifies its
#      basename/size without persisting the path.
#   9. `page drag` holds left from one rendered element to another, verifies
#      the trusted event sequence and reads the fixture's business result.
#  10. `page dialog --text` waits for and accepts a real prompt, verifies its
#      close event and persists byte counts rather than the response.
#  11. `page fill --node N --text ' world' --submit` appends and submits
#      (the form's onsubmit rewrites #out -> "submitted:hello world").
#  12. `page screenshot --target-id B --out` writes a PNG, or answers the
#      typed cdp_screenshot_unavailable -- either way without activating.
#  13. After EVERY verb: /json lists A first (B never became active) and
#      `windows --focused` reports the same front window as before the run
#      (or "none" both times on an unattended session).
#  14. `receipts` lists every actuation as completed lines.
#
# Browser: $CU_CDP_BROWSER (an executable), else /Applications/Brave Origin.app,
# else /Applications/Google Chrome.app; none present is a typed SKIP.
# Binary: $AGENTERM_CU, else `agenterm-cu` on PATH, else target/debug or
# target/abi-dev. This script never builds (other lanes may own Cargo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

skip() { echo "SKIP: cu-cdp-actuate-smoke: $*" >&2; exit 0; }
fail() { echo "FAIL: cu-cdp-actuate-smoke: $*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || skip "macOS only (browser bundle discovery)"
command -v python3 >/dev/null 2>&1 || skip "python3 not found"
command -v curl >/dev/null 2>&1 || skip "curl not found"
curl_local() { curl --noproxy '*' "$@"; }

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
UD="$(mktemp -d "${TMPDIR:-/tmp}/cu-cdp-actuate-smoke.XXXXXX")"
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
  if curl_local -sf "$BASE/json/version" >/dev/null 2>&1; then ready=1; break; fi
  kill -0 "$BPID" 2>/dev/null || break
  sleep 0.2
done
[[ "$ready" == 1 ]] || fail "$BROWSER_NAME did not answer /json/version on $PORT ($(head -c 300 "$UD/browser.log" | tr '\n' ' '))"
echo "STEP 1 browser: $BROWSER_NAME headless, throwaway profile, CDP on 127.0.0.1:$PORT"

# JSON helpers: jf <json> <dotted.path>.
jf() {
  python3 -c '
import json, sys
cur = json.loads(sys.argv[1])
for part in sys.argv[2].split("."):
    cur = cur[int(part)] if isinstance(cur, list) else cur.get(part)
    if cur is None: break
print("" if cur is None else (json.dumps(cur) if isinstance(cur, (dict, list)) else cur))' "$1" "$2"
}

# Two tabs through DevTools (headless refuses more than one argv URL).
curl_local -s -X PUT "$BASE/json/new?data:text/html,<title>cu-smoke-A</title>A" >/dev/null
curl_local -s -X PUT "$BASE/json/new?data:text/html,<title>cu-smoke-B</title>B" >/dev/null
ids=""
for _ in $(seq 1 50); do
  ids="$(curl_local -s "$BASE/json" | python3 -c '
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
curl_local -s "$BASE/json/activate/$ID_A" >/dev/null

active_title() {
  curl_local -s "$BASE/json" | python3 -c 'import json,sys; print([t for t in json.load(sys.stdin) if t.get("type")=="page"][0]["title"])'
}
[[ "$(active_title)" == "cu-smoke-A" ]] || fail "expected cu-smoke-A to be the first /json page after activate, got: $(active_title)"

obs() { "$CU" --target current --grant observe "$@"; }
act() { "$CU" --target current --grant observe,actuate "$@"; }

# The user's front window, as the inventory sees it (handle + title), or
# "none" when the session reports no focused window (unattended host).
front_window() {
  local out
  out="$(obs windows --focused 2>/dev/null || true)"
  python3 -c '
import json, sys
try:
    reply = json.loads(sys.argv[1])
except Exception:
    print("unreadable"); sys.exit(0)
data = reply.get("data") or {}
rows = data if isinstance(data, list) else data.get("windows") or []
if not reply.get("ok") or not rows:
    print("none"); sys.exit(0)
print(" | ".join("%s:%s" % (r.get("handle"), r.get("app_name", "")) for r in rows))' "$out"
}
FRONT0="$(front_window)"
echo "STEP 2 tabs: A=$ID_A (active) B=$ID_B (background); front window before: $FRONT0"

# After every verb: A is still the active page target, the front window is
# unchanged, and the reply (when ok) said focus_changed false.
CHECKS=0
still_background() {
  local step="$1" reply="$2" now
  now="$(active_title)"
  [[ "$now" == "cu-smoke-A" ]] || fail "$step made another tab active: /json lists $now first"
  local front; front="$(front_window)"
  [[ "$front" == "$FRONT0" ]] || fail "$step changed the front window: before [$FRONT0] after [$front]"
  if [[ "$(jf "$reply" ok)" == "True" ]]; then
    [[ "$(jf "$reply" data.focus_changed)" == "False" ]] || fail "$step reply lacks focus_changed:false: $reply"
    [[ "$(jf "$reply" data.target.id)" == "$ID_B" ]] || fail "$step reply names the wrong target: $(jf "$reply" data.target)"
  fi
  CHECKS=$((CHECKS + 1))
}

# 1. page nav loads the fixture page in B: a heading, a form with an input
#    and a button whose onclick mutates the DOM, and a paragraph to read back.
FIXTURE="$(python3 -c '
import urllib.parse
html = """<!doctype html><title>cu-actuate-B</title><h1>Hello B</h1>
<form onsubmit="document.getElementById(&quot;out&quot;).textContent=&quot;submitted:&quot;+document.getElementById(&quot;q&quot;).value;return false">
<input id="q" placeholder="Query"><button id="go" type="button" onclick="document.getElementById(&quot;out&quot;).textContent=&quot;clicked:&quot;+document.getElementById(&quot;q&quot;).value">Go</button>
</form><p id="out">idle</p><input id="upload" type="file">
<div id="drag-from" style="display:inline-block;width:80px;height:40px;background:#ccc">From</div>
<div id="drag-to" style="display:inline-block;width:80px;height:40px;background:#ddd">To</div>
<script>let dragStarted=false;document.getElementById("drag-from").onmousedown=e=>{dragStarted=e.isTrusted};document.getElementById("drag-to").onmouseup=e=>{if(dragStarted&&e.isTrusted){document.body.dataset.drag="done";document.getElementById("out").textContent="dragged"}}</script>
<div id="scroll" style="height:120px;overflow:auto"><div style="height:1000px">Tall</div></div>"""
print("data:text/html," + urllib.parse.quote(html, safe=""))')"
OUT="$(act page nav --port "$PORT" --target-id "$ID_B" --url "$FIXTURE" --wait-ms 5000)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page nav: $OUT"
[[ "$(jf "$OUT" data.final_title)" == "cu-actuate-B" ]] || fail "page nav final_title: $(jf "$OUT" data.final_title)"
still_background "page nav" "$OUT"
echo "STEP 3 page nav --target-id B -> verified (load event), final_title=cu-actuate-B; A still active"

# 2. page text over CDP: the same row shape, backend cdp, node ids.
OUT="$(obs page text --port "$PORT" --match cu-actuate-B)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.backend)" == "cdp" ]] || fail "page text: $OUT"
texts="$(jf "$OUT" data.rows | python3 -c 'import json,sys; print("|".join(r["text"] for r in json.load(sys.stdin)))')"
[[ "$texts" == *"Hello B"* && "$texts" == *"Go"* && "$texts" == *"idle"* ]] || fail "page text rows missed the page: $texts"
still_background "page text" "$OUT"
AMBIG="$(obs page text --port "$PORT" --match cu- 2>/dev/null || true)"
[[ "$(jf "$AMBIG" ok)" == "False" && "$(jf "$AMBIG" error.code)" == "cdp_target_ambiguous" ]] || fail "page --match ambiguity: $AMBIG"
still_background "page text (ambiguous match)" "$AMBIG"
echo "STEP 4 page text --match cu-actuate-B -> B; --match cu- -> typed cdp_target_ambiguous; rows=$(jf "$OUT" data.returned) [$texts]"

# 3. page find: by text (lifted to the button), by selector, by role; a
#    miss and an ambiguity are typed.
OUT="$(obs page find --port "$PORT" --target-id "$ID_B" --text Go)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.returned)" == "1" ]] || fail "page find --text Go: $OUT"
[[ "$(jf "$OUT" data.nodes.0.role)" == "button" ]] || fail "page find --text Go should lift to the button: $(jf "$OUT" data.nodes.0)"
NODE_GO="$(jf "$OUT" data.nodes.0.node)"
[[ -n "$(jf "$OUT" data.nodes.0.box)" ]] || fail "page find carries no box: $(jf "$OUT" data.nodes.0)"
still_background "page find --text" "$OUT"
OUT="$(obs page find --port "$PORT" --target-id "$ID_B" --selector '#q')"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.nodes.0.editable)" == "True" ]] || fail "page find --selector #q: $OUT"
NODE_Q="$(jf "$OUT" data.nodes.0.node)"
still_background "page find --selector" "$OUT"
OUT="$(obs page find --port "$PORT" --target-id "$ID_B" --role button)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.nodes.0.node)" == "$NODE_GO" ]] || fail "page find --role button: $OUT"
still_background "page find --role" "$OUT"
OUT="$(obs page find --port "$PORT" --target-id "$ID_B" --text nowhere-at-all)" || true
[[ "$(jf "$OUT" ok)" == "False" && "$(jf "$OUT" error.code)" == "cdp_node_not_found" ]] || fail "expected cdp_node_not_found: $OUT"
OUT="$(act page click --port "$PORT" --target-id "$ID_B" --selector 'p,button,input')" || true
[[ "$(jf "$OUT" ok)" == "False" && "$(jf "$OUT" error.code)" == "cdp_node_ambiguous" ]] || fail "expected cdp_node_ambiguous: $OUT"
[[ "$(jf "$OUT" error.detail.count)" == "4" ]] || fail "cdp_node_ambiguous should count 4: $OUT"
still_background "page click (ambiguous)" "$OUT"
echo "STEP 5 page find: --text Go -> button node=$NODE_GO (box), --selector #q -> node=$NODE_Q editable, --role button -> same node; miss -> cdp_node_not_found; 4 hits -> cdp_node_ambiguous"

# 4. page fill --clear, verified by the value read-back.
OUT="$(act page fill --port "$PORT" --target-id "$ID_B" --selector '#q' --text hello --clear)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page fill: $OUT"
[[ "$(jf "$OUT" data.verification.after_value)" == "hello" ]] || fail "page fill read-back: $(jf "$OUT" data.verification)"
still_background "page fill" "$OUT"
echo "STEP 6 page fill --selector #q --text hello --clear -> verified (after_value=hello)"

TYPE_SECRET="typed-$PORT"
OUT="$(act page type --port "$PORT" --match cu-actuate-B "$TYPE_SECRET")"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page type: $OUT"
[[ "$OUT" != *"$TYPE_SECRET"* ]] || fail "page type leaked inserted text"
still_background "page type" "$OUT"
READ="$(obs page-js --port "$PORT" --target-id "$ID_B" --expression "document.getElementById('q').value")"
[[ "$(jf "$READ" data.value)" == "hello$TYPE_SECRET" ]] || fail "page type value read-back: $READ"
echo "STEP 6b page type at existing focus -> same-focus/value-growth verified; plaintext redacted"

# 5. page click by text: the onclick handler rewrites #out; verified by the
#    document read-back and independently through page-js.
OUT="$(act page click --port "$PORT" --target-id "$ID_B" --text Go)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.performed)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page click: $OUT"
still_background "page click" "$OUT"
READ="$(obs page-js --port "$PORT" --target-id "$ID_B" --expression "document.getElementById('out').textContent")"
[[ "$(jf "$READ" data.value)" == "clicked:hello$TYPE_SECRET" ]] || fail "the click did not run the page's onclick: $READ"
echo "STEP 7 page click --text Go -> performed+verified (changed: $(jf "$OUT" data.verification.changed)); page-js confirms the typed value"

# 6. Pointer verbs use the live boxes, never guessed fixture pixels.
BOX_GO="$(obs page find --port "$PORT" --target-id "$ID_B" --selector '#go')"
XY_GO="$(jf "$BOX_GO" data.nodes.0.box | python3 -c 'import json,sys; b=json.load(sys.stdin); print("{} {}".format(b["x"]+b["width"]/2,b["y"]+b["height"]/2))')"
OUT="$(act page click --port "$PORT" --match cu-actuate-B --x "${XY_GO%% *}" --y "${XY_GO##* }")"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page point click: $OUT"
[[ "$(jf "$OUT" data.verification.method)" == "trusted-mousedown-mouseup-readback" ]] || fail "page point click verification: $OUT"
still_background "page point click" "$OUT"
OUT="$(act page hover --port "$PORT" --target-id "$ID_B" --x "${XY_GO%% *}" --y "${XY_GO##* }")"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page hover: $OUT"
still_background "page hover" "$OUT"
BOX_SCROLL="$(obs page find --port "$PORT" --target-id "$ID_B" --selector '#scroll')"
XY_SCROLL="$(jf "$BOX_SCROLL" data.nodes.0.box | python3 -c 'import json,sys; b=json.load(sys.stdin); print("{} {}".format(b["x"]+b["width"]/2,b["y"]+b["height"]/2))')"
OUT="$(act page scroll --port "$PORT" --target-id "$ID_B" --x "${XY_SCROLL%% *}" --y "${XY_SCROLL##* }" --dy 120)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page scroll: $OUT"
[[ "$(jf "$OUT" data.verification.changed)" == *"top"* ]] || fail "page scroll did not read top back: $OUT"
still_background "page scroll" "$OUT"
echo "STEP 8 page point click + hover -> trusted target verified; page scroll -> container top changed; A still active"

# 7. File input is exact and path-redacted in the public reply/receipt.
UPLOAD="$UD/upload.txt"
printf 'hello' >"$UPLOAD"
OUT="$(act page files --port "$PORT" --target-id "$ID_B" --selector '#upload' "$UPLOAD")"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page files: $OUT"
[[ "$(jf "$OUT" data.verification.observed.0.name)" == "upload.txt" && "$(jf "$OUT" data.verification.observed.0.size)" == "5" ]] || fail "page files read-back: $OUT"
[[ "$OUT" != *"$UPLOAD"* ]] || fail "page files leaked the local path"
still_background "page files" "$OUT"
echo "STEP 9 page files --selector #upload -> verified basename=upload.txt size=5; no picker/path leak"

# 8. A held left-button sequence crosses two exact rendered elements and the
#    fixture confirms that trusted down/up handlers observed the gesture.
BOX_FROM="$(obs page find --port "$PORT" --target-id "$ID_B" --selector '#drag-from')"
BOX_TO="$(obs page find --port "$PORT" --target-id "$ID_B" --selector '#drag-to')"
XY_FROM="$(jf "$BOX_FROM" data.nodes.0.box | python3 -c 'import json,sys; b=json.load(sys.stdin); print("{} {}".format(b["x"]+b["width"]/2,b["y"]+b["height"]/2))')"
XY_TO="$(jf "$BOX_TO" data.nodes.0.box | python3 -c 'import json,sys; b=json.load(sys.stdin); print("{} {}".format(b["x"]+b["width"]/2,b["y"]+b["height"]/2))')"
OUT="$(act page drag --port "$PORT" --target-id "$ID_B" --x1 "${XY_FROM%% *}" --y1 "${XY_FROM##* }" --x2 "${XY_TO%% *}" --y2 "${XY_TO##* }")"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.performed)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page drag: $OUT"
[[ "$(jf "$OUT" data.release_attempted)" == "True" ]] || fail "page drag did not release: $OUT"
still_background "page drag" "$OUT"
READ="$(obs page-js --port "$PORT" --target-id "$ID_B" --expression "document.body.dataset.drag")"
[[ "$(jf "$READ" data.value)" == "done" ]] || fail "page drag business read-back: $READ"
echo "STEP 10 page drag #drag-from -> #drag-to: trusted held sequence + business result verified"

# 9. Arm a real prompt asynchronously, then handle it through a fresh CDP
#    session. The supplied response is business-readable but absent from the
#    public reply and persistent receipt.
DIALOG_SECRET="dialog-answer-$PORT"
OUT="$(obs page-js --port "$PORT" --target-id "$ID_B" --expression "setTimeout(()=>{document.body.dataset.dialog=prompt(String.fromCharCode(81,117,101,115,116,105,111,110),'')||'dismissed'},100);'armed'")"
[[ "$(jf "$OUT" data.value)" == "armed" ]] || fail "page dialog arm: $OUT"
OUT="$(act page dialog --port "$PORT" --target-id "$ID_B" --text "$DIALOG_SECRET" --wait-ms 3000)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.performed)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page dialog: $OUT"
[[ "$OUT" != *"$DIALOG_SECRET"* ]] || fail "page dialog leaked prompt response"
still_background "page dialog" "$OUT"
READ="$(obs page-js --port "$PORT" --target-id "$ID_B" --expression "document.body.dataset.dialog")"
[[ "$(jf "$READ" data.value)" == "$DIALOG_SECRET" ]] || fail "page dialog business read-back: $READ"
echo "STEP 11 page dialog prompt -> closed event verified; response redacted; business result exact"

# 10. page fill by node id, appended, then --submit runs the form's onsubmit.
OUT="$(act page fill --port "$PORT" --target-id "$ID_B" --node "$NODE_Q" --text ' world' --submit)"
[[ "$(jf "$OUT" ok)" == "True" && "$(jf "$OUT" data.verified)" == "True" ]] || fail "page fill --node --submit: $OUT"
[[ "$(jf "$OUT" data.verification.after_value)" == "hello$TYPE_SECRET world" ]] || fail "page fill append read-back: $(jf "$OUT" data.verification)"
[[ "$(jf "$OUT" data.submitted.dispatched)" == "True" ]] || fail "page fill --submit not dispatched: $OUT"
still_background "page fill --submit" "$OUT"
READ="$(obs page-js --port "$PORT" --target-id "$ID_B" --expression "document.getElementById('out').textContent")"
[[ "$(jf "$READ" data.value)" == "submitted:hello$TYPE_SECRET world" ]] || fail "Enter did not submit the form: $READ"
echo "STEP 12 page fill --node $NODE_Q --text ' world' --submit -> verified; page-js confirms form submission"

# 9. page screenshot: a PNG, or the typed refusal -- never an activation.
SHOT="$UD/b.png"
OUT="$(obs page screenshot --port "$PORT" --target-id "$ID_B" --out "$SHOT")" || true
if [[ "$(jf "$OUT" ok)" == "True" ]]; then
  [[ -s "$SHOT" ]] || fail "page screenshot wrote nothing: $OUT"
  magic="$(head -c 4 "$SHOT" | od -An -c | tr -d ' ')"
  [[ "$magic" == *"PNG"* ]] || fail "page screenshot is not a PNG: $magic"
  SHOT_NOTE="png bytes=$(jf "$OUT" data.bytes)"
else
  [[ "$(jf "$OUT" error.code)" == "cdp_screenshot_unavailable" ]] || fail "page screenshot: $OUT"
  SHOT_NOTE="typed cdp_screenshot_unavailable (background tab not painted)"
fi
still_background "page screenshot" "$OUT"
echo "STEP 13 page screenshot --target-id B -> $SHOT_NOTE; A still active"

# 10. Receipts: every actuation was reserved and completed.
OUT="$(obs receipts --max 40)"
[[ "$(jf "$OUT" ok)" == "True" ]] || fail "receipts: $OUT"
verbs="$(jf "$OUT" data.receipts | python3 -c '
import json,sys
lines = json.load(sys.stdin)
print(" ".join(sorted({l["verb"] for l in lines if l.get("phase") == "completed"})))')"
for verb in page-nav page-fill page-type page-click page-hover page-scroll page-drag page-dialog page-files; do
  [[ "$verbs" == *"$verb"* ]] || fail "receipts lack a completed $verb line: $verbs"
done
echo "STEP 14 receipts: completed [$verbs]"

echo "PASS: cu-cdp-actuate-smoke ($BROWSER_NAME headless, port $PORT, every CDP verb on the background tab; $CHECKS active-target + front-window checks; front window before/after: [$FRONT0])"
