#!/usr/bin/env bash
# cu-brave-live-smoke — proof on the REAL running Chromium-family browser
# (Brave Origin / Brave Browser / Google Chrome), not a throwaway headless
# one. It opens one throwaway `data:` tab in the profile named by
# $AGENTERM_CU_SMOKE_PROFILE (required; this script refuses to guess a
# profile), drives it through the a11y verbs only -- no screenshot, no CDP
# port, no keyboard shortcut -- closes that tab again, and proves the
# profile's other tabs were untouched. The browser is never quit or
# restarted; the front window is compared before / after the click.
#
# Evidence (one dated line per run; PASS / FAIL(reason) / SKIP(reason);
# never a profile name or a host path):
#   2026-09-03  PASS  macOS, Brave Origin, five-profile running instance:
#               browser open --url into a profile that already had a window
#               (created:false, title changed), windows --browser-profile,
#               tab list (new tab selected, count 6 -> 7), page text (h1
#               row), click --node on the page button verified by the tree
#               diff (the button relabels itself) with the front window
#               unchanged, tab close --exact --expect gone verified by the
#               strip read-back in one poll, tab count back to 6, other
#               titles identical. Note: `open` activates the profile's
#               window, so the front window differs from the run's start.
#   2026-09-03  PASS  same instance, second run after the tab-close /
#               focused-window fixes: tabs 6 -> 7 -> 6, front window
#               identical across the click and at the end (the window was
#               already the profile's focused one). The page text step now
#               pokes `unlock` and re-reads: a freshly opened tab's web-area
#               reached the AX tree only after the poke (the first run of
#               the day saw 25 chrome rows and no h1 until then).
#
# What it proves, in order:
#   1. `browser profiles` lists the profile and its window(s).
#   2. `browser open --profile P --url data:...` opens the URL in the running
#      instance and returns {handle, browser_profile, title, created}.
#   3. `windows --browser-profile P` finds that window.
#   4. `tab list --window H` shows the new tab selected; count is before + 1.
#   5. `page text --window H` returns the page's h1 text.
#   6. `click --node` on the page button succeeds, and `windows --focused`
#      reports the same front window before and after.
#   7. `tab close --window H --title T --exact --expect gone` is verified.
#   8. `tab list` count is back to before, and the other tab titles are the
#      same set as before.
#
# Env:  AGENTERM_CU_SMOKE_PROFILE   profile display name (required)
#       AGENTERM_CU_SMOKE_APP       --app substring (optional; default: the
#                                   one running catalog browser)
#       AGENTERM_CU                 binary (else PATH, target/debug, abi-dev)
# This script never builds (other lanes may own Cargo).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

skip() { echo "SKIP: cu-brave-live-smoke: $*" >&2; exit 0; }
fail() { echo "FAIL: cu-brave-live-smoke: $*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || skip "macOS only (browser open uses open -na)"
command -v python3 >/dev/null 2>&1 || skip "python3 not found"

PROFILE="${AGENTERM_CU_SMOKE_PROFILE:-}"
[[ -n "$PROFILE" ]] || fail "set AGENTERM_CU_SMOKE_PROFILE to a profile name from \`agenterm-cu --target current --grant observe browser profiles\`; this script refuses to guess"

CU="${AGENTERM_CU:-}"
if [[ -z "$CU" ]]; then
  if command -v agenterm-cu >/dev/null 2>&1; then CU="$(command -v agenterm-cu)"
  elif [[ -x "$ROOT/target/debug/agenterm-cu" ]]; then CU="$ROOT/target/debug/agenterm-cu"
  elif [[ -x "$ROOT/target/abi-dev/agenterm-cu" ]]; then CU="$ROOT/target/abi-dev/agenterm-cu"
  else skip "no agenterm-cu binary (set AGENTERM_CU or build the crate)"; fi
fi

APP_ARGS=()
[[ -n "${AGENTERM_CU_SMOKE_APP:-}" ]] && APP_ARGS=(--app "$AGENTERM_CU_SMOKE_APP")

# Audit + receipts go to a scratch dir so the run leaves the user's audit
# log alone; the receipt file is printed at the end.
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/cu-brave-live-smoke.XXXXXX")"
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
elif isinstance(v, (list, dict)): print(json.dumps(v, ensure_ascii=False))
else: print(v)
PY
}
ok_or_fail() { # ok_or_fail "<json>" "<step>"
  [[ "$(py "$1" 'r["ok"]')" == "true" ]] || fail "$2: $(py "$1" 'json.dumps(r.get("error"), ensure_ascii=False)')"
}

