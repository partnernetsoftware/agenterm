#!/usr/bin/env bash
# cu-brave-live-cdp-smoke — proof that agenterm-cu reads AND acts on a
# BACKGROUND tab of the REAL running Chromium-family browser (Brave Origin /
# Brave Browser / Google Chrome) over CDP, without changing which tab is
# selected in its window or which window is in front. Needs the real
# instance started with --remote-debugging-port (default 9222; a port that
# does not answer is a typed SKIP) and the profile named by
# $AGENTERM_CU_SMOKE_PROFILE (required; this script refuses to guess).
# It opens one throwaway `data:` tab in that profile, makes it a background
# tab again, drives every CDP verb on it, closes it while it is NOT the
# selected tab (a11y select-first / restore path), and proves the other
# tabs, the selected tab and the front window are as they were. The
# browser is never quit or restarted.
#
# Evidence (one dated line per run; PASS / FAIL(reason) / SKIP(reason);
# never a profile name or a host path):
#   2026-09-03  PASS  macOS, Brave Origin, multi-profile running instance,
#               port 9222: browser open --url into a profile window
#               (created:false, tab_index 6 / tab_title = the new tab, strip
#               6 -> 7), tab select back to the previous tab (index 1; the
#               smoke tab a background tab), page targets --browser-profile
#               (target joined to the window, profile_match title), page
#               text (4 rows, backend cdp), page find (--text lifted to the
#               button / --selector #q editable), page fill --clear, page
#               click, page-js read-back "clicked:hello", page screenshot
#               on the occluded background tab (a real 31 KB PNG, no
#               --activate), page nav (final_title read back); after every
#               verb the window's selected index [1] and focused-window
#               were unchanged; tab close --exact on the BACKGROUND tab
#               selected it first (1 poll, button found), pressed its close
#               button and reported selection_restored:true; a second smoke
#               tab closed with --port via cdp-close-target with the
#               selection unchanged; tab count 7 -> 6 (the start), titles
#               identical, front window identical to the baseline.
#
# What it proves, in order:
#   1. `browser profiles` lists the profile with a window; `tab list` /
#      `focused-window` snapshot the strip and the front window.
#   2. `browser open --profile P --url data:...` opens the page in that
#      window (created:false) and names the new tab (tab_index, tab_title).
#   3. `tab select --index` puts the previous tab back: the smoke tab is a
#      BACKGROUND tab from here on; front window re-read as the baseline.
#   4. `page targets --port --browser-profile P` lists the smoke tab joined
#      to the window (profile_match title) -> its target id.
#   5. `page text --target-id` returns the page's words (backend cdp).
#   6. `page find` by --text and --selector locate the button / field.
#   7. `page fill --selector '#q' --text hello --clear` verified by value.
#   8. `page click --text Go` verified; `page-js` reads back clicked:hello.
#   9. `page screenshot` of the occluded background tab: a PNG, or the typed
#      cdp_screenshot_unavailable -- recorded either way, never --activate.
#  10. `page nav --url data:...` loads a second title in the same tab.
#  After EVERY verb 4-10: the window's selected tab index is unchanged and
#  `focused-window` reports the same front window as the baseline.
#  11. `tab close --title T --exact --expect gone` on the BACKGROUND tab:
#      select_first performed, verified, selection_restored:true.
#  12. A second smoke tab closed with --port: via cdp-close-target, verified,
#      selection unchanged.
#  13. Tab count and titles equal the start; selected index and front
#      window equal the baseline.
#
# Env:  AGENTERM_CU_SMOKE_PROFILE   profile display name (required)
#       AGENTERM_CU_SMOKE_PORT      CDP port (default 9222)
#       AGENTERM_CU_SMOKE_APP       --app substring (optional; default: the
#                                   one running catalog browser)
#       AGENTERM_CU                 binary (else PATH, target/debug, abi-dev)
# This script never builds (other lanes may own Cargo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

