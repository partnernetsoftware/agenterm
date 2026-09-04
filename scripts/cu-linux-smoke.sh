#!/usr/bin/env bash
# Public black-box smoke for agenterm-cu on Linux/X11 + AT-SPI2.
#
# SUPERSEDED as the live gate by scripts/qjs/cu-linux-smoke.qjs, registered
# as the `cu-linux-smoke` task:
#
#   agenterm cli script task run cu-linux-smoke --json
#
# That one owns its GTK fixture, asserts the readings rather than the exit
# codes, and emits evidence ids. This script stays as a dependency-free
# check anyone can run against an already-running desktop without the task
# runner; it does not replace the journey and does not emit evidence.
#
# The last section owns a throwaway Chromium-family browser (fresh
# --user-data-dir, --no-first-run, two data: URL tabs) and proves the
# browser verbs against it without a screenshot: `windows --app`, `unlock`
# (the AT-SPI org.a11y.Status poke), `tab list`, `tab select` with a
# read-back, `page text`, and over CDP `page targets --pid` / `page-js` against a
# *background* tab. Every missing prerequisite prints a TYPED
# `SKIP[<code>]` and makes the final line say the section was skipped -- a
# bare exit 0 there would be read as a pass by the parity ledger.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -z "${DISPLAY:-}" ]]; then
  echo "SKIP[no_display]: DISPLAY is not set; export DISPLAY=:1 or start Xvfb" >&2
  exit 0
fi

AUDIT_DIR="${AGENTERM_CU_AUDIT_PATH:-${TMPDIR:-/tmp}/agenterm-cu-smoke-$$}"
export AGENTERM_CU_AUDIT_PATH="${AUDIT_DIR}/audit.jsonl"
mkdir -p "$(dirname "$AGENTERM_CU_AUDIT_PATH")"

echo "Building agenterm-cu..."
# libagenterm requires panic=unwind; the workspace dev/release profiles abort.
cargo build -p agenterm-cu --bin agenterm-cu --profile abi-dev
CU="$ROOT/target/abi-dev/agenterm-cu"

json_field() {
  python3 - "$1" "$2" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
print(payload[sys.argv[2]])
PY
}

run_json() {
  "$CU" "$@"
}

echo "== capabilities =="
OUT="$(run_json --target current --grant observe capabilities)"
test "$(json_field "$OUT" ok)" = "True"

echo "== windows =="
OUT="$(run_json --target current --grant observe windows)"
test "$(json_field "$OUT" ok)" = "True"

echo "== at-spi tree =="
OUT="$(run_json --target current --grant observe tree)"
test "$(json_field "$OUT" ok)" = "True"
python3 - "$OUT" <<'PY'
import json, sys
data = json.loads(sys.argv[1])["data"]
assert data["degraded"] is False
assert data["backend"] == "at-spi2"
assert data["addressing"] == "accessibility-tree"
assert len(data["nodes"]) > 0
node = data["nodes"][0]
for key in ("id", "role", "name", "states", "bounds", "actions"):
    assert key in node
bounds = node["bounds"]
for key in ("x", "y", "width", "height"):
    assert key in bounds
PY

echo "== wait window-count =="
OUT="$(run_json --target current --grant observe wait --timeout-ms 2000 --window-count-gte 1)"
test "$(json_field "$OUT" ok)" = "True"

echo "== refused actuation without grant =="
OUT="$(run_json --target current --grant observe send-text smoke-refused)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "refused"
PY

echo "== audited degraded click =="
OUT="$(run_json --target current --grant actuate click --coords 1,1 --degraded)"
test "$(json_field "$OUT" ok)" = "True"
test -s "$AGENTERM_CU_AUDIT_PATH"

echo "== invalid at-spi node path fails typed =="
OUT="$(run_json --target current --grant actuate click --node /0/999999)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "a11y_node_not_found"
PY

echo "== name-addressed click requires --window =="
OUT="$(run_json --target current --grant actuate click --name Reload)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "invalid_input"
PY

echo "== name-addressed click missing node fails typed =="
HANDLE="$(python3 - "$("$CU" --target current --grant observe windows)" <<'PY'
import json, sys
payload = json.loads(sys.argv[1])
if payload.get("ok") and payload.get("data"):
    print(payload["data"][0]["handle"])
PY
)"
if [[ -n "${HANDLE:-}" ]]; then
  OUT="$(run_json --target current --grant actuate click --window "$HANDLE" --name agenterm-no-such-control)"
  test "$(json_field "$OUT" ok)" = "False"
  python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] in ("a11y_node_not_found", "unsupported")