echo "== 1. browser profiles =="
OUT="$(obs browser profiles ${APP_ARGS[@]+"${APP_ARGS[@]}"})"; ok_or_fail "$OUT" "browser profiles"
APP="$(py "$OUT" 'r["data"]["app"]')"
ROW="$(py "$OUT" '[p for p in r["data"]["profiles"] if p["name"] == "'"$PROFILE"'"]')"
[[ "$ROW" != "[]" ]] || fail "profile named in AGENTERM_CU_SMOKE_PROFILE is not in Local State of $APP"
BEFORE_HANDLES="$(py "$ROW" 'r[0]["windows"]')"
N_BEFORE="$(py "$ROW" 'len(r[0]["windows"])')"
echo "app=$APP profile-windows-before=$BEFORE_HANDLES"
(( N_BEFORE >= 1 )) || fail "the profile has no window; this smoke proves a URL into a profile that already has a window (open one by hand first)"
H0="$(py "$ROW" 'r[0]["windows"][0]')"

OUT="$(obs tab list --window "$H0")"; ok_or_fail "$OUT" "tab list before"
TABS_BEFORE="$(py "$OUT" 'r["data"]["returned"]')"
TITLES_BEFORE="$(py "$OUT" 'sorted(t["title"] for t in r["data"]["tabs"])')"
OUT="$(obs windows --focused)"; ok_or_fail "$OUT" "windows --focused"
FOCUS0="$(py "$OUT" '[w["handle"] for w in r["data"]["windows"]]')"
echo "window=$H0 tabs-before=$TABS_BEFORE focused-before=$FOCUS0"

TS="$(date +%s)"
TITLE="cu-live-$TS"
# The button relabels itself on press so the click is verifiable by the
# tree diff (a handler-less button changes nothing the tree can see).
URL="data:text/html,<title>$TITLE</title><h1>cu%20live%20smoke</h1><button%20onclick=\"this.textContent='Pressed'\">Press%20me</button>"

echo "== 2. browser open --profile --url =="
OUT="$(act browser open --profile "$PROFILE" --url "$URL" ${APP_ARGS[@]+"${APP_ARGS[@]}"})"; ok_or_fail "$OUT" "browser open"
H="$(py "$OUT" 'r["data"]["handle"]')"
CREATED="$(py "$OUT" 'r["data"]["created"]')"
[[ "$(py "$OUT" 'r["data"]["browser_profile"]')" == "$PROFILE" ]] || fail "browser open returned another profile"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "browser open not verified"
echo "handle=$H created=$CREATED title=$(py "$OUT" 'r["data"]["title"]') polls=$(py "$OUT" 'r["data"]["verification"]["polls"]')"
# The title follows the active tab; give the document a moment if the poll
# hit on the window before the title settled.
OUT="$(obs wait --timeout-ms 3000 --window-title-contains "$TITLE")"; ok_or_fail "$OUT" "wait window title"

echo "== 3. windows --browser-profile =="
OUT="$(obs windows --browser-profile "$PROFILE")"; ok_or_fail "$OUT" "windows --browser-profile"
[[ "$(py "$OUT" '"'"$H"'" in [str(w["handle"]) for w in r["data"]["windows"]]')" == "true" ]] || fail "windows --browser-profile does not list $H"
[[ "$(py "$OUT" 'all(w["browser_profile"] == "'"$PROFILE"'" for w in r["data"]["windows"])')" == "true" ]] || fail "windows --browser-profile returned another profile's window"
echo "matched=$(py "$OUT" 'r["data"]["matched"]') handles=$(py "$OUT" '[w["handle"] for w in r["data"]["windows"]]')"

echo "== 4. tab list: new tab selected, count + 1 =="
OUT="$(obs tab list --window "$H")"; ok_or_fail "$OUT" "tab list after open"
TABS_OPEN="$(py "$OUT" 'r["data"]["returned"]')"
SEL="$(py "$OUT" '[t["selected"] for t in r["data"]["tabs"] if t["title"] == "'"$TITLE"'"]')"
[[ "$SEL" == "[true]" ]] || fail "new tab $TITLE is not the single selected tab: $SEL"
if [[ "$H" == "$H0" ]]; then
  (( TABS_OPEN == TABS_BEFORE + 1 )) || fail "tab count after open is $TABS_OPEN, expected $((TABS_BEFORE + 1))"
fi
echo "tabs-after-open=$TABS_OPEN selected=$TITLE"