skip() { echo "SKIP: cu-brave-live-cdp-smoke: $*" >&2; exit 0; }
fail() { echo "FAIL: cu-brave-live-cdp-smoke: $*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || skip "macOS only (browser open uses open -na)"
command -v python3 >/dev/null 2>&1 || skip "python3 not found"
command -v curl >/dev/null 2>&1 || skip "curl not found"

PROFILE="${AGENTERM_CU_SMOKE_PROFILE:-}"
[[ -n "$PROFILE" ]] || fail "set AGENTERM_CU_SMOKE_PROFILE to a profile name from \`agenterm-cu --target current --grant observe browser profiles\`; this script refuses to guess"
PORT="${AGENTERM_CU_SMOKE_PORT:-9222}"
curl -sf -m 3 "http://127.0.0.1:$PORT/json/version" >/dev/null 2>&1 \
  || skip "no CDP listener on 127.0.0.1:$PORT (start the browser with --remote-debugging-port=$PORT)"

CU="${AGENTERM_CU:-}"
if [[ -z "$CU" ]]; then
  if command -v agenterm-cu >/dev/null 2>&1; then CU="$(command -v agenterm-cu)"
  elif [[ -x "$ROOT/target/debug/agenterm-cu" ]]; then CU="$ROOT/target/debug/agenterm-cu"
  elif [[ -x "$ROOT/target/abi-dev/agenterm-cu" ]]; then CU="$ROOT/target/abi-dev/agenterm-cu"
  else skip "no agenterm-cu binary (set AGENTERM_CU or build the crate)"; fi
fi

APP_ARGS=()
[[ -n "${AGENTERM_CU_SMOKE_APP:-}" ]] && APP_ARGS=(--app "$AGENTERM_CU_SMOKE_APP")

# Audit + receipts + the screenshot go to a scratch dir removed on exit.
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/cu-brave-live-cdp-smoke.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
export AGENTERM_CU_AUDIT_PATH="$SCRATCH/audit.jsonl"

obs() { "$CU" --target current --grant observe "$@"; }
act() { "$CU" --target current --grant actuate "$@"; }

# py "<json>" "<python expression over r>"  -> prints the expression value.
py() {
  python3 - "$1" "$2" <<'PY'
import json, sys
r = json.loads(sys.argv[1])
v = eval(sys.argv[2], {"r": r, "json": json})
if isinstance(v, bool): print("true" if v else "false")
elif v is None: print("null")
elif isinstance(v, (list, dict)): print(json.dumps(v, ensure_ascii=False))
else: print(v)
PY
}
ok_or_fail() { # ok_or_fail "<json>" "<step>"
  [[ "$(py "$1" 'r["ok"]')" == "true" ]] || fail "$2: $(py "$1" 'json.dumps(r.get("error"), ensure_ascii=False)')"
}
# The strip of window $1: "selected-index|count|sorted titles|selected of TITLE".
strip() {
  local out; out="$(obs tab list --window "$1")"; ok_or_fail "$out" "tab list ($2)"
  py "$out" '[r["data"]["selected"], r["data"]["returned"], sorted(t["title"] for t in r["data"]["tabs"]), [t["selected"] for t in r["data"]["tabs"] if t["title"] == "'"$3"'"]]'
}
front() { # the focused-window handle (or null with the frontmost app)
  local out; out="$(obs focused-window)"; ok_or_fail "$out" "focused-window ($1)"
  py "$out" '(r["data"]["window"] or {}).get("handle"), (r["data"]["focused_app"] or {}).get("pid")'
}
# After every CDP verb: the selected tab index and the front window are
# unchanged, and the smoke tab is still a background tab.
unchanged() { # unchanged "<step>"
  local out sel fr
  out="$(obs tab list --window "$H")"; ok_or_fail "$out" "tab list after $1"
  sel="$(py "$out" 'r["data"]["selected"]')"
  [[ "$sel" == "$BASE_SEL" ]] || fail "$1 changed the selected tab: $BASE_SEL -> $sel"
  [[ "$(py "$out" '[t["selected"] for t in r["data"]["tabs"] if t["index"] == '"$NEW_INDEX"']')" == "[false]" ]] || fail "$1: the smoke tab is no longer a background tab"
  fr="$(front "after $1")"
  [[ "$fr" == "$FOCUS_BASE" ]] || fail "$1 changed the front window: $FOCUS_BASE -> $fr"
  echo "   unchanged after $1: selected=$sel front=$fr"
}

echo "== 1. browser profiles + baseline =="
OUT="$(obs browser profiles ${APP_ARGS[@]+"${APP_ARGS[@]}"})"; ok_or_fail "$OUT" "browser profiles"
APP="$(py "$OUT" 'r["data"]["app"]')"
ROW="$(py "$OUT" '[p for p in r["data"]["profiles"] if p["name"] == "'"$PROFILE"'"]')"
[[ "$ROW" != "[]" ]] || fail "profile named in AGENTERM_CU_SMOKE_PROFILE is not in Local State of $APP"
N_BEFORE="$(py "$ROW" 'len(r[0]["windows"])')"
(( N_BEFORE >= 1 )) || fail "the profile has no window; this smoke needs a window with a selected tab to come back to (open one by hand first)"
H="$(py "$ROW" 'r[0]["windows"][0]')"
OUT="$(obs tab list --window "$H")"; ok_or_fail "$OUT" "tab list before"
TABS_BEFORE="$(py "$OUT" 'r["data"]["returned"]')"
TITLES_BEFORE="$(py "$OUT" 'sorted(t["title"] for t in r["data"]["tabs"])')"
SEL_BEFORE="$(py "$OUT" 'r["data"]["selected"][0] if len(r["data"]["selected"]) == 1 else "none"')"
[[ "$SEL_BEFORE" != "none" ]] || fail "window $H has no single selected tab"
FOCUS0="$(front start)"
echo "app=$APP window=$H tabs-before=$TABS_BEFORE selected-before=$SEL_BEFORE front-start=$FOCUS0 port=$PORT"

TS="$(date +%s)"
TITLE="cu-real-$TS"
TITLE_NAV="cu-real-$TS-nav"
TITLE_B="cu-real-$TS-b"
url_for() { # url_for TITLE BODY -> a data: URL, percent-encoded
  python3 -c 'import sys, urllib.parse; print("data:text/html," + urllib.parse.quote("<title>%s</title>%s" % (sys.argv[1], sys.argv[2]), safe=""))' "$1" "$2"
}
# The handler rewrites #out so the click is verifiable through page-js; JS
# single quotes inside the double-quoted attribute keep it one attribute.
BODY="<h1>cu real smoke</h1><input id=\"q\" placeholder=\"type here\"><button id=\"go\" onclick=\"document.getElementById('out').textContent='clicked:'+document.getElementById('q').value\">Go</button><p id=\"out\">idle</p>"
URL="$(url_for "$TITLE" "$BODY")"
URL_NAV="$(url_for "$TITLE_NAV" '<h1>navigated</h1>')"
URL_B="$(url_for "$TITLE_B" '<h1>second</h1>')"

echo "== 2. browser open --url into the profile window (new tab named) =="
OUT="$(act browser open --profile "$PROFILE" --url "$URL" ${APP_ARGS[@]+"${APP_ARGS[@]}"})"; ok_or_fail "$OUT" "browser open"
[[ "$(py "$OUT" 'r["data"]["handle"]')" == "$H" ]] || fail "browser open used another window ($(py "$OUT" 'r["data"]["handle"]') vs $H); this smoke needs the URL to land in the existing window"
[[ "$(py "$OUT" 'r["data"]["created"]')" == "false" ]] || fail "browser open created a window instead of a tab"
NEW_INDEX="$(py "$OUT" 'r["data"]["tab_index"]')"
NEW_TITLE="$(py "$OUT" 'r["data"]["tab_title"]')"
[[ "$NEW_INDEX" != "null" ]] || fail "browser open did not name the new tab (tab_index null): $(py "$OUT" 'r["data"]["tabs"]')"
[[ "$NEW_TITLE" == "$TITLE" ]] || fail "browser open named tab $NEW_INDEX as $NEW_TITLE, expected $TITLE"
echo "created=false tab_index=$NEW_INDEX tab_title=$NEW_TITLE tabs=$(py "$OUT" 'r["data"]["tabs"]')"
OUT="$(obs wait --timeout-ms 3000 --window-title-contains "$TITLE")"; ok_or_fail "$OUT" "wait window title"

echo "== 3. tab select back: the smoke tab becomes a BACKGROUND tab =="
if (( SEL_BEFORE < NEW_INDEX )); then BASE_SEL_IDX=$SEL_BEFORE; else BASE_SEL_IDX=$((SEL_BEFORE + 1)); fi
OUT="$(act tab select --window "$H" --index "$BASE_SEL_IDX")"; ok_or_fail "$OUT" "tab select --index $BASE_SEL_IDX"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "tab select not verified"
OUT="$(obs tab list --window "$H")"; ok_or_fail "$OUT" "tab list after select"
BASE_SEL="$(py "$OUT" 'r["data"]["selected"]')"
[[ "$BASE_SEL" == "[$BASE_SEL_IDX]" ]] || fail "selected after tab select is $BASE_SEL, expected [$BASE_SEL_IDX]"
[[ "$(py "$OUT" '[t["selected"] for t in r["data"]["tabs"] if t["index"] == '"$NEW_INDEX"']')" == "[false]" ]] || fail "the smoke tab is still selected"
FOCUS_BASE="$(front baseline)"
echo "selected=$BASE_SEL smoke-tab-index=$NEW_INDEX (background) front-baseline=$FOCUS_BASE"

echo "== 4. page targets --browser-profile =="
OUT="$(obs page targets --port "$PORT" --browser-profile "$PROFILE")"; ok_or_fail "$OUT" "page targets"
TID="$(py "$OUT" '[t["id"] for t in r["data"]["targets"] if t["title"] == "'"$TITLE"'" and t["window"] == '"$H"']')"
[[ "$TID" != "[]" ]] || fail "page targets --browser-profile does not list $TITLE on window $H: $(py "$OUT" '[(t["title"], t["window"]) for t in r["data"]["targets"]]')"
TID="$(py "$TID" 'r[0]')"
echo "target=$TID profile_match=$(py "$OUT" 'r["data"]["profile_match"]') returned=$(py "$OUT" 'r["data"]["returned"]')"
unchanged "page targets"

echo "== 5. page text --target-id =="
OUT="$(obs page text --port "$PORT" --target-id "$TID")"; ok_or_fail "$OUT" "page text"
[[ "$(py "$OUT" 'r["data"]["backend"]')" == "cdp" ]] || fail "page text backend is not cdp"
[[ "$(py "$OUT" 'any("cu real smoke" in (row.get("text") or "") for row in r["data"]["rows"])')" == "true" ]] || fail "page text has no 'cu real smoke' row"
[[ "$(py "$OUT" 'r["data"]["focus_changed"]')" == "false" ]] || fail "page text reports focus_changed"
echo "rows=$(py "$OUT" 'r["data"]["returned"]')"
unchanged "page text"

echo "== 6. page find --text / --selector =="
OUT="$(obs page find --port "$PORT" --target-id "$TID" --text Go)"; ok_or_fail "$OUT" "page find --text"
BTN="$(py "$OUT" 'r["data"]["nodes"][0]["node"]')"
[[ "$(py "$OUT" 'r["data"]["nodes"][0]["tag"]')" == "button" ]] || fail "page find --text Go did not lift to the button"
OUT="$(obs page find --port "$PORT" --target-id "$TID" --selector '#q')"; ok_or_fail "$OUT" "page find --selector"
[[ "$(py "$OUT" 'r["data"]["nodes"][0]["editable"]')" == "true" ]] || fail "#q is not editable"
echo "button-node=$BTN field-node=$(py "$OUT" 'r["data"]["nodes"][0]["node"]')"
unchanged "page find"

echo "== 7. page fill --clear =="
OUT="$(act page fill --port "$PORT" --target-id "$TID" --selector '#q' --text hello --clear)"; ok_or_fail "$OUT" "page fill"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "page fill not verified: $(py "$OUT" 'r["data"].get("verification")')"
unchanged "page fill"

echo "== 8. page click + page-js read-back =="
OUT="$(act page click --port "$PORT" --target-id "$TID" --text Go)"; ok_or_fail "$OUT" "page click"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "page click not verified: $(py "$OUT" 'r["data"].get("verification")')"
OUT="$(obs page-js --port "$PORT" --target-id "$TID" --expression "document.getElementById('out').textContent")"; ok_or_fail "$OUT" "page-js"
READBACK="$(py "$OUT" 'r["data"]["value"]')"
[[ "$READBACK" == "clicked:hello" ]] || fail "page-js read back $READBACK, expected clicked:hello"
echo "readback=$READBACK"
unchanged "page click"

echo "== 9. page screenshot of the occluded background tab =="
SHOT="$SCRATCH/shot.png"
set +e
OUT="$(obs page screenshot --port "$PORT" --target-id "$TID" --out "$SHOT")"
set -e
if [[ "$(py "$OUT" 'r["ok"]')" == "true" ]]; then
  [[ -s "$SHOT" ]] || fail "page screenshot ok but no file"
  [[ "$(head -c 8 "$SHOT" | xxd -p)" == "89504e470d0a1a0a" ]] || fail "page screenshot is not a PNG"
  [[ "$(py "$OUT" 'r["data"]["focus_changed"]')" == "false" ]] || fail "page screenshot reports focus_changed"
  SHOT_RESULT="png $(stat -f %z "$SHOT") bytes"
else
  CODE="$(py "$OUT" 'r["error"]["code"]')"
  [[ "$CODE" == "cdp_screenshot_unavailable" ]] || fail "page screenshot failed with $CODE"
  SHOT_RESULT="typed $CODE (background tab; --activate never used)"
fi
echo "screenshot: $SHOT_RESULT"
unchanged "page screenshot"

echo "== 10. page nav =="
OUT="$(act page nav --port "$PORT" --target-id "$TID" --url "$URL_NAV" --wait-ms 8000)"; ok_or_fail "$OUT" "page nav"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "page nav not verified"
[[ "$(py "$OUT" 'r["data"]["final_title"]')" == "$TITLE_NAV" ]] || fail "page nav final_title is $(py "$OUT" 'r["data"]["final_title"]')"
OUT="$(obs wait --timeout-ms 3000 --window "$H" --node-name-contains "$TITLE_NAV")"; ok_or_fail "$OUT" "wait strip title"
unchanged "page nav"

echo "== 11. tab close --exact on the BACKGROUND tab (select-first + restore) =="
OUT="$(act tab close --window "$H" --title "$TITLE_NAV" --exact --expect gone)"; ok_or_fail "$OUT" "tab close (a11y)"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "tab close not verified"
[[ "$(py "$OUT" 'r["data"]["via"]')" == "tab-close" ]] || fail "tab close via $(py "$OUT" 'r["data"]["via"]'), expected tab-close"
[[ "$(py "$OUT" 'r["data"]["select_first"]["performed"]')" == "true" ]] || fail "tab close did not select the background tab first: $(py "$OUT" 'r["data"]["select_first"]')"
[[ "$(py "$OUT" 'r["data"]["selection_restored"]')" == "true" ]] || fail "tab close did not restore the selection: $(py "$OUT" 'r["data"]["restore"]')"
echo "verified=true select_first=$(py "$OUT" 'r["data"]["select_first"]') selection_restored=true polls=$(py "$OUT" 'r["data"]["verification"]["polls"]')"
OUT="$(obs tab list --window "$H")"; ok_or_fail "$OUT" "tab list after a11y close"
[[ "$(py "$OUT" 'r["data"]["selected"]')" == "[$SEL_BEFORE]" ]] || fail "selected after a11y close is $(py "$OUT" 'r["data"]["selected"]'), expected [$SEL_BEFORE]"

echo "== 12. a second smoke tab closed over CDP (--port) =="
OUT="$(act browser open --profile "$PROFILE" --url "$URL_B" ${APP_ARGS[@]+"${APP_ARGS[@]}"})"; ok_or_fail "$OUT" "browser open (b)"
NEW_B="$(py "$OUT" 'r["data"]["tab_index"]')"
[[ "$(py "$OUT" 'r["data"]["tab_title"]')" == "$TITLE_B" ]] || fail "browser open (b) named $(py "$OUT" 'r["data"]["tab_title"]')"
OUT="$(obs wait --timeout-ms 3000 --window-title-contains "$TITLE_B")"; ok_or_fail "$OUT" "wait window title (b)"
if (( SEL_BEFORE < NEW_B )); then SEL_B=$SEL_BEFORE; else SEL_B=$((SEL_BEFORE + 1)); fi
OUT="$(act tab select --window "$H" --index "$SEL_B")"; ok_or_fail "$OUT" "tab select back (b)"
OUT="$(act tab close --window "$H" --index "$NEW_B" --expect gone --port "$PORT")"; ok_or_fail "$OUT" "tab close (cdp)"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "tab close (cdp) not verified"
[[ "$(py "$OUT" 'r["data"]["via"]')" == "cdp-close-target" ]] || fail "tab close --port went via $(py "$OUT" 'r["data"]["via"]') ($(py "$OUT" 'r["data"].get("cdp_fallback")'))"
[[ "$(py "$OUT" 'r["data"]["selection_restored"]')" == "true" ]] || fail "selection moved across the CDP close"
echo "verified=true via=cdp-close-target target=$(py "$OUT" 'r["data"]["cdp"]["target"]["id"]') selection_restored=true"

echo "== 13. everything back to the start =="
OUT="$(obs tab list --window "$H")"; ok_or_fail "$OUT" "tab list end"
TABS_AFTER="$(py "$OUT" 'r["data"]["returned"]')"
TITLES_AFTER="$(py "$OUT" 'sorted(t["title"] for t in r["data"]["tabs"])')"
SEL_AFTER="$(py "$OUT" 'r["data"]["selected"]')"
(( TABS_AFTER == TABS_BEFORE )) || fail "tab count after close is $TABS_AFTER, expected $TABS_BEFORE"
[[ "$TITLES_AFTER" == "$TITLES_BEFORE" ]] || fail "tab titles changed: $TITLES_BEFORE -> $TITLES_AFTER"
[[ "$SEL_AFTER" == "[$SEL_BEFORE]" ]] || fail "selected tab at the end is $SEL_AFTER, expected [$SEL_BEFORE]"
FOCUS_END="$(front end)"
[[ "$FOCUS_END" == "$FOCUS_BASE" ]] || fail "front window changed: $FOCUS_BASE -> $FOCUS_END"
echo "tabs $TABS_BEFORE -> $TABS_AFTER selected $SEL_BEFORE -> ${SEL_AFTER} front $FOCUS_BASE == $FOCUS_END (start $FOCUS0)"

echo "== receipts =="
obs receipts --window "$H" --max 12 | python3 -c 'import json,sys; r=json.load(sys.stdin); [print(" ", l["verb"], l["phase"], l.get("verified")) for l in r["data"]["receipts"]]'

echo "PASS: cu-brave-live-cdp-smoke: window=$H tabs $TABS_BEFORE -> $((TABS_BEFORE + 1)) -> $TABS_AFTER, selected $BASE_SEL unchanged across every CDP verb, front $FOCUS_BASE unchanged, screenshot: $SHOT_RESULT"