PY
  OUT="$(run_json --target current --grant actuate send-text --window "$HANDLE" --name agenterm-no-such-control -- hello)"
  test "$(json_field "$OUT" ok)" = "False"
  python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] in ("a11y_node_not_found", "unsupported")
PY
  OUT="$(run_json --target current --grant actuate send-keys --window "$HANDLE" --name agenterm-no-such-control -- enter)"
  test "$(json_field "$OUT" ok)" = "False"
  python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] in ("a11y_node_not_found", "unsupported")
PY
  OUT="$(run_json --target current --grant actuate paste --window "$HANDLE" --name agenterm-no-such-control --text hello)"
  test "$(json_field "$OUT" ok)" = "False"
  python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] in ("a11y_node_not_found", "unsupported")
PY
fi

echo "== name-addressed send-text requires --window =="
OUT="$(run_json --target current --grant actuate send-text --name Reload -- hello)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "invalid_input"
PY

echo "== name-addressed send-keys requires --window =="
OUT="$(run_json --target current --grant actuate send-keys --name Reload -- enter)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "invalid_input"
PY

echo "== name-addressed paste requires --name =="
OUT="$(run_json --target current --grant actuate paste --window 1 --text hello)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "invalid_input"
PY

echo "== name-addressed paste requires --window =="
OUT="$(run_json --target current --grant actuate paste --name FixtureField --text hello)"
test "$(json_field "$OUT" ok)" = "False"
python3 - "$OUT" <<'PY'
import json, sys
err = json.loads(sys.argv[1])["error"]
assert err["code"] == "invalid_input"
PY

echo "== structured at-spi click when node exists =="
OUT="$(run_json --target current --grant observe tree)"
NODE_ID="$(python3 - "$OUT" <<'PY'
import json, sys

def showing(node):
    bounds = node.get("bounds") or {}
    return bounds.get("width", 0) > 0 and bounds.get("height", 0) > 0

def named_click(node):
    return any(str(action).lower() in ("click", "press") for action in node.get("actions", []))

def actionable_control(node):
    role = str(node.get("role") or "").lower()
    name = str(node.get("name") or "").lower()
    if name in ("minimize", "maximize", "close", "close this view"):
        return False
    return showing(node) and (
        named_click(node)
        or role in ("button", "push button", "toggle button", "link")
        or "button" in role
    )

nodes = json.loads(sys.argv[1])["data"]["nodes"]
for node in nodes:
    if named_click(node) and showing(node):
        print(node["id"])
        raise SystemExit
for node in nodes:
    if actionable_control(node):
        print(node["id"])
        break
PY
)"
if [[ -n "${NODE_ID:-}" ]]; then
  OUT="$(run_json --target current --grant actuate click --node "$NODE_ID")"
  test "$(json_field "$OUT" ok)" = "True"
  python3 - "$OUT" <<'PY'
import json, sys
data = json.loads(sys.argv[1])["data"]
assert data["addressing"] == "accessibility-tree"
assert data.get("addressing") != "degraded-coordinates"
PY
else
  echo "SKIP: no showing AT-SPI button/link or named click action in current desktop tree"
fi

# ---------------------------------------------------------------------------
# Browser section: a throwaway Chromium-family instance, two tabs, and the
# tab / page-text / CDP verbs read against it. No screenshot anywhere.
#
# Every prerequisite that is missing exits through `browser_skip`, which
# prints a TYPED skip code and makes the final line say the section was
# skipped. A bare `exit 0` here would be read as a pass by the evidence
# ledger, and "we could not run it" is not evidence that it works.
# ---------------------------------------------------------------------------
BROWSER_SECTION="skipped"
BROWSER_SKIP_CODE=""

browser_skip() {
  BROWSER_SECTION="skipped"
  BROWSER_SKIP_CODE="$1"
  echo "SKIP[$1]: browser section needs $2" >&2
  return 0
}

BROWSER_PID=""
BROWSER_DIR=""
browser_cleanup() {
  if [[ -n "${BROWSER_PID:-}" ]]; then
    kill "$BROWSER_PID" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$BROWSER_PID" 2>/dev/null || break
      sleep 0.3
    done
    kill -9 "$BROWSER_PID" 2>/dev/null || true
    wait "$BROWSER_PID" 2>/dev/null || true
  fi
  if [[ -n "${BROWSER_DIR:-}" && -d "$BROWSER_DIR" ]]; then
    rm -rf "$BROWSER_DIR"
  fi
}
trap browser_cleanup EXIT