echo "== 5. page text =="
# A freshly opened tab's renderer publishes its web-area to the AX tree a
# moment after the title lands (and only once the engine has been asked
# for it): poke with `unlock` and re-read, bounded.
HAS_H1=false
for _ in 1 2 3 4 5 6 7 8; do
  OUT="$(obs page text --window "$H")"; ok_or_fail "$OUT" "page text"
  if [[ "$(py "$OUT" 'any("cu live smoke" in (row.get("text") or "") for row in r["data"]["rows"])')" == "true" ]]; then HAS_H1=true; break; fi
  obs unlock --window "$H" >/dev/null 2>&1 || true
  sleep 0.5
done
[[ "$HAS_H1" == true ]] || fail "page text has no 'cu live smoke' row after unlock re-reads (rows=$(py "$OUT" 'r["data"]["returned"]'))"
BTN="$(py "$OUT" '[row["id"] for row in r["data"]["rows"] if "button" in row["role"].lower() and "Press me" in ((row.get("text") or "") + (row.get("name") or ""))]')"
[[ "$BTN" != "[]" ]] || fail "page text has no 'Press me' button row"
NODE="$(py "$BTN" 'r[0]')"
echo "rows=$(py "$OUT" 'r["data"]["returned"]') button-node=$NODE"

echo "== 6. click --node with the front window unchanged =="
OUT="$(obs windows --focused)"; ok_or_fail "$OUT" "windows --focused (before click)"
FOCUS_A="$(py "$OUT" '[w["handle"] for w in r["data"]["windows"]]')"
OUT="$(act click --window "$H" --node "$NODE")"; ok_or_fail "$OUT" "click --node"
[[ "$(py "$OUT" 'r["data"].get("performed", True)')" == "true" ]] || fail "click not performed"
CLICK_VERIFIED="$(py "$OUT" 'r["data"].get("verified")')"
[[ "$CLICK_VERIFIED" == "true" ]] || fail "click performed but not verified: $(py "$OUT" 'json.dumps(r["data"].get("verification"), ensure_ascii=False)')"
OUT2="$(obs page text --window "$H")"; ok_or_fail "$OUT2" "page text after click"
[[ "$(py "$OUT2" 'any("Pressed" in ((row.get("text") or "") + (row.get("name") or "")) for row in r["data"]["rows"])')" == "true" ]] || fail "the button did not relabel itself after the click"
OUT="$(obs windows --focused)"; ok_or_fail "$OUT" "windows --focused (after click)"
FOCUS_B="$(py "$OUT" '[w["handle"] for w in r["data"]["windows"]]')"
[[ "$FOCUS_A" == "$FOCUS_B" ]] || fail "front window changed across the click: $FOCUS_A -> $FOCUS_B"
echo "click verified=$CLICK_VERIFIED (button relabeled) focused-before-click=$FOCUS_A focused-after-click=$FOCUS_B"

echo "== 7. tab close --exact --expect gone =="
OUT="$(act tab close --window "$H" --title "$TITLE" --exact --expect gone)"; ok_or_fail "$OUT" "tab close"
[[ "$(py "$OUT" 'r["data"]["verified"]')" == "true" ]] || fail "tab close not verified"
echo "verified=true polls=$(py "$OUT" 'r["data"]["verification"]["polls"]') window_present=$(py "$OUT" 'r["data"]["after"]["window_present"]')"

echo "== 8. other tabs untouched =="
OUT="$(obs tab list --window "$H0")"; ok_or_fail "$OUT" "tab list after close"
TABS_AFTER="$(py "$OUT" 'r["data"]["returned"]')"
TITLES_AFTER="$(py "$OUT" 'sorted(t["title"] for t in r["data"]["tabs"])')"
(( TABS_AFTER == TABS_BEFORE )) || fail "tab count after close is $TABS_AFTER, expected $TABS_BEFORE"
[[ "$TITLES_AFTER" == "$TITLES_BEFORE" ]] || fail "tab titles changed: $TITLES_BEFORE -> $TITLES_AFTER"
OUT="$(obs windows --focused)"; ok_or_fail "$OUT" "windows --focused (end)"
FOCUS_END="$(py "$OUT" '[w["handle"] for w in r["data"]["windows"]]')"
echo "tabs-after-close=$TABS_AFTER (before $TABS_BEFORE) focused-end=$FOCUS_END (start $FOCUS0)"

echo "== receipts =="
obs receipts --window "$H" --max 6 | python3 -c 'import json,sys; r=json.load(sys.stdin); [print(" ", l["verb"], l["phase"], l.get("verified")) for l in r["data"]["receipts"]]'

echo "PASS: cu-brave-live-smoke: window=$H created=$CREATED tabs $TABS_BEFORE -> $TABS_OPEN -> $TABS_AFTER, front window $FOCUS_A == $FOCUS_B across the click"