BROWSER_BIN=""
for candidate in chromium chromium-browser google-chrome google-chrome-stable \
                 brave-browser brave-browser-stable microsoft-edge microsoft-edge-stable; do
  if command -v "$candidate" >/dev/null 2>&1; then
    BROWSER_BIN="$(command -v "$candidate")"
    break
  fi
done

if [[ -z "$BROWSER_BIN" ]]; then
  browser_skip no_chromium_browser \
    "a Chromium-family binary on PATH (chromium / chromium-browser / google-chrome / brave-browser / microsoft-edge)"
elif ! command -v python3 >/dev/null 2>&1; then
  browser_skip no_python3 "python3, which shapes every assertion in this script"
else
  echo "== browser: throwaway $(basename "$BROWSER_BIN") =="
  BROWSER_DIR="$(mktemp -d "${TMPDIR:-/tmp}/agenterm-cu-browser-XXXXXX")"
  CDP_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
  TAB_A="AGENTERM-TAB-A"
  TAB_B="AGENTERM-TAB-B"
  URL_A="data:text/html,<title>${TAB_A}</title><h1>alpha marker one</h1>"
  URL_B="data:text/html,<title>${TAB_B}</title><h1>bravo marker two</h1>"

  # No --force-renderer-accessibility on purpose: this section is also the
  # evidence that `unlock` (the org.a11y.Status toggle, the AT-SPI analogue
  # of macOS AXManualAccessibility) is what makes the tree appear.
  "$BROWSER_BIN" \
    --user-data-dir="$BROWSER_DIR" \
    --no-first-run \
    --no-default-browser-check \
    --disable-background-networking \
    --disable-component-update \
    --remote-debugging-port="$CDP_PORT" \
    "$URL_A" "$URL_B" >"$BROWSER_DIR/browser.log" 2>&1 &
  BROWSER_PID=$!

  BROWSER_APP=""
  for _ in $(seq 1 40); do
    if [[ -r "/proc/$BROWSER_PID/comm" ]]; then
      BROWSER_APP="$(tr -d '\n' <"/proc/$BROWSER_PID/comm")"
      [[ -n "$BROWSER_APP" ]] && break
    fi
    sleep 0.25
  done
  if [[ -z "$BROWSER_APP" ]]; then
    browser_skip browser_did_not_start \
      "the launched browser process to stay alive long enough to name itself (see browser.log in the temp profile dir)"
  else
    HANDLE=""
    for _ in $(seq 1 60); do
      OUT="$(run_json --target current --grant observe windows --app "$BROWSER_APP" || true)"
      HANDLE="$(python3 -c '
import json, sys
try:
    payload = json.loads(sys.argv[1])
except Exception:
    raise SystemExit
rows = payload.get("data") or []
if isinstance(rows, dict):
    rows = rows.get("windows") or []
for row in rows:
    if row.get("handle"):
        print(row["handle"])
        break
' "$OUT")"
      [[ -n "$HANDLE" ]] && break
      sleep 0.5
    done

    if [[ -z "$HANDLE" ]]; then
      browser_skip browser_window_not_in_inventory \
        "an X11 toplevel for the browser in 'windows --app $BROWSER_APP' (a Wayland-only session publishes no window inventory)"
    else
      echo "== browser: windows --app $BROWSER_APP lists handle $HANDLE =="

      echo "== browser: unlock (org.a11y.Status poke) =="
      OUT="$(run_json --target current --grant actuate unlock --window "$HANDLE" || true)"
      python3 -c '
import json, sys
payload = json.loads(sys.argv[1])
data = payload.get("data") or {}
if not data.get("poked"):
    raise SystemExit("FAIL: unlock reported poked=false: %s" % (data.get("reason") or "<no reason reported>"))
' "$OUT"

      echo "== browser: tab list =="
      TABS=""
      for _ in $(seq 1 20); do
        TABS="$(run_json --target current --grant observe tab list --window "$HANDLE" || true)"
        COUNT="$(python3 -c '
import json, sys
try:
    data = json.loads(sys.argv[1]).get("data") or {}
except Exception:
    print(0); raise SystemExit
print(len(data.get("tabs") or []))
' "$TABS")"
        [[ "${COUNT:-0}" -ge 2 ]] && break
        sleep 0.5
      done
      python3 -c '
import json, sys
data = json.loads(sys.argv[1])["data"]
want_a, want_b = sys.argv[2], sys.argv[3]
tabs = data.get("tabs") or []
titles = [t.get("title", "") for t in tabs]
assert any(want_a in title for title in titles), "tab %s missing from %s" % (want_a, titles)
assert any(want_b in title for title in titles), "tab %s missing from %s" % (want_b, titles)
selected = [t for t in tabs if t.get("selected") is True]
assert len(selected) == 1, "expected exactly one selected tab, got %s" % [t.get("title") for t in selected]
assert all(t.get("selected") in (True, False) for t in tabs), "a tab reports no readable selection state: %s" % tabs
print("tab list ok:", titles)
' "$TABS" "$TAB_A" "$TAB_B"

      echo "== browser: tab select --title $TAB_B, verified by read-back =="
      OUT="$(run_json --target current --grant actuate tab select --window "$HANDLE" --title "$TAB_B")"
      test "$(json_field "$OUT" ok)" = "True"
      TABS="$(run_json --target current --grant observe tab list --window "$HANDLE")"
      python3 -c '
import json, sys
tabs = json.loads(sys.argv[1])["data"]["tabs"]
want = sys.argv[2]
selected = [t for t in tabs if t.get("selected") is True]
assert len(selected) == 1, "expected one selected tab after select, got %s" % selected
assert want in selected[0].get("title", ""), "read-back says %s is selected, not %s" % (selected[0], want)
print("tab select read-back ok")
' "$TABS" "$TAB_B"

      echo "== browser: page text of the selected tab =="
      TEXT=""
      for _ in $(seq 1 20); do
        TEXT="$(run_json --target current --grant observe page text --window "$HANDLE" || true)"
        if python3 -c '
import json, sys
try:
    rows = (json.loads(sys.argv[1]).get("data") or {}).get("rows") or []
except Exception:
    raise SystemExit(1)
blob = " ".join(str(r.get("text", "")) for r in rows)
raise SystemExit(0 if "bravo marker two" in blob else 1)
' "$TEXT"; then break; fi
        sleep 0.5
      done
      python3 -c '
import json, sys
rows = json.loads(sys.argv[1])["data"]["rows"]
blob = " ".join(str(r.get("text", "")) for r in rows)
assert "bravo marker two" in blob, "page text did not carry the selected tab words: %r" % blob[:400]
assert any(r.get("id") for r in rows), "page text rows must name the node that carries them"
print("page text ok:", len(rows), "rows")
' "$TEXT"

      echo "== browser: back to $TAB_A so the CDP read is against a background tab =="
      OUT="$(run_json --target current --grant actuate tab select --window "$HANDLE" --title "$TAB_A")"
      test "$(json_field "$OUT" ok)" = "True"
      TABS="$(run_json --target current --grant observe tab list --window "$HANDLE")"
      python3 -c '
import json, sys
tabs = json.loads(sys.argv[1])["data"]["tabs"]
selected = [t for t in tabs if t.get("selected") is True]
assert len(selected) == 1 and sys.argv[2] in selected[0].get("title", ""), "expected %s active, got %s" % (sys.argv[2], selected)
' "$TABS" "$TAB_A"

      echo "== browser: page targets --pid =="
      OUT="$(run_json --target current --grant observe page targets --pid "$BROWSER_PID")"
      test "$(json_field "$OUT" ok)" = "True"
      python3 -c '
import json, sys
targets = json.loads(sys.argv[1])["data"]["targets"]
titles = [t.get("title", "") for t in targets]
for want in sys.argv[2:]:
    assert any(want in title for title in titles), "CDP target %s missing from %s" % (want, titles)
print("page targets ok:", titles)
' "$OUT" "$TAB_A" "$TAB_B"

      echo "== browser: page-js against the background tab $TAB_B =="
      OUT="$(run_json --target current --grant observe page-js --port "$CDP_PORT" --target-title "$TAB_B" --expression document.title)"
      test "$(json_field "$OUT" ok)" = "True"
      python3 -c '
import json, sys
data = json.loads(sys.argv[1])["data"]
want = sys.argv[2]
value = data.get("value")
if isinstance(value, dict):
    value = value.get("value")
assert value == want, "page-js answered %r, expected %r (A is the active tab)" % (value, want)
print("page-js on the background tab ok:", value)
' "$OUT" "$TAB_B"

      BROWSER_SECTION="passed"
    fi
  fi
fi

if [[ "$BROWSER_SECTION" == "passed" ]]; then
  echo "PASS: cu-linux-smoke (browser section PASSED)"
else
  echo "PASS: cu-linux-smoke (browser section SKIPPED[${BROWSER_SKIP_CODE:-unknown}] -- not evidence of browser support)"
fi
